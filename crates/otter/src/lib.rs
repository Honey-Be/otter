//! # otter — L0 meta-VM generation framework (the meta-hybrid JIT toolchain)
//!
//! `otter` is neosurf's L0: the substrate that turns a single annotated source into a multi-tier,
//! multi-strategy VM. The crate name is the framework's name; the runtime (L1) and engine (L2) build on
//! it. The machine-code backend lives in `otter-cranelift`.
//!
//! Vision (see `.claude-research-library/engine-stack-decision.md` §2 and `jit-approaches-survey.md`):
//! a single annotated source `{bytecode semantics + JIT hints}` generates a multi-tier,
//! multi-strategy VM — T0 specializing interpreter + T1 copy-and-patch baseline (Deegen-style)
//! and a hint-driven T2 optimizing tier (method/PE; optional LBBV; meta-tracing optional).
//! Unifies Deegen + Truffle + RPython virtues behind ONE strategy-agnostic hint vocabulary.
//!
//! # The "single source" (B0)
//! An operation's semantics are written ONCE as a small **reifiable IR** ([`Expr`] + [`Effect`]) that
//! the framework can both *evaluate* (the interpreter, [`interp::interpret`] — the PyPy-style reference
//! oracle) and *lower to machine code* (the `otter-cranelift` backend). Defining a new op is a
//! data entry in an [`Isa`], not a new match arm in every backend — that is the "meta" property. The
//! only closed, framework-level set is the primitive ALU ([`BinOp`]), like a CPU's instruction set;
//! bytecode ops compose primitives.
//!
//! Abstract machine: **register-based** (V8 Ignition / Lua style) — state is a flat array of `i64`
//! locals plus a program counter. Cleaner to lower to SSA than a stack machine, and the modern choice
//! for a JS bytecode.
//!
//! Config (memory: runtime-rust-toolchain-config): `no_std` + `alloc` (std only under `cargo test`),
//! `#![forbid(unsafe_code)]` — the unsafe machine-code emission lives in the backend platform shim.
#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "f16-intrinsic", feature(f16))]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// --- f16 codec: the ONLY config-dependent f16 primitives. `half` (default) and the native `f16`
// (nightly `f16-intrinsic`) both implement IEEE-754 binary16 round-to-nearest-even, so they MUST agree
// bit-for-bit — the bytecode result never depends on which is built (a cross-config test gates this).
#[cfg(all(not(feature = "f16-intrinsic"), feature = "half-codec"))]
#[inline]
fn f16_to_f64(bits: u16) -> f64 {
    half::f16::from_bits(bits).to_f64()
}
#[cfg(all(not(feature = "f16-intrinsic"), feature = "half-codec"))]
#[inline]
fn f64_to_f16_bits(x: f64) -> u16 {
    half::f16::from_f64(x).to_bits()
}
#[cfg(feature = "f16-intrinsic")]
#[inline]
fn f16_to_f64(bits: u16) -> f64 {
    f16::from_bits(bits) as f64
}
#[cfg(feature = "f16-intrinsic")]
#[inline]
fn f64_to_f16_bits(x: f64) -> u16 {
    (x as f16).to_bits()
}
#[cfg(all(not(feature = "f16-intrinsic"), not(feature = "half-codec")))]
compile_error!("enable exactly one of `half-codec` (default, stable) or `f16-intrinsic` (nightly)");

#[inline]
fn as_f16_word(w: Value) -> f64 {
    f16_to_f64(w as u16)
}
#[inline]
fn from_f16(x: f64) -> Value {
    f64_to_f16_bits(x) as Value // zero-extend the 16-bit pattern into the word
}

/// A host (runtime) builtin callable from bytecode: `extern "C"` so the JIT tiers can call it with the
/// SystemV ABI; the interpreter calls it directly (a safe fn-pointer call). The pointer is baked into
/// the IR ([`Expr::Host1`]/[`Expr::Host2`]) by whoever builds the ISA — no host-table threading.
pub type HostFn1 = extern "C" fn(Value) -> Value;
/// A binary host builtin (see [`HostFn1`]).
pub type HostFn2 = extern "C" fn(Value, Value) -> Value;

/// A host builtin that additionally receives an embedder-owned **context word** as its FIRST argument
/// (matching `otter-engine`'s own `dispatch(ctx, ...)` convention): `extern "C"` so the JIT tiers can
/// call it with the SystemV ABI; the interpreter calls it directly. `host_ctx` is otherwise identical in
/// spirit to [`Module::host_ctx`] — `otter` never inspects or dereferences it, the embedder assigns it
/// whatever meaning it needs (a heap pointer, a symbol-table handle, an interpreter-instance id, …).
/// Unlike the fn pointer (per-callsite IR data, resolved when the ISA is built), `host_ctx` is a
/// per-[`Module`] constant baked into compiled code at compile time on every tier — it costs nothing
/// beyond an immediate load at each call site (see [`Expr::HostCtx1`]). Deliberately named `host_ctx`,
/// not `ctx`: on the `ModuleEngine` (Dispatch-mode) compiled ABI, `ctx` already names a DIFFERENT
/// pointer — the engine's own bookkeeping pointer threaded through every compiled call so it can call
/// back into `module_dispatch`/`tail_signal` (see `otter-cranelift`/`otter-baseline`'s `opaque_ptr` and
/// `otter-engine`'s `CompiledFn` ABI). Compiled code never dereferences that pointer; `host_ctx` is a
/// completely separate, embedder-owned word that happens to live in the same stack frame. Use
/// [`HostFn1`] instead when a builtin needs no per-instance state.
pub type HostFnCtx1 = extern "C" fn(Value, Value) -> Value; // (host_ctx, x) -> result
/// A binary host builtin with context (see [`HostFnCtx1`]): `(host_ctx, x, y) -> result`.
pub type HostFnCtx2 = extern "C" fn(Value, Value, Value) -> Value;

/// A runtime value is a **raw 64-bit word** — the universal slot of this language-agnostic machine
/// (WASM/CPU model). Integers are stored directly; floating-point values are stored as their IEEE-754
/// bits (`f64::to_bits`). Operations are *typed* (see [`BinOp`]/[`UnOp`]): integer ops read the word as
/// `i64`, float ops reinterpret it as `f64`. Booleans are `0`/`1` words. (GC references will later be
/// tagged words too.)
pub type Value = i64;

#[inline]
fn as_f64(w: Value) -> f64 {
    f64::from_bits(w as u64)
}
#[inline]
fn from_f64(x: f64) -> Value {
    x.to_bits() as Value
}
#[inline]
fn as_f32(w: Value) -> f32 {
    f32::from_bits(w as u32)
}
#[inline]
fn from_f32(x: f32) -> Value {
    x.to_bits() as Value // zero-extend the 32-bit pattern into the word
}

/// A numeric primitive type. The 64-bit slot is *normalized* per type: signed ints are sign-extended,
/// unsigned ints zero-extended, floats hold their IEEE bits in the low `bits()` bits (the rest zero).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumTy {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F16,
    F32,
    F64,
}

impl NumTy {
    pub fn is_float(self) -> bool {
        matches!(self, NumTy::F16 | NumTy::F32 | NumTy::F64)
    }
    pub fn is_signed_int(self) -> bool {
        matches!(self, NumTy::I8 | NumTy::I16 | NumTy::I32 | NumTy::I64)
    }
    pub fn bits(self) -> u32 {
        match self {
            NumTy::I8 | NumTy::U8 => 8,
            NumTy::I16 | NumTy::U16 | NumTy::F16 => 16,
            NumTy::I32 | NumTy::U32 | NumTy::F32 => 32,
            NumTy::I64 | NumTy::U64 | NumTy::F64 => 64,
        }
    }
    /// Re-normalize a raw word to this *integer* type's canonical slot form (truncate to width, then
    /// sign- or zero-extend). For floats this is the identity (the float ops produce normalized bits).
    pub fn norm(self, w: Value) -> Value {
        match self {
            NumTy::I8 => (w as i8) as Value,
            NumTy::I16 => (w as i16) as Value,
            NumTy::I32 => (w as i32) as Value,
            NumTy::U8 => (w as u8) as Value,
            NumTy::U16 => (w as u16) as Value,
            NumTy::U32 => (w as u32) as Value,
            NumTy::I64 | NumTy::U64 | NumTy::F16 | NumTy::F32 | NumTy::F64 => w,
        }
    }
    /// Stable numeric tag (for the FFI helpers the JIT backends call).
    pub fn tag(self) -> u32 {
        self as u32
    }
    pub fn from_tag(t: u32) -> NumTy {
        use NumTy::*;
        [I8, I16, I32, I64, U8, U16, U32, U64, F16, F32, F64][t as usize]
    }
}

/// The arithmetic/comparison a [`BinOp`] performs (the type is carried separately). `Div` is float-only
/// for now (integer division is deferred).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Eq,
}

impl ArithOp {
    pub fn tag(self) -> u32 {
        self as u32
    }
    pub fn from_tag(t: u32) -> ArithOp {
        use ArithOp::*;
        [Add, Sub, Mul, Div, Lt, Eq][t as usize]
    }
}

/// FFI helper: a binary op via the canonical interpreter semantics. The JIT backends `call` this for
/// ops they don't lower natively (notably `f16`), guaranteeing bit-identical results across all tiers
/// and across the `half`/`f16-intrinsic` configs.
pub extern "C" fn otter_binop(a: Value, b: Value, ty: u32, op: u32) -> Value {
    BinOp::new(NumTy::from_tag(ty), ArithOp::from_tag(op)).eval(a, b)
}

/// FFI helper: a conversion via the canonical interpreter semantics (see [`otter_binop`]).
pub extern "C" fn otter_convert(a: Value, from: u32, to: u32) -> Value {
    UnOp::Convert {
        from: NumTy::from_tag(from),
        to: NumTy::from_tag(to),
    }
    .eval(a)
}

/// FFI helper: a *unary* op via the canonical interpreter semantics (the [`otter_binop`] pattern).
/// Added for the **wasm-emit backend** (O-13 design §4.1 ⑤ / §4.2): `Twasm` lowers every
/// `Expr::Un(UnOp::Neg(..), ..)` through this helper (`call_indirect`), guaranteeing bit-identical
/// results with T0/T1/T2 and across the `half`/`f16-intrinsic` configs by construction.
///
/// Tag scheme (mirrors `otter_binop`'s `(value.., ty, op)` order): `ty` is a [`NumTy::tag`]; `op`
/// selects the [`UnOp`] *variant* — [`UnOp::NEG_TAG`] (= 0) is `Neg(ty)`. `Convert` is deliberately
/// NOT in this tag space: it carries **two** types and already has its own total helper,
/// [`otter_convert`]. Future single-type unary variants extend `op` (1, 2, …).
///
/// **Canonical-reuse contract**: the body delegates verbatim to [`UnOp::eval`] and must NEVER
/// re-implement its semantics — any hand copy here could silently diverge from the tiers' shared
/// canonical source. Tags are constants baked by a backend at lowering time, so an out-of-range
/// tag is a backend bug (panics, exactly like [`NumTy::from_tag`] on an OOB index).
pub extern "C" fn otter_unop(a: Value, ty: u32, op: u32) -> Value {
    match op {
        UnOp::NEG_TAG => UnOp::Neg(NumTy::from_tag(ty)).eval(a),
        _ => unreachable!("unknown UnOp tag"),
    }
}

/// Identifies this layer (kept for the L1 scaffold's smoke test).
pub fn layer() -> String {
    String::from("L0 meta-VM framework")
}

// ---------------------------------------------------------------------------------------------------
// Reifiable semantic IR — the single source consumed by interpreter, JIT, and (later) the optimizer.
// ---------------------------------------------------------------------------------------------------

/// The primitive ALU — a closed, framework-level, *typed* op set (like a CPU's): an [`ArithOp`] applied
/// at a [`NumTy`]. Operands/result are read/written in the type's normalized slot form. Bytecode ops
/// compose these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinOp {
    pub ty: NumTy,
    pub op: ArithOp,
}

impl BinOp {
    pub const fn new(ty: NumTy, op: ArithOp) -> Self {
        BinOp { ty, op }
    }

    /// Reference semantics (the interpreter's; the JITs must match these exactly). Operands are assumed
    /// already normalized for `ty`; the result is returned normalized.
    pub fn eval(self, a: Value, b: Value) -> Value {
        use ArithOp::*;
        match self.ty {
            NumTy::F64 => {
                let (x, y) = (as_f64(a), as_f64(b));
                match self.op {
                    Add => from_f64(x + y),
                    Sub => from_f64(x - y),
                    Mul => from_f64(x * y),
                    Div => from_f64(x / y),
                    Lt => (!x.is_nan() && !y.is_nan() && x < y) as Value,
                    Eq => (x == y) as Value,
                }
            }
            NumTy::F32 => {
                let (x, y) = (as_f32(a), as_f32(b));
                match self.op {
                    Add => from_f32(x + y),
                    Sub => from_f32(x - y),
                    Mul => from_f32(x * y),
                    Div => from_f32(x / y),
                    Lt => (!x.is_nan() && !y.is_nan() && x < y) as Value,
                    Eq => (x == y) as Value,
                }
            }
            NumTy::F16 => {
                // arithmetic via an f64 intermediate, then a single round to f16 (exact for add/sub/mul)
                let (x, y) = (as_f16_word(a), as_f16_word(b));
                match self.op {
                    Add => from_f16(x + y),
                    Sub => from_f16(x - y),
                    Mul => from_f16(x * y),
                    Div => from_f16(x / y),
                    Lt => (!x.is_nan() && !y.is_nan() && x < y) as Value,
                    Eq => (x == y) as Value,
                }
            }
            _ => {
                // integer (signed or unsigned), all widths handled via normalize
                match self.op {
                    Add => self.ty.norm(a.wrapping_add(b)),
                    Sub => self.ty.norm(a.wrapping_sub(b)),
                    Mul => self.ty.norm(a.wrapping_mul(b)),
                    Lt => {
                        if self.ty.is_signed_int() {
                            (a < b) as Value
                        } else {
                            ((a as u64) < (b as u64)) as Value
                        }
                    }
                    Eq => (a == b) as Value,
                    Div => unreachable!("integer division is not yet supported"),
                }
            }
        }
    }

    // --- algebraic-law hints (integer-only; float laws are unsound for ±0/NaN/Inf) ---

    /// `a OP b == b OP a`?
    pub fn is_commutative(self) -> bool {
        !self.ty.is_float() && matches!(self.op, ArithOp::Add | ArithOp::Mul | ArithOp::Eq)
    }

    /// An `e` such that `a OP e == a` for all `a` (right identity), if any (integer only).
    pub fn right_identity(self) -> Option<Value> {
        if self.ty.is_float() {
            return None;
        }
        match self.op {
            ArithOp::Add | ArithOp::Sub => Some(0),
            ArithOp::Mul => Some(1),
            _ => None,
        }
    }

    /// An `e` such that `e OP a == a OP e == e` for all `a` (absorbing element), if any (integer only;
    /// `x*0.0` is not `0.0` for `Inf`/`NaN`).
    pub fn absorbing_element(self) -> Option<Value> {
        if !self.ty.is_float() && self.op == ArithOp::Mul {
            Some(0)
        } else {
            None
        }
    }
}

/// A primitive *unary* op on the word machine: negation at a type, or a numeric conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// negate at `NumTy` (integer two's-complement wrap, or float sign flip)
    Neg(NumTy),
    /// convert a value of `from` to `to` (int↔int truncate/extend, int↔float, float↔float;
    /// float→int is saturating + round-toward-zero, matching Rust `as`).
    Convert { from: NumTy, to: NumTy },
}

impl UnOp {
    /// Stable `op` tag of the [`UnOp::Neg`] variant for the [`otter_unop`] FFI helper (backends bake
    /// it as a constant; see `otter_unop`'s doc for the tag scheme).
    pub const NEG_TAG: u32 = 0;

    /// Reference semantics.
    pub fn eval(self, a: Value) -> Value {
        match self {
            UnOp::Neg(ty) => match ty {
                NumTy::F64 => from_f64(-as_f64(a)),
                NumTy::F32 => from_f32(-as_f32(a)),
                // IEEE-754 binary16 sign flip — the `fneg` twin of the F64/F32 arms above: flips
                // ONLY the sign bit, so NaN payloads are preserved (an f16→f64→negate→f16 round
                // trip would NOT be — widening/rounding canonicalizes NaNs — and is therefore
                // forbidden here). `a as u16` truncates to the 16 IEEE bits and the `as Value`
                // zero-extension re-establishes the [`NumTy`] normalization invariant (IEEE bits
                // in the low `bits()` bits, the rest zero). W-0.5 canonical-semantics fix (O-13
                // design §4.1): the old fall-through to the integer arm produced a full 64-bit
                // two's-complement word, violating that invariant and the IEEE result.
                NumTy::F16 => ((a as u16) ^ 0x8000) as Value,
                _ => ty.norm(a.wrapping_neg()),
            },
            UnOp::Convert { from, to } => convert(a, from, to),
        }
    }
}

/// `a` (a value of type `from`, normalized) converted to type `to` (normalized).
fn convert(a: Value, from: NumTy, to: NumTy) -> Value {
    // float source: read its value as f64 (f16/f32 widen exactly)
    if from.is_float() {
        let x = match from {
            NumTy::F16 => as_f16_word(a),
            NumTy::F32 => as_f32(a) as f64,
            _ => as_f64(a),
        };
        return match to {
            NumTy::F16 => from_f16(x),
            NumTy::F32 => from_f32(x as f32),
            NumTy::F64 => from_f64(x),
            // float -> int: Rust `as` saturates (NaN→0, ±Inf→min/max) and rounds toward zero
            NumTy::I8 => (x as i8) as Value,
            NumTy::I16 => (x as i16) as Value,
            NumTy::I32 => (x as i32) as Value,
            NumTy::I64 => x as i64,
            NumTy::U8 => (x as u8) as Value,
            NumTy::U16 => (x as u16) as Value,
            NumTy::U32 => (x as u32) as Value,
            NumTy::U64 => (x as u64) as Value,
        };
    }
    // integer source
    if to.is_float() {
        // the integer's value: signed sources are sign-extended in the slot; unsigned zero-extended
        let v = if from.is_signed_int() {
            a as f64
        } else {
            (a as u64) as f64
        };
        return match to {
            NumTy::F16 => from_f16(v),
            NumTy::F32 => from_f32(v as f32),
            NumTy::F64 => from_f64(v),
            _ => unreachable!(),
        };
    }
    // int -> int: re-normalize the low bits to the destination type
    to.norm(a)
}

/// A pure expression over an instruction's operand slots and the locals array.
///
/// Operands are referenced by *slot index* (0..N of an [`Inst`]'s operands) so an op's semantics are
/// generic over the concrete operand values a given instruction carries. (No `PartialEq`/`Eq`: it holds
/// an `f64` and host fn-pointers; the optimizer inspects it by pattern-matching, never by `==`.)
#[derive(Clone, Debug)]
pub enum Expr {
    /// An integer literal word baked into the semantics.
    Imm(Value),
    /// A floating-point literal (stored as its IEEE bits in the word).
    ImmF64(f64),
    /// Operand slot `i` taken as a raw (integer) immediate.
    OperandImm(u8),
    /// `locals[ operand_slot(i) ]` — operand `i` is a register index.
    LocalAt(u8),
    /// Apply a primitive binary op to two sub-expressions.
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// Apply a primitive unary op to a sub-expression.
    Un(UnOp, Box<Expr>),
    /// Call a unary **host** builtin (a runtime function baked into the IR) on a sub-expression.
    Host1(HostFn1, Box<Expr>),
    /// Call a binary **host** builtin on two sub-expressions.
    Host2(HostFn2, Box<Expr>, Box<Expr>),
    /// Call a unary **host** builtin **with the module's [`Module::host_ctx`] word** prepended — the
    /// context-carrying sibling of [`Expr::Host1`]. Use this when the builtin needs per-embedder-instance
    /// state (e.g. a Scheme interpreter's heap/symbol-table/dynamic-environment pointer) instead of a
    /// global or thread-local. `otter` treats `host_ctx` exactly like [`Module::fill`]: an opaque word it
    /// threads through unexamined, baked into compiled code as a per-module compile-time constant (never
    /// a runtime register) — so it costs nothing beyond an immediate load at each call site, on every
    /// tier and every `CallMode`. **Only meaningful when executed via a [`Module`]** (`interp::run_module`
    /// / `run_one` / a `ModuleEngine`); the single-function, `Module`-less layer (`interp::interpret*`,
    /// `otter::engine::Engine`, `otter-cranelift::compile_fn`/`compile_resumable`,
    /// `otter-baseline::compile`) has no `Module` to read a `host_ctx` from, so it always sees `0` there
    /// — consistent across ALL of that layer's own tiers (T0/T1/T2), but NOT the same word a `Module`
    /// executing the identical `Expr` would see.
    HostCtx1(HostFnCtx1, Box<Expr>),
    /// Call a binary **host** builtin with context — see [`Expr::HostCtx1`].
    HostCtx2(HostFnCtx2, Box<Expr>, Box<Expr>),
}

/// One side effect of executing an op. Effects run in order; control effects set the next pc.
///
/// **Framework contract — a control effect must come LAST**: a control effect
/// ([`Effect::JumpTo`], [`Effect::JumpIf`], [`Effect::Halt`], [`Effect::Return`],
/// [`Effect::TailCall`]) must be the final entry of an [`OpDef`]'s `effects` list. The T0
/// interpreter does not enforce this — a taken `JumpTo`/`JumpIf` merely records the next pc and
/// the REMAINING effects still execute (`Halt`/`Return`/`TailCall` return immediately instead) —
/// but every compiling tier ends its basic block at the control effect — and the failure mode of
/// a violating `Isa` is NOT uniformly loud: native cranelift at least fails loudly at compile
/// time, but the baseline (T1) linear emitter keeps emitting the residual effects after a
/// `JumpTo`/`JumpIf` unchecked, baking in a silent divergence where the taken edge skips residue
/// T0 still executes (measured), and in a wasm dispatcher loop the residue would sit after the
/// `br` as *validating dead code*. An `Isa` that violates this contract therefore produces silent
/// tier divergence; the wasm-emit backend refuses such functions outright (O-13 design §4.6-4,
/// `otter-wasmemit::Refusal::ControlEffectNotLast`).
#[derive(Clone, Debug)]
pub enum Effect {
    /// `locals[ operand_slot(reg) ] = eval(value)`.
    SetLocalAt { reg: u8, value: Expr },
    /// Unconditional `pc = operand_slot(target)`.
    JumpTo { target: u8 },
    /// `if eval(cond) != 0 { pc = operand_slot(target) }`.
    JumpIf { cond: Expr, target: u8 },
    /// Stop and return `eval(value)`.
    Halt { value: Expr },
    /// Speculation marker: the optimizing tier may assume `eval(cond) != 0` and **deoptimize** (bail
    /// back to the interpreter at this pc) if it does not hold. Semantically a **no-op** — the result is
    /// identical whether or not a deopt occurs, which is exactly what makes deopt safe. The reference
    /// interpreter ignores it.
    Guard { cond: Expr },
    /// **Function call** (a [`Module`] construct — see [`interp::run_module`]). All three fields are
    /// *operand indices*: `dest` = the slot to receive the return value; `base` = the slot of a
    /// contiguous block laid out as `[callee, arg0, … arg(argc-1)]` (the callee word is a raw function
    /// index into `Module::funcs`); `argc` = the argument count. All three tiers lower `Call`: the T0
    /// interpreter dispatches it directly, and the baseline/cranelift backends lower it in their
    /// `compile_dispatch_fn` path (only the single-function, call-free `compile`/`compile_module` entry
    /// points leave `Call` `unimplemented!`). The `ModuleEngine` routes every call through one dispatcher.
    Call { dest: u8, base: u8, argc: u8 },
    /// **Tail call** — a call that REPLACES the current frame instead of stacking a new one, so unbounded
    /// tail recursion runs in bounded stack (R7RS proper tail calls; also the WASM tail-call proposal).
    /// Operand convention is `Call`'s minus `dest`: `base` = the slot of a `[callee, arg0, … arg(argc-1)]`
    /// block; the callee's result becomes THIS function's return value (the outgoing frame's return
    /// target is preserved). A terminator. The oracle [`interp::run_module`] replaces the top frame in
    /// place; the adaptive engine trampolines (a tail op signals "next call" and returns, and the
    /// dispatcher re-enters without recursing — see `otter-engine`). The baseline (T1) tier does NOT
    /// implement it and refuses to compile a function containing it (the engine keeps such a function on
    /// T0/T2). See `.claude-research-library/otter-o8-tailcall-design.md`.
    TailCall { base: u8, argc: u8 },
    /// **Return** `eval(value)` from the current function frame to its caller.
    Return { value: Expr },
}

/// One function in a [`Module`]: a body plus its frame shape. Frame layout (leading locals):
/// `[ params (nparams) ] [ consts (consts.len()) ] [ temporaries … ]`, total `nlocals`. At call time the
/// frame is initialized to [`Module::fill`], then arguments overwrite the parameter slots and `consts`
/// initialize the slots right after them. (For the entry/"main" function `nparams == 0`, so `consts`
/// occupy the leading slots — exactly the single-function "constants ride in the arg block" convention.)
#[derive(Clone, Debug)]
pub struct FuncDef {
    pub nparams: usize,
    pub consts: Vec<Value>,
    pub nlocals: usize,
    pub code: Vec<Inst>,
}

/// A collection of functions executed with a call stack by [`interp::run_module`]. `fill` is the word a
/// fresh frame's locals (and any missing argument) are initialized to — the embedder owns its meaning
/// (the JS runtime uses the NaN-boxed `undefined`), keeping `otter` value-agnostic. `entry` is the
/// function index run first.
#[derive(Clone, Debug)]
pub struct Module {
    pub funcs: Vec<FuncDef>,
    pub fill: Value,
    /// The word passed as the first argument to every [`Expr::HostCtx1`]/[`Expr::HostCtx2`] call in this
    /// module — the embedder owns its meaning (mirrors [`Module::fill`]'s "value-agnostic" contract
    /// exactly; e.g. an R7RS embedding could use it to carry a pointer to the enclosing Scheme
    /// interpreter instance, so multiple independent instances in one process never collide). Ignored
    /// entirely by ordinary [`Expr::Host1`]/[`Expr::Host2`] builtins. A compile-time constant: the same
    /// word for every call within one `Module`/`ModuleEngine`, baked into compiled code at
    /// compile-tier-up time (T1/T2) exactly like `fill` — never threaded as a live register. Deliberately
    /// named `host_ctx`, not `ctx` — see [`HostFnCtx1`]'s doc for why that name is reserved for a
    /// different (engine-internal) pointer on the Dispatch-mode compiled ABI.
    pub host_ctx: Value,
    pub entry: usize,
}

/// Strategy-agnostic hint vocabulary — ONE set drives all tiers (interp dispatch, JIT block-splitting,
/// optimizer). B0 starts minimal; the vocabulary grows but stays strategy-neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OpHints {
    /// No effect beyond its `SetLocalAt` writes — foldable when all inputs are constant.
    pub pure: bool,
    /// Ends a basic block (its last effect transfers control or halts).
    pub terminator: bool,
}

/// An operation definition — the single source of truth for one opcode.
#[derive(Clone, Debug)]
pub struct OpDef {
    pub name: &'static str,
    pub effects: Vec<Effect>,
    pub hints: OpHints,
}

/// An instruction set: opcode `u16` indexes `ops`.
#[derive(Clone, Debug, Default)]
pub struct Isa {
    pub ops: Vec<OpDef>,
}

impl Isa {
    pub fn op(&self, opcode: u16) -> &OpDef {
        &self.ops[opcode as usize]
    }
}

/// Number of operand slots per instruction (fixed 3-address format for B0).
pub const NOPERANDS: usize = 3;

/// A decoded instruction: opcode + fixed operand slots (register indices, immediates, or pc targets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inst {
    pub op: u16,
    pub operands: [i32; NOPERANDS],
}

impl Inst {
    pub fn new(op: u16, a: i32, b: i32, c: i32) -> Self {
        Inst {
            op,
            operands: [a, b, c],
        }
    }
    #[inline]
    pub fn operand(&self, slot: u8) -> i32 {
        self.operands[slot as usize]
    }
}

// ---------------------------------------------------------------------------------------------------
// Tier 0: the generic interpreter — the semantic reference oracle (correctness-first, PyPy style).
// ---------------------------------------------------------------------------------------------------

pub mod interp {
    use super::{Effect, Expr, FuncDef, Inst, Isa, Module, Value};
    use alloc::vec;
    use alloc::vec::Vec;

    /// Evaluate a pure [`Expr`] against the current instruction's operands and the locals. `host_ctx` is
    /// the word threaded to any [`Expr::HostCtx1`]/[`Expr::HostCtx2`] encountered — see their docs (it is
    /// [`Module::host_ctx`] when running under a `Module`, or `0` on the `Module`-less single-function
    /// layer — see [`interpret_from`]).
    pub fn eval(e: &Expr, inst: &Inst, locals: &[Value], host_ctx: Value) -> Value {
        match e {
            Expr::Imm(v) => *v,
            Expr::ImmF64(f) => super::from_f64(*f),
            Expr::OperandImm(i) => inst.operand(*i) as Value,
            Expr::LocalAt(i) => locals[inst.operand(*i) as usize],
            Expr::Bin(op, a, b) => op.eval(eval(a, inst, locals, host_ctx), eval(b, inst, locals, host_ctx)),
            Expr::Un(op, a) => op.eval(eval(a, inst, locals, host_ctx)),
            Expr::Host1(f, a) => f(eval(a, inst, locals, host_ctx)),
            Expr::Host2(f, a, b) => f(eval(a, inst, locals, host_ctx), eval(b, inst, locals, host_ctx)),
            Expr::HostCtx1(f, a) => f(host_ctx, eval(a, inst, locals, host_ctx)),
            Expr::HostCtx2(f, a, b) => f(
                host_ctx,
                eval(a, inst, locals, host_ctx),
                eval(b, inst, locals, host_ctx),
            ),
        }
    }

    /// Run `prog` with no arguments (all locals zero-initialized).
    pub fn interpret(isa: &Isa, prog: &[Inst], nlocals: usize) -> Value {
        interpret_with(isa, prog, nlocals, &[])
    }

    /// Run `prog` under `isa` with `nlocals` registers; the first `args.len()` are initialized from
    /// `args` (the function's arguments), the rest to zero. Returns the `Halt` value.
    pub fn interpret_with(isa: &Isa, prog: &[Inst], nlocals: usize, args: &[Value]) -> Value {
        let mut locals = vec![0 as Value; nlocals];
        for (slot, &a) in args.iter().enumerate() {
            locals[slot] = a;
        }
        interpret_from(isa, prog, &mut locals, 0)
    }

    /// Resume interpretation from `pc` with the given `locals` (mutated in place). This is the **deopt
    /// resume** entry: a JIT tier that bails out hands back its live `locals` + the pc, and the
    /// interpreter — the semantic oracle — finishes the rest. Returns the `Halt` value.
    /// Panics on a malformed program (out-of-range pc/local/opcode) — fine for the reference tier.
    ///
    /// **No `Module`, so no `host_ctx` source**: any [`Expr::HostCtx1`]/[`Expr::HostCtx2`] reached here
    /// (this is also the T0 path under `otter::engine::Engine`, which is likewise `Module`-less) sees a
    /// fixed `host_ctx = 0` — the same fixed value `otter-cranelift::compile_fn`/`compile_resumable` and
    /// `otter-baseline::compile` bake in for this exact layer, so T0/T1/T2 stay consistent WITHIN this
    /// `Module`-less layer even though it is a different word than a `Module` executing the identical
    /// `Expr` would see (see [`Expr::HostCtx1`]'s doc). This mirrors the pre-existing precedent that this
    /// layer has no [`Module::fill`] either (`locals` is caller-initialized, never module-`fill`-filled).
    pub fn interpret_from(isa: &Isa, prog: &[Inst], locals: &mut [Value], mut pc: usize) -> Value {
        const HOST_CTX: Value = 0;
        loop {
            let inst = &prog[pc];
            let op = isa.op(inst.op);
            let mut next = pc + 1;
            for eff in &op.effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        let dst = inst.operand(*reg) as usize;
                        locals[dst] = eval(value, inst, locals, HOST_CTX);
                    }
                    Effect::JumpTo { target } => {
                        next = inst.operand(*target) as usize;
                    }
                    Effect::JumpIf { cond, target } => {
                        if eval(cond, inst, locals, HOST_CTX) != 0 {
                            next = inst.operand(*target) as usize;
                        }
                    }
                    Effect::Halt { value } => {
                        return eval(value, inst, locals, HOST_CTX);
                    }
                    Effect::Guard { .. } => {} // speculation marker — no-op in the reference tier
                    // Single-frame interpreter (deopt resume / engine tier): function calls run on
                    // `run_module`, never here. The frontend never mixes calls into a JIT'd function.
                    Effect::Call { .. } | Effect::Return { .. } | Effect::TailCall { .. } => {
                        unreachable!(
                            "Call/Return/TailCall execute on run_module, not the single-frame interpreter"
                        )
                    }
                }
            }
            pc = next;
        }
    }

    /// Outcome of [`interpret_until_osr`].
    pub enum Step {
        /// The program ran to a `Halt`.
        Completed(Value),
        /// A loop back-edge to `pc` got hot; the caller should **on-stack-replace** into a compiled
        /// tier entering at `pc`, using the (now updated) `locals` as the live frame.
        Osr(usize),
    }

    /// Like [`interpret_from`] but counts loop back-edges (a control transfer to a `pc <= current`);
    /// when one reaches `osr_threshold`, returns [`Step::Osr`] with `locals` holding the live frame at
    /// that loop header. This is the **OSR trigger**: a long-running loop that was only *called* once
    /// (so call-count tiering never fires) can still be promoted mid-flight.
    pub fn interpret_until_osr(
        isa: &Isa,
        prog: &[Inst],
        locals: &mut [Value],
        mut pc: usize,
        osr_threshold: u32,
    ) -> Step {
        // No `Module` here either — see `interpret_from`'s doc for why `host_ctx` is fixed at 0.
        const HOST_CTX: Value = 0;
        let mut backedge = vec![0u32; prog.len()];
        loop {
            let inst = &prog[pc];
            let op = isa.op(inst.op);
            let mut next = pc + 1;
            for eff in &op.effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        let dst = inst.operand(*reg) as usize;
                        locals[dst] = eval(value, inst, locals, HOST_CTX);
                    }
                    Effect::JumpTo { target } => {
                        next = inst.operand(*target) as usize;
                    }
                    Effect::JumpIf { cond, target } => {
                        if eval(cond, inst, locals, HOST_CTX) != 0 {
                            next = inst.operand(*target) as usize;
                        }
                    }
                    Effect::Halt { value } => {
                        return Step::Completed(eval(value, inst, locals, HOST_CTX));
                    }
                    Effect::Guard { .. } => {}
                    Effect::Call { .. } | Effect::Return { .. } | Effect::TailCall { .. } => {
                        unreachable!(
                            "Call/Return/TailCall execute on run_module, not the single-frame interpreter"
                        )
                    }
                }
            }
            if next <= pc {
                backedge[next] += 1;
                if backedge[next] >= osr_threshold {
                    return Step::Osr(next);
                }
            }
            pc = next;
        }
    }

    /// One activation record on the call stack.
    struct Frame {
        func: usize,
        pc: usize,
        locals: Vec<Value>,
        /// Slot in the **caller** that receives this frame's return value.
        ret_dest: usize,
    }

    fn make_frame(module: &Module, func_idx: usize, args: &[Value]) -> Frame {
        let f: &FuncDef = &module.funcs[func_idx];
        let mut locals = vec![module.fill; f.nlocals];
        for (i, &a) in args.iter().take(f.nparams).enumerate() {
            locals[i] = a; // missing params stay `fill`; extra args are ignored (no `arguments` yet)
        }
        for (i, &c) in f.consts.iter().enumerate() {
            locals[f.nparams + i] = c;
        }
        Frame {
            func: func_idx,
            pc: 0,
            locals,
            ret_dest: 0,
        }
    }

    /// Run a multi-function [`Module`] with an explicit call stack — the reference oracle for function
    /// calls. `args` initialize the entry function's parameters. Returns the value of the first `Halt`
    /// reached, or the value the entry function `Return`s if it returns past the bottom of the stack.
    /// Panics on a malformed module (out-of-range func/pc/local/opcode) — fine for the reference tier.
    pub fn run_module(isa: &Isa, module: &Module, args: &[Value]) -> Value {
        let host_ctx = module.host_ctx;
        let mut stack: Vec<Frame> = vec![make_frame(module, module.entry, args)];
        loop {
            let top = stack.len() - 1;
            let (func_idx, pc) = (stack[top].func, stack[top].pc);
            let inst = module.funcs[func_idx].code[pc]; // Inst: Copy
            let op = isa.op(inst.op);
            let mut next = pc + 1;
            let mut transferred = false;
            for eff in &op.effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        let dst = inst.operand(*reg) as usize;
                        let v = eval(value, &inst, &stack[top].locals, host_ctx);
                        stack[top].locals[dst] = v;
                    }
                    Effect::JumpTo { target } => next = inst.operand(*target) as usize,
                    Effect::JumpIf { cond, target } => {
                        if eval(cond, &inst, &stack[top].locals, host_ctx) != 0 {
                            next = inst.operand(*target) as usize;
                        }
                    }
                    Effect::Halt { value } => return eval(value, &inst, &stack[top].locals, host_ctx),
                    Effect::Guard { .. } => {}
                    Effect::Call { dest, base, argc } => {
                        let dest_slot = inst.operand(*dest) as usize;
                        let base_slot = inst.operand(*base) as usize;
                        let n = inst.operand(*argc) as usize;
                        // Full-width conversion BEFORE the index is used (A-C1, `callee_index`'s
                        // doc): an out-of-range word stays out of range on every target width and
                        // hits `make_frame`'s documented out-of-range panic — never a truncated
                        // silent call into `funcs[callee mod 2^32]`.
                        let callee = callee_index(stack[top].locals[base_slot]);
                        let mut argv = Vec::with_capacity(n);
                        for k in 0..n {
                            argv.push(stack[top].locals[base_slot + 1 + k]);
                        }
                        stack[top].pc = next; // resume the caller after the call returns
                        let mut frame = make_frame(module, callee, &argv);
                        frame.ret_dest = dest_slot;
                        stack.push(frame);
                        transferred = true;
                        break;
                    }
                    Effect::TailCall { base, argc } => {
                        // Replace the current frame (don't push): unbounded tail recursion stays bounded.
                        // Snapshot callee + args FIRST (they live in the frame we're about to overwrite),
                        // rebuild via `make_frame` (so missing params / consts / temps are `fill`-correct
                        // for the CALLEE, not carried over), and PRESERVE the outgoing frame's `ret_dest`
                        // (the callee's result must return to THIS frame's original caller).
                        let base_slot = inst.operand(*base) as usize;
                        let n = inst.operand(*argc) as usize;
                        // Same full-width conversion as `Call` above (A-C1).
                        let callee = callee_index(stack[top].locals[base_slot]);
                        let mut argv = Vec::with_capacity(n);
                        for k in 0..n {
                            argv.push(stack[top].locals[base_slot + 1 + k]);
                        }
                        let ret_dest = stack[top].ret_dest;
                        let mut frame = make_frame(module, callee, &argv);
                        frame.ret_dest = ret_dest;
                        stack[top] = frame;
                        transferred = true;
                        break;
                    }
                    Effect::Return { value } => {
                        let result = eval(value, &inst, &stack[top].locals, host_ctx);
                        let frame = stack.pop().unwrap();
                        match stack.last_mut() {
                            Some(caller) => caller.locals[frame.ret_dest] = result,
                            None => return result,
                        }
                        transferred = true;
                        break;
                    }
                }
            }
            if !transferred {
                stack[top].pc = next;
            }
        }
    }

    /// Convert a callee word — the raw i64 in `l(base)` that [`Effect::Call`]/[`Effect::TailCall`]
    /// name their target function with — into a dispatch index, checking the FULL 64-bit value
    /// BEFORE any narrowing cast (O-14 design §3.2a, increment A-C1). The word is reinterpreted as
    /// unsigned (two's complement — a negative word becomes a huge u64, exactly what `as usize`
    /// already produced on 64-bit targets) and then width-checked: a value that does not fit
    /// `usize` cannot possibly index a function store on this target, so it saturates to
    /// `usize::MAX` — an index that is itself ALWAYS out of range (a function store of
    /// `usize::MAX` entries cannot exist: every entry is wider than one byte, so it could not fit
    /// the address space) — routing the word into the caller's EXISTING out-of-range path (the
    /// engine dispatch loop's fail-closed `fill` fallback, or [`run_module`]'s documented
    /// out-of-range panic on the reference tier). On 64-bit targets this is bit-for-bit the old
    /// `word as usize` (the `try_from` never fails there) — observably unchanged; on 32-bit
    /// targets (the wasm32 deployment) it removes the silent pre-check truncation branch where a
    /// word `x >= 2^32` with `x mod 2^32 < nfuncs` called `f_{x mod 2^32}` instead of falling
    /// back. Every tier-side callee conversion MUST go through this one function (no-dual-source
    /// rule): `run_module`/`run_one` here, `module_dispatch`/`tail_signal` in `otter-engine`.
    #[inline]
    pub fn callee_index(word: Value) -> usize {
        usize::try_from(word as u64).unwrap_or(usize::MAX)
    }

    /// Out-parameter for [`run_one`]: a proper **tail call** fills this and returns (rather than
    /// recursing through `dispatch`), so the caller can re-enter the callee on a flat loop — bounded
    /// native stack. Reuse the SAME `TailReq` across trampoline iterations: its `args` buffer is reused
    /// (`clear` + push), so a tail-recursive loop does not allocate per iteration. `pending` is the sole
    /// tail-vs-return discriminator (never the returned value, which may legitimately be `fill`).
    #[derive(Default, Debug)]
    pub struct TailReq {
        pub pending: bool,
        pub callee: usize,
        pub args: Vec<Value>,
    }

    /// Run a **single function** to completion, delegating every `Call` to `dispatch(callee, args)`.
    /// This is the interpreter (T0) building block of the **adaptive module engine**: unlike
    /// [`run_module`] (which owns the whole call stack), each callee here is dispatched externally, so a
    /// callee may run at a *different* tier (compiled or interpreted). `args` initialize the parameters
    /// (missing → `fill`); `consts` initialize the slots after them; the rest start at `fill`. `host_ctx`
    /// is the word threaded to any [`Expr::HostCtx1`]/[`Expr::HostCtx2`] in `func` — pass
    /// [`Module::host_ctx`] (a `ModuleEngine`-style caller owns exactly one `Module`, so every function it
    /// runs — original or `push_func`-injected — shares that same word). Returns the function's
    /// `Return`/`Halt` value. A `TailCall` fills `tail` (`pending=true`) and returns `fill` WITHOUT
    /// recursing — the caller's trampoline re-enters the callee. Panics on a malformed function — fine
    /// for the reference tier.
    pub fn run_one(
        isa: &Isa,
        func: &FuncDef,
        args: &[Value],
        fill: Value,
        host_ctx: Value,
        dispatch: &mut dyn FnMut(usize, &[Value]) -> Value,
        tail: &mut TailReq,
    ) -> Value {
        let mut locals = vec![fill; func.nlocals];
        for (i, &a) in args.iter().take(func.nparams).enumerate() {
            locals[i] = a;
        }
        for (i, &c) in func.consts.iter().enumerate() {
            locals[func.nparams + i] = c;
        }
        let mut pc = 0usize;
        loop {
            let inst = func.code[pc];
            let op = isa.op(inst.op);
            let mut next = pc + 1;
            for eff in &op.effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        let dst = inst.operand(*reg) as usize;
                        locals[dst] = eval(value, &inst, &locals, host_ctx);
                    }
                    Effect::JumpTo { target } => next = inst.operand(*target) as usize,
                    Effect::JumpIf { cond, target } => {
                        if eval(cond, &inst, &locals, host_ctx) != 0 {
                            next = inst.operand(*target) as usize;
                        }
                    }
                    Effect::Halt { value } | Effect::Return { value } => {
                        return eval(value, &inst, &locals, host_ctx);
                    }
                    Effect::Guard { .. } => {}
                    Effect::Call { dest, base, argc } => {
                        let dest_slot = inst.operand(*dest) as usize;
                        let base_slot = inst.operand(*base) as usize;
                        let n = inst.operand(*argc) as usize;
                        // Full-width conversion BEFORE `dispatch` sees the index (A-C1,
                        // `callee_index`'s doc): the engine's loop-head range check then takes its
                        // existing fail-closed `fill` path for the whole 64-bit OOB space — no
                        // 32-bit target can truncate the word into range first.
                        let callee = callee_index(locals[base_slot]);
                        let mut argv = Vec::with_capacity(n);
                        for k in 0..n {
                            argv.push(locals[base_slot + 1 + k]);
                        }
                        locals[dest_slot] = dispatch(callee, &argv);
                    }
                    Effect::TailCall { base, argc } => {
                        // Signal-and-return: fill `tail` and return (NEVER `dispatch(callee)` — that would
                        // recurse and grow the native stack, defeating proper tail calls). The caller's
                        // trampoline re-enters the callee. Reuse `tail.args` (clear + push, no realloc).
                        let base_slot = inst.operand(*base) as usize;
                        let n = inst.operand(*argc) as usize;
                        // Same full-width conversion as `Call` above (A-C1) — the trampoline's
                        // loop-head range check consumes `tail.callee` on its next iteration.
                        tail.callee = callee_index(locals[base_slot]);
                        tail.args.clear();
                        for k in 0..n {
                            tail.args.push(locals[base_slot + 1 + k]);
                        }
                        tail.pending = true;
                        return fill;
                    }
                }
            }
            pc = next;
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// A sample register ISA defined purely as data — the first "client" of the framework. The SAME table
// is consumed by the interpreter (here) and the Cranelift JIT (backend crate).
// ---------------------------------------------------------------------------------------------------

pub mod sample {
    use super::{ArithOp, BinOp, Effect, Expr, Inst, Isa, NumTy, OpDef, OpHints, UnOp};
    use alloc::boxed::Box;
    use alloc::vec;

    // control / utility (type-agnostic)
    pub const MOV_IMM: u16 = 0; // local[op0] = sign-extended i32 immediate op1
    pub const JUMP_IF: u16 = 5; // if local[op0] != 0 goto op1
    pub const JUMP: u16 = 6; // goto op0
    pub const HALT: u16 = 7; // return local[op0]
    pub const GUARD: u16 = 8; // speculate local[op0] < local[op1] (i64); deopt if not
    pub const COPY: u16 = 9; // local[op0] = local[op1]
    // i64 integer arithmetic
    pub const ADD: u16 = 1;
    pub const SUB: u16 = 2;
    pub const MUL: u16 = 3;
    pub const LT: u16 = 4; // signed i64 <
    // f64
    pub const FADD: u16 = 10;
    pub const FSUB: u16 = 11;
    pub const FMUL: u16 = 12;
    pub const FDIV: u16 = 13;
    pub const FLT: u16 = 14;
    pub const I2F: u16 = 15; // convert i64 -> f64
    pub const F2I: u16 = 16; // convert f64 -> i64 (saturating)
    // width family (B1.2-wide+): representative ops exercising narrow widths, unsigned, and f32
    pub const I8_ADD: u16 = 17; // signed 8-bit wrapping add
    pub const U8_ADD: u16 = 18; // unsigned 8-bit wrapping add
    pub const I32_ADD: u16 = 19;
    pub const U32_ADD: u16 = 20;
    pub const U64_LT: u16 = 21; // unsigned 64-bit <
    pub const F32_ADD: u16 = 22;
    pub const F32_MUL: u16 = 23;
    pub const F32_LT: u16 = 24;
    pub const CONV_I64_F32: u16 = 25;
    pub const CONV_F32_F64: u16 = 26;
    pub const CONV_F64_U8: u16 = 27; // saturating f64 -> u8
    pub const CONV_U8_I64: u16 = 28; // zero-extend u8 -> i64
    pub const F16_ADD: u16 = 29; // half-precision add (round to f16)
    pub const F16_MUL: u16 = 30;
    pub const CONV_F64_F16: u16 = 31; // round f64 -> f16
    pub const CONV_F16_F64: u16 = 32; // widen f16 -> f64
    pub const CONV_U64_F64: u16 = 33; // unsigned u64 -> f64
    pub const FNEG_F64: u16 = 34; // local[op0] = -local[op1] (f64)
    pub const FEQ_F64: u16 = 35; // local[op0] = (local[op1] == local[op2]) as bool-word (ordered f64)

    fn alu(name: &'static str, op: BinOp) -> OpDef {
        OpDef {
            name,
            effects: vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::Bin(op, Box::new(Expr::LocalAt(1)), Box::new(Expr::LocalAt(2))),
            }],
            hints: OpHints {
                pure: true,
                terminator: false,
            },
        }
    }

    fn unary(name: &'static str, op: UnOp) -> OpDef {
        OpDef {
            name,
            effects: vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::Un(op, Box::new(Expr::LocalAt(1))),
            }],
            hints: OpHints {
                pure: true,
                terminator: false,
            },
        }
    }

    /// Build the sample ISA. Opcode order MUST match the `*_*` constants above.
    pub fn isa() -> Isa {
        Isa {
            ops: vec![
                OpDef {
                    name: "mov_imm",
                    effects: vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::OperandImm(1),
                    }],
                    hints: OpHints {
                        pure: true,
                        terminator: false,
                    },
                },
                alu("add", BinOp::new(NumTy::I64, ArithOp::Add)),
                alu("sub", BinOp::new(NumTy::I64, ArithOp::Sub)),
                alu("mul", BinOp::new(NumTy::I64, ArithOp::Mul)),
                alu("lt", BinOp::new(NumTy::I64, ArithOp::Lt)),
                OpDef {
                    name: "jump_if",
                    effects: vec![Effect::JumpIf {
                        cond: Expr::LocalAt(0),
                        target: 1,
                    }],
                    hints: OpHints {
                        pure: false,
                        terminator: true,
                    },
                },
                OpDef {
                    name: "jump",
                    effects: vec![Effect::JumpTo { target: 0 }],
                    hints: OpHints {
                        pure: false,
                        terminator: true,
                    },
                },
                OpDef {
                    name: "halt",
                    effects: vec![Effect::Halt {
                        value: Expr::LocalAt(0),
                    }],
                    hints: OpHints {
                        pure: false,
                        terminator: true,
                    },
                },
                OpDef {
                    name: "guard",
                    // speculate local[op0] < local[op1]; the optimizing tier deopts if it fails
                    effects: vec![Effect::Guard {
                        cond: Expr::Bin(
                            BinOp::new(NumTy::I64, ArithOp::Lt),
                            Box::new(Expr::LocalAt(0)),
                            Box::new(Expr::LocalAt(1)),
                        ),
                    }],
                    hints: OpHints {
                        pure: false,
                        terminator: false,
                    },
                },
                OpDef {
                    name: "copy",
                    effects: vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::LocalAt(1),
                    }],
                    hints: OpHints {
                        pure: true,
                        terminator: false,
                    },
                },
                alu("fadd", BinOp::new(NumTy::F64, ArithOp::Add)),
                alu("fsub", BinOp::new(NumTy::F64, ArithOp::Sub)),
                alu("fmul", BinOp::new(NumTy::F64, ArithOp::Mul)),
                alu("fdiv", BinOp::new(NumTy::F64, ArithOp::Div)),
                alu("flt", BinOp::new(NumTy::F64, ArithOp::Lt)),
                unary(
                    "i2f",
                    UnOp::Convert {
                        from: NumTy::I64,
                        to: NumTy::F64,
                    },
                ),
                unary(
                    "f2i",
                    UnOp::Convert {
                        from: NumTy::F64,
                        to: NumTy::I64,
                    },
                ),
                // --- width family (opcodes 17..) ---
                alu("i8.add", BinOp::new(NumTy::I8, ArithOp::Add)),
                alu("u8.add", BinOp::new(NumTy::U8, ArithOp::Add)),
                alu("i32.add", BinOp::new(NumTy::I32, ArithOp::Add)),
                alu("u32.add", BinOp::new(NumTy::U32, ArithOp::Add)),
                alu("u64.lt", BinOp::new(NumTy::U64, ArithOp::Lt)),
                alu("f32.add", BinOp::new(NumTy::F32, ArithOp::Add)),
                alu("f32.mul", BinOp::new(NumTy::F32, ArithOp::Mul)),
                alu("f32.lt", BinOp::new(NumTy::F32, ArithOp::Lt)),
                unary(
                    "conv.i64_f32",
                    UnOp::Convert {
                        from: NumTy::I64,
                        to: NumTy::F32,
                    },
                ),
                unary(
                    "conv.f32_f64",
                    UnOp::Convert {
                        from: NumTy::F32,
                        to: NumTy::F64,
                    },
                ),
                unary(
                    "conv.f64_u8",
                    UnOp::Convert {
                        from: NumTy::F64,
                        to: NumTy::U8,
                    },
                ),
                unary(
                    "conv.u8_i64",
                    UnOp::Convert {
                        from: NumTy::U8,
                        to: NumTy::I64,
                    },
                ),
                alu("f16.add", BinOp::new(NumTy::F16, ArithOp::Add)),
                alu("f16.mul", BinOp::new(NumTy::F16, ArithOp::Mul)),
                unary(
                    "conv.f64_f16",
                    UnOp::Convert {
                        from: NumTy::F64,
                        to: NumTy::F16,
                    },
                ),
                unary(
                    "conv.f16_f64",
                    UnOp::Convert {
                        from: NumTy::F16,
                        to: NumTy::F64,
                    },
                ),
                unary(
                    "conv.u64_f64",
                    UnOp::Convert {
                        from: NumTy::U64,
                        to: NumTy::F64,
                    },
                ),
                unary("fneg.f64", UnOp::Neg(NumTy::F64)),
                alu("feq.f64", BinOp::new(NumTy::F64, ArithOp::Eq)),
            ],
        }
    }

    /// `sum 1..=n` (= n(n+1)/2). Registers: 0=acc, 1=i, 2=n, 3=cond, 4=one.
    pub fn sum_program(n: i32) -> alloc::vec::Vec<Inst> {
        vec![
            Inst::new(MOV_IMM, 0, 0, 0), // acc = 0
            Inst::new(MOV_IMM, 1, 1, 0), // i = 1
            Inst::new(MOV_IMM, 2, n, 0), // n
            Inst::new(MOV_IMM, 4, 1, 0), // one = 1
            Inst::new(LT, 3, 2, 1),      // 4: cond = (n < i)
            Inst::new(JUMP_IF, 3, 9, 0), // 5: if cond goto 9
            Inst::new(ADD, 0, 0, 1),     // 6: acc += i
            Inst::new(ADD, 1, 1, 4),     // 7: i += 1
            Inst::new(JUMP, 4, 0, 0),    // 8: goto 4
            Inst::new(HALT, 0, 0, 0),    // 9: return acc
        ]
    }

    /// `sum 1..=n` where **n is argument 0** (a parameterized function, for tier-up). `nargs = 1`,
    /// `nlocals = 5`. Registers: 0=n(arg), 1=acc, 2=i, 3=cond, 4=one.
    pub fn sum_to_program() -> alloc::vec::Vec<Inst> {
        vec![
            Inst::new(MOV_IMM, 1, 0, 0), // acc = 0
            Inst::new(MOV_IMM, 2, 1, 0), // i = 1
            Inst::new(MOV_IMM, 4, 1, 0), // one = 1
            Inst::new(LT, 3, 0, 2),      // 3: cond = (n < i)
            Inst::new(JUMP_IF, 3, 8, 0), // 4: if cond goto 8
            Inst::new(ADD, 1, 1, 2),     // 5: acc += i
            Inst::new(ADD, 2, 2, 4),     // 6: i += 1
            Inst::new(JUMP, 3, 0, 0),    // 7: goto 3
            Inst::new(HALT, 1, 0, 0),    // 8: return acc
        ]
    }

    /// `sum 1..=n` (n = arg 0) with a `GUARD i < limit` in the loop body — for exercising **deopt**: the
    /// optimizing tier speculates `i < limit` and bails to the interpreter once `i` reaches `limit`
    /// (which happens iff `n >= limit`). The result is `n(n+1)/2` regardless (the guard is transparent).
    /// `nargs = 1`, `nlocals = 6`. Registers: 0=n(arg), 1=acc, 2=i, 3=cond, 4=one, 5=limit.
    pub fn sum_to_guarded_program(limit: i32) -> alloc::vec::Vec<Inst> {
        vec![
            Inst::new(MOV_IMM, 1, 0, 0),     // acc = 0
            Inst::new(MOV_IMM, 2, 1, 0),     // i = 1
            Inst::new(MOV_IMM, 4, 1, 0),     // one = 1
            Inst::new(MOV_IMM, 5, limit, 0), // limit
            Inst::new(LT, 3, 0, 2),          // 4: cond = (n < i)   [loop header]
            Inst::new(JUMP_IF, 3, 10, 0),    // 5: if cond goto 10
            Inst::new(GUARD, 2, 5, 0),       // 6: speculate i < limit (deopt if i >= limit)
            Inst::new(ADD, 1, 1, 2),         // 7: acc += i
            Inst::new(ADD, 2, 2, 4),         // 8: i += 1
            Inst::new(JUMP, 4, 0, 0),        // 9: goto 4
            Inst::new(HALT, 1, 0, 0),        // 10: return acc
        ]
    }

    /// `n!` for n >= 0. Registers: 0=acc, 1=i, 2=one, 3=cond.
    pub fn factorial_program(n: i32) -> alloc::vec::Vec<Inst> {
        vec![
            Inst::new(MOV_IMM, 0, 1, 0), // acc = 1
            Inst::new(MOV_IMM, 1, n, 0), // i = n
            Inst::new(MOV_IMM, 2, 1, 0), // one = 1
            Inst::new(LT, 3, 1, 2),      // 3: cond = (i < 1)
            Inst::new(JUMP_IF, 3, 8, 0), // 4: if cond goto 8
            Inst::new(MUL, 0, 0, 1),     // 5: acc *= i
            Inst::new(SUB, 1, 1, 2),     // 6: i -= 1
            Inst::new(JUMP, 3, 0, 0),    // 7: goto 3
            Inst::new(HALT, 0, 0, 0),    // 8: return acc
        ]
    }

    /// `(x + y) * x` over **floating-point** arguments (x = arg0, y = arg1, both as `f64` bit-words).
    /// Returns an `f64` bit-word. Registers: 0=x, 1=y, 2=tmp. `nargs = 2`, `nlocals = 3`.
    pub fn float_expr_program() -> alloc::vec::Vec<Inst> {
        vec![
            Inst::new(FADD, 2, 0, 1), // tmp = x + y
            Inst::new(FMUL, 2, 2, 0), // tmp = tmp * x
            Inst::new(HALT, 2, 0, 0), // return tmp
        ]
    }

    /// `(i64)((f64)n + (f64)n)` = `2n` — exercises both conversions and a float op between them.
    /// `n` = arg0 (i64). Registers: 0=n, 1=acc. `nargs = 1`, `nlocals = 2`.
    pub fn convert_roundtrip_program() -> alloc::vec::Vec<Inst> {
        vec![
            Inst::new(I2F, 1, 0, 0),  // acc = (f64) n
            Inst::new(FADD, 1, 1, 1), // acc = acc + acc  (= 2n as f64)
            Inst::new(F2I, 1, 1, 0),  // acc = (i64) acc  (truncate toward zero)
            Inst::new(HALT, 1, 0, 0),
        ]
    }
}

// ---------------------------------------------------------------------------------------------------
// Hint-driven optimizer — the THIRD consumer of the single source (after interpreter and JIT).
// ---------------------------------------------------------------------------------------------------

pub mod opt {
    //! Generic, hint-driven constant propagation + folding over an [`Isa`]-defined program.
    //!
    //! Nothing here is hardcoded to a specific opcode — it is driven by the [`OpHints`](super::OpHints)
    //! (`pure`, `terminator`) and the reifiable IR:
    //!   * a `pure` op with a single `SetLocalAt` whose value evaluates to a constant (block-locally) is
    //!     folded at compile time using [`BinOp::eval`](super::BinOp::eval) — the SAME wrapping
    //!     arithmetic the interpreter and JIT use, so the folded constant is bit-identical to the
    //!     runtime result;
    //!   * the "load immediate into a register" op is *discovered* from the ISA by its IR signature
    //!     (`SetLocalAt { value: OperandImm(_) }`), not assumed — so folding works for any ISA that has
    //!     such an op.
    //!
    //! **This function does NOT bounds-check its own indexing.** `eval_const`'s `consts[inst.operand(*i)
    //! as usize]` (driven by `Expr::LocalAt`, exactly like the interpreter's `eval`) and the
    //! `SetLocalAt`/`Call` destination writes below it panic on the same malformed-operand shapes
    //! `otter::interp` does, given a `prog` an attacker (not the trusted `Isa` author) controls — see the
    //! O-12 design doc §1.1/§1.2 and panel Lens A finding #1, which is what added this paragraph.
    //! `constant_fold` was previously undocumented and unlisted in O-12's own panic-site inventory despite
    //! walking the identical `Effect::SetLocalAt`/`Expr::LocalAt` shapes `check_func` validates; it is
    //! currently reachable only from trusted, `#[cfg(test)]`-only callers, so this is not (yet) a live
    //! gap, but any future caller that feeds `constant_fold` untrusted `prog`/`nlocals` MUST validate first
    //! — `otter::validate::check_func(isa, &FuncDef { nlocals, code: prog.to_vec(), .. })`'s
    //! `LocalOutOfRange` check structurally covers this function's indexing too (same operand-extraction
    //! shapes), even though `check_func` was written against `interp`'s sites, not this one.
    //!
    //! It also **algebraically simplifies** using the primitive hint vocabulary
    //! ([`BinOp::absorbing_element`](super::BinOp::absorbing_element),
    //! [`BinOp::right_identity`](super::BinOp::right_identity),
    //! [`BinOp::is_commutative`](super::BinOp::is_commutative)) — `x*0 → 0` (even when `x` is unknown)
    //! and `x+0 / x*1 / x-0 → x` (via a *discovered* register-copy op) — which is strictly stronger than
    //! constant folding. Both the materializer and the copy op are discovered from the ISA by IR
    //! signature, so this works for any ISA that provides them.
    //!
    //! Soundness: the constant lattice is reset to all-unknown at every basic-block leader (a join point
    //! is treated conservatively), and a folded constant is only *materialized* if it fits the immediate
    //! width (`i32`); otherwise the original instruction is kept (the constant is still tracked). The
    //! rewrite is **length-preserving** (one instruction → one instruction) so pc indices and branch
    //! targets stay valid — no renumbering.

    use super::{Effect, Expr, Inst, Isa, NOPERANDS, Value};
    use alloc::vec;
    use alloc::vec::Vec;

    /// The discovered "set register := immediate" op: opcode + which operand slots carry the dest
    /// register and the immediate value.
    #[derive(Clone, Copy)]
    struct Materializer {
        op: u16,
        reg_slot: u8,
        val_slot: u8,
    }

    fn find_materializer(isa: &Isa) -> Option<Materializer> {
        for (i, op) in isa.ops.iter().enumerate() {
            if op.effects.len() == 1
                && let Effect::SetLocalAt {
                    reg,
                    value: Expr::OperandImm(v),
                } = &op.effects[0]
            {
                return Some(Materializer {
                    op: i as u16,
                    reg_slot: *reg,
                    val_slot: *v,
                });
            }
        }
        None
    }

    /// The discovered "set register := another register" (copy) op: opcode + dest/src operand slots.
    #[derive(Clone, Copy)]
    struct Copy {
        op: u16,
        dst_slot: u8,
        src_slot: u8,
    }

    fn find_copy(isa: &Isa) -> Option<Copy> {
        for (i, op) in isa.ops.iter().enumerate() {
            if op.effects.len() == 1
                && let Effect::SetLocalAt {
                    reg,
                    value: Expr::LocalAt(v),
                } = &op.effects[0]
            {
                return Some(Copy {
                    op: i as u16,
                    dst_slot: *reg,
                    src_slot: *v,
                });
            }
        }
        None
    }

    fn make_inst(op: u16, slot_a: u8, val_a: i32, slot_b: u8, val_b: i32) -> Inst {
        let mut operands = [0i32; NOPERANDS];
        operands[slot_a as usize] = val_a;
        operands[slot_b as usize] = val_b;
        Inst { op, operands }
    }

    /// Evaluate `e` to a constant given the current per-local constant lattice, or `None` if any input
    /// is unknown. Uses `BinOp::eval` so folding matches runtime semantics exactly (incl. wrapping).
    fn eval_const(e: &Expr, inst: &Inst, consts: &[Option<Value>]) -> Option<Value> {
        match e {
            Expr::Imm(v) => Some(*v),
            Expr::ImmF64(f) => Some(super::from_f64(*f)),
            Expr::OperandImm(i) => Some(inst.operand(*i) as Value),
            Expr::LocalAt(i) => consts[inst.operand(*i) as usize],
            Expr::Bin(op, a, b) => Some(op.eval(eval_const(a, inst, consts)?, eval_const(b, inst, consts)?)),
            Expr::Un(op, a) => Some(op.eval(eval_const(a, inst, consts)?)),
            // host calls are opaque to the optimizer (assume side-effecting / unknown) — HostCtx1/2 too,
            // even though `host_ctx` is itself a compile-time constant: O-11 scope keeps them uniformly
            // opaque like Host1/Host2 rather than opening a narrower "constant-ctx-only" fold (see the
            // O-11 design doc §7 risk 2).
            Expr::Host1(..) | Expr::Host2(..) | Expr::HostCtx1(..) | Expr::HostCtx2(..) => None,
        }
    }

    /// Basic-block leaders, derived generically from the `terminator` hint + control effects (same rule
    /// the JIT uses). Resetting the lattice at each leader keeps the analysis sound across joins.
    fn leaders(isa: &Isa, prog: &[Inst]) -> Vec<bool> {
        let mut leader = vec![false; prog.len()];
        if !prog.is_empty() {
            leader[0] = true;
        }
        for (pc, inst) in prog.iter().enumerate() {
            if isa.op(inst.op).hints.terminator {
                for e in &isa.op(inst.op).effects {
                    if let Effect::JumpTo { target } | Effect::JumpIf { target, .. } = e {
                        let t = inst.operand(*target) as usize;
                        if t < leader.len() {
                            leader[t] = true;
                        }
                    }
                }
                if pc + 1 < leader.len() {
                    leader[pc + 1] = true;
                }
            }
        }
        leader
    }

    /// Materialize constant `c` into register `dest` if it fits the immediate width — returns the rewrite.
    fn materialize(mat: Option<Materializer>, dest: usize, c: Value, cur: Inst) -> Option<Inst> {
        let m = mat?;
        let c32 = i32::try_from(c).ok()?;
        let new = make_inst(m.op, m.reg_slot, dest as i32, m.val_slot, c32);
        if new != cur { Some(new) } else { None }
    }

    /// Constant-fold + **algebraic-simplify** `prog` under `isa`. Returns a new, length-equal program.
    ///
    /// Algebraic simplification uses the primitive hints ([`BinOp::absorbing_element`],
    /// [`BinOp::right_identity`], [`BinOp::is_commutative`]) to simplify even when an operand is *not*
    /// statically known: `x*0 → 0` (absorbing, materialized) and `x+0 / x*1 / x-0 → x` (identity, via a
    /// discovered copy op). Strictly stronger than constant folding alone.
    pub fn constant_fold(isa: &Isa, prog: &[Inst], nlocals: usize) -> Vec<Inst> {
        let leader = leaders(isa, prog);
        let mat = find_materializer(isa);
        let copy = find_copy(isa);
        let mut out = prog.to_vec();
        let mut consts: Vec<Option<Value>> = vec![None; nlocals];

        for pc in 0..prog.len() {
            if leader[pc] {
                consts.iter_mut().for_each(|c| *c = None); // join point: forget constants
            }
            let inst = out[pc];
            let op = isa.op(inst.op);

            if op.hints.pure
                && op.effects.len() == 1
                && let Effect::SetLocalAt { reg, value } = &op.effects[0]
            {
                let dest = inst.operand(*reg) as usize;

                // (1) full constant fold
                if let Some(c) = eval_const(value, &inst, &consts) {
                    if let Some(new) = materialize(mat, dest, c, inst) {
                        out[pc] = new;
                    }
                    consts[dest] = Some(c);
                    continue;
                }

                // (2) algebraic simplification of `dest = a OP b` (one operand unknown)
                if let Expr::Bin(bop, ea, eb) = value
                    && let (Expr::LocalAt(sa), Expr::LocalAt(sb)) = (ea.as_ref(), eb.as_ref())
                {
                    let (ra, rb) = (inst.operand(*sa) as usize, inst.operand(*sb) as usize);
                    let (ca, cb) = (consts[ra], consts[rb]);

                    // x * 0 = 0 (absorbing) — result known even though the other operand is not
                    if let Some(e) = bop.absorbing_element()
                        && (ca == Some(e) || cb == Some(e))
                    {
                        if let Some(new) = materialize(mat, dest, e, inst) {
                            out[pc] = new;
                        }
                        consts[dest] = Some(e);
                        continue;
                    }
                    // x OP r = x (identity): right operand is the identity → dest = local[ra]
                    if let Some(r) = bop.right_identity() {
                        if cb == Some(r) {
                            if let Some(cp) = copy {
                                out[pc] = make_inst(cp.op, cp.dst_slot, dest as i32, cp.src_slot, ra as i32);
                            }
                            consts[dest] = ca;
                            continue;
                        }
                        // commutative: left operand is the identity → dest = local[rb]
                        if bop.is_commutative() && ca == Some(r) {
                            if let Some(cp) = copy {
                                out[pc] = make_inst(cp.op, cp.dst_slot, dest as i32, cp.src_slot, rb as i32);
                            }
                            consts[dest] = cb;
                            continue;
                        }
                    }
                }
            }

            // General lattice update: each write becomes its constant value, or unknown.
            for e in &op.effects {
                if let Effect::SetLocalAt { reg, value } = e {
                    let dest = inst.operand(*reg) as usize;
                    consts[dest] = eval_const(value, &inst, &consts);
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------
// Optional static validator (O-12) — proves a FuncDef/Module cannot trigger the indexing-class panics
// (and, for otter-baseline, worse: panic-less memory corruption) that every tier's hot path takes on
// faith. NOT wired into any existing API automatically — see the module doc for why and how to use it.
// ---------------------------------------------------------------------------------------------------

pub mod validate {
    //! Static, allocation-free, single-pass validation that a [`FuncDef`]/[`Module`] cannot trigger the
    //! *indexing*-class panics (out-of-range opcode/local/jump-target/call-slot) every tier's hot path
    //! takes on faith — [`Isa::op`], [`interp::eval`]'s `LocalAt`, `interp::run_module`'s `prog[pc]`/
    //! `locals[..]`, `otter-baseline`'s hand-computed `[rbp+disp32]` stack offsets (which, worse than a
    //! panic, silently corrupt caller-frame stack memory in a release build if left unchecked — see
    //! `.claude-research-library`'s O-12 design doc §1.4), and `otter-cranelift`'s `vars[..]`/
    //! `block_at[&target]` — see the O-12 design doc §1 for the full inventory this module closes (and
    //! §7/§8 for exactly what it does not).
    //!
    //! **Not automatically wired in anywhere.** `ModuleEngine::push_func`/`ModuleEngine::new` (in the
    //! `otter-engine` crate) keep their existing, already panel-reviewed (O-9/O-10) signatures rather than
    //! becoming fallible — a deliberate, narrow scope decision, not an oversight (O-12 design doc
    //! §3.2/§3.4: making that call is left to a dedicated future panel review, not decided unilaterally
    //! here). **If you are about to hand a `FuncDef`/`Module` you did not fully control the construction
    //! of — bytecode from an untrusted compiler, a fuzzer, a REPL, something deserialized from disk or the
    //! network — to any tier, call [`check_func`]/[`check_module`] on it first and reject the input on
    //! `Err` instead of executing it.**
    //!
    //! # What this proves, and how
    //! One linear pass over `f.code`; no evaluation, no allocation, no recursion into loops (only into an
    //! individual instruction's small, ISA-FIXED [`Expr`] tree — see "Isa is trusted" below for why that
    //! recursion depth is not attacker-controlled). For every instruction it resolves the opcode against
    //! `isa` and, for every [`Effect`]/[`Expr`] operand ANY tier later uses as a raw array/`HashMap` index
    //! — [`Effect::SetLocalAt`]/[`Effect::Call`]'s `dest`, [`Expr::LocalAt`], jump `target`s,
    //! [`Effect::Call`]/[`Effect::TailCall`]'s `base..base+1+argc` block — proves it is in range *for this
    //! specific `f`* (its own `nlocals`/`code.len()`), using sign-checked-then-widened arithmetic so the
    //! proof itself cannot be fooled by the same integer-overflow trick it exists to catch (a negative or
    //! near-`i32::MAX` operand can never slip through as "in range" — see [`local_ok`]/[`call_slot_ok`]/
    //! [`jump_target_ok`]). The `Effect`/`Expr` traversal is an EXHAUSTIVE `match` with **no wildcard
    //! arm**, on purpose: if `otter`'s IR ever grows a new [`Effect`]/[`Expr`] variant that carries an
    //! index-shaped operand, this module fails to *compile* — loudly, at the next opcode-vocabulary
    //! change — instead of silently under-validating it forever.
    //!
    //! Beyond the per-instruction operands above, [`check_func`] also proves a function-level (not
    //! per-instruction) invariant [`FuncDef`]'s own doc states but nothing previously checked: `nparams +
    //! consts.len() <= nlocals` (see [`ValidationError::FrameLayoutTooSmall`]) — every tier's frame-init
    //! code writes `f.consts` into the slots immediately after the `nparams` argument slots, unconditionally
    //! trusting this to fit inside `nlocals`. A `FuncDef` could otherwise pass every other check here (every
    //! operand it contains valid) while still describing a frame too small for its own declared shape.
    //!
    //! # What this does NOT prove — explicit non-goals (do not oversell this as a full verifier)
    //! - **The `callee` VALUE of a `Call`/`TailCall`** (which function index a call actually targets at
    //!   runtime) is not checked here — it is a runtime value read out of a local, not a static property
    //!   of `f.code`, and `otter_engine::ModuleEngine::dispatch`'s `in_range` check (or
    //!   [`interp::run_module`]'s `module.funcs[func_idx]`, on the un-engined oracle path) already handles
    //!   it at the layer that actually knows the value. What THIS module checks is the *static* shape of
    //!   the call site itself: is `base` a valid local, does the `[callee, arg0, ...]` block fit inside
    //!   `nlocals` (see [`ValidationError::CallSlotOutOfRange`]).
    //! - **Host function pointers** ([`Expr::Host1`]/[`Expr::Host2`]/[`Expr::HostCtx1`]/[`Expr::HostCtx2`])
    //!   are opaque `extern "C" fn` pointers baked into the [`Isa`] by whoever builds it. **`Isa` itself is
    //!   trusted throughout this module**, exactly like `interp`/`opt` already assume — an ISA-construction
    //!   bug (a dangling host fn pointer, an out-of-range operand-SLOT literal baked into an `OpDef`) is
    //!   out of scope for a checker whose entire premise is validating the untrusted part: the bytecode
    //!   (`FuncDef`/`Module`), not the trusted part (`Isa`).
    //! - **Cranelift's own internal verifier/panics** (O-12 design doc §1.5) are a third-party code surface
    //!   this module cannot see into, let alone replicate — `panic = "unwind"` + `catch_unwind` (the OTHER
    //!   half of O-12, enforced in `otter-engine`) is the only mechanism that contains that residual risk;
    //!   this module does not attempt to.
    //! - **Resource exhaustion** from a huge-but-technically-in-range `nlocals`/`code.len()` (a large
    //!   allocation, a slow compile) is a DIFFERENT risk class — availability via resource cost, not
    //!   memory-unsafety via a narrowing/wrapping cast — and is out of scope EXCEPT where the two overlap:
    //!   see [`ValidationError::FrameTooLarge`] for the (narrow, structural) overlap this module does
    //!   close.
    //! - **[`super::opt::constant_fold`]** is a THIRD consumer of the exact same `nlocals`-sized indexing
    //!   pattern (`consts[inst.operand(*i) as usize]`, driven by the identical `Effect::SetLocalAt`/
    //!   `Expr::LocalAt` shapes `check_func` walks) — [`check_func`]'s [`ValidationError::LocalOutOfRange`]
    //!   check structurally also proves `constant_fold` cannot index out of bounds on the SAME
    //!   `(isa, prog, nlocals)` triple, since both walk identical `Effect`/`Expr` shapes to extract the
    //!   identical index. This module does not call `constant_fold` itself, so that protection is only
    //!   real if the CALLER validates before folding — same "opt-in, not automatic" caveat as everywhere
    //!   else in this module (O-12 design doc, panel Lens A finding #1; see `constant_fold`'s own doc).

    use super::{Effect, Expr, FuncDef, Inst, Isa, Module};

    /// Why [`check_func`]/[`check_module`] rejected a [`FuncDef`]/[`Module`]. Each variant names the
    /// exact downstream panic-or-worse site it prevents — see the variant's own doc for which O-12
    /// design-doc finding (and, where applicable, which post-design panel finding) it closes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ValidationError {
        /// `f.code` is empty — every tier's entry point unconditionally indexes `code[0]`
        /// (`interp::interpret_from`'s `prog[pc]` at `pc=0`; `interp::run_module`/`run_one` identically).
        EmptyCode,
        /// `f.nlocals` exceeds [`MAX_SAFE_NLOCALS`]. Even before touching any per-instruction operand, an
        /// `nlocals` this large would let downstream per-tier frame-size arithmetic — `otter-baseline`'s
        /// `((nlocals + 3 + maxargc) * 8).div_ceil(16) as i32`, computed in BOTH `compile` and
        /// `compile_module_func` — narrow or wrap, turning `sub rsp, imm32` into an effective `add rsp,
        /// |imm32|`: a return-address-adjacent stack-smashing primitive with **no panic anywhere on the
        /// path** (O-12 design doc §1.4/§1.6; this specific narrowing-cast failure mode was identified by
        /// the O-12 panel, Lens A finding #2 — the design doc's own §1.6 only names the more general
        /// 64-bit wraparound case). Combined with [`ValidationError::CallSlotOutOfRange`] (which
        /// transitively bounds every call's `argc` at `<= nlocals`), this ceiling on `nlocals` ALONE is
        /// enough to also keep `otter-cranelift`'s per-call `(argc * 8) as u32` stack-slot size safely
        /// representable (panel Lens C finding #3) — see this field's derivation for why.
        FrameTooLarge { nlocals: usize, limit: usize },
        /// `pc`'s opcode is `>= isa.ops.len()` — the direct [`Isa::op`] malformed-opcode site (`&self.ops
        /// [opcode as usize]`), reached from every tier's dispatch loop.
        OpcodeOutOfRange { pc: usize, opcode: u16 },
        /// A local-index operand at `pc` is negative or `>= nlocals`. Covers EVERY operand any tier reads
        /// or writes as a plain local-array index: [`Effect::SetLocalAt`]'s `reg`, [`Expr::LocalAt`]
        /// (wherever it appears — including nested inside a [`Effect::JumpIf`]/[`Effect::Guard`] condition
        /// or an arithmetic [`Expr`] tree), [`Effect::Halt`]/[`Effect::Return`]'s value expression, and —
        /// per the O-12 panel review, Lens C finding #1, which the design doc's own §3.2 enum sketch
        /// missed — [`Effect::Call`]'s `dest` too: `interp::run_module`'s `caller.locals[frame.ret_dest]`,
        /// `interp::run_one`'s `locals[dest_slot]`, `otter-cranelift`'s `vars[dest_slot]`, and
        /// `otter-baseline`'s `disp(dest_slot)` are all the exact same "write a local by index" site
        /// `SetLocalAt` uses, so `Call.dest` gets the identical check here.
        LocalOutOfRange { pc: usize, local: i32, nlocals: usize },
        /// A jump target at `pc` ([`Effect::JumpTo`]/[`Effect::JumpIf`]) is negative or `>= code.len()` —
        /// every tier's `prog[pc]`/`func.code[pc]` site (T0), or the `block_at[&target]` `HashMap` lookup
        /// (T2, which panics on a missing key rather than corrupting memory — `leaders()`'s own bounds
        /// check on the `HashMap`'s insertion side is why that stays a panic, not silent misuse; see the
        /// O-12 design doc §1.2).
        JumpTargetOutOfRange { pc: usize, target: i32, code_len: usize },
        /// [`Effect::Call`]/[`Effect::TailCall`]'s `base`/`argc` don't describe a `[callee, arg0, ...
        /// arg(argc-1)]` block that fits inside `nlocals` — i.e. `base < 0 || argc < 0 ||
        /// base + 1 + argc > nlocals`. This is the read-side twin of `LocalOutOfRange`: every tier reads
        /// `locals[base]`(callee)/`locals[base+1..base+1+argc]`(args) exactly like a `LocalAt` chain
        /// (`interp::run_module`/`run_one`'s `stack[top].locals[base_slot + 1 + k]`; `otter-baseline`'s
        /// unchecked `disp(base_slot + 1 + k)` in BOTH the `Call` and `TailCall` argument-marshaling
        /// loops — the O-12 design doc's own §1.4 discovery named only the `LocalAt`/`SetLocalAt` sites;
        /// the panel, Lens A finding #3, pointed out this same unchecked-`disp()` vulnerability class also
        /// covers these two call-marshaling sites). Bounding `argc <= nlocals - 1 - base <= nlocals` here
        /// is also what makes [`ValidationError::FrameTooLarge`]'s `nlocals`-only ceiling sufficient to
        /// bound `otter-baseline`'s `maxargc`-dependent frame size and `otter-cranelift`'s per-call
        /// stack-slot size (panel Lens C finding #3) WITHOUT this module needing to separately track
        /// `maxargc` itself — see [`ValidationError::FrameTooLarge`]'s doc for the full argument. The O-12
        /// panel (Lens C finding #4) also flagged that this bound only holds `nlocals` itself is *also*
        /// bounded — which is exactly what `FrameTooLarge` is for; the two variants are a matched pair,
        /// not independently sufficient.
        CallSlotOutOfRange {
            pc: usize,
            base: i32,
            argc: i32,
            nlocals: usize,
        },
        /// [`Module::entry`] is `>= module.funcs.len()` — [`interp::run_module`]'s `make_frame`
        /// (`module.funcs[func_idx]`) has no guard of its own for this (O-12 design doc §1.2).
        EntryOutOfRange { entry: usize, nfuncs: usize },
        /// `f.nparams + f.consts.len() > f.nlocals` — violates the frame-layout invariant [`FuncDef`]'s own
        /// doc states (`[params(nparams)][consts(consts.len())][temporaries…]`, total `nlocals`), which
        /// EVERY tier's argument/const initialization assumes without checking: `interp::make_frame`'s
        /// `frame.locals[..f.nparams]`/`[f.nparams..f.nparams + f.consts.len()]` writes,
        /// `interp::run_one`'s identical pattern, `otter-cranelift`'s `vars[f.nparams + j]` prologue
        /// (`vars` is sized to exactly `f.nlocals`), and `otter-baseline`'s `disp(f.nparams + jc)` stores
        /// (which — same class as [`FrameTooLarge`]/[`CallSlotOutOfRange`] — corrupt stack memory with NO
        /// panic at all, rather than merely panicking, if `nlocals` is undersized for `nparams`/`consts`).
        /// Found by post-implementation review: none of the other variants bound this relationship — a
        /// `FuncDef` can pass every other check here and still be malformed in exactly the way this module
        /// exists to rule out.
        FrameLayoutTooSmall {
            nparams: usize,
            consts_len: usize,
            nlocals: usize,
        },
    }

    /// Conservative ceiling on `f.nlocals` ALONE (independent of any per-call `argc`, which
    /// [`ValidationError::CallSlotOutOfRange`] separately bounds `<= nlocals`), derived so that even the
    /// worst case downstream frame-slot count — locals, plus a same-sized worst-case call-argument
    /// buffer (`argc <= nlocals`, from `CallSlotOutOfRange`), plus a handful of small constant spill slots
    /// a backend reserves (`otter-baseline` reserves 3 today; this derivation does not hard-code that
    /// number, so it stays correct if a backend reserves more later) — times 8 bytes/slot, stays a small
    /// POSITIVE `i32` with a large safety margin, so the `as i32` cast baked into every backend's `sub
    /// rsp, imm32` prologue can never turn negative (O-12 design doc §1.4/§1.6, panel Lens A finding #2,
    /// panel Lens C finding #4).
    ///
    /// Derivation: worst-case frame bytes `≈ 2 * nlocals * 8 = 16 * nlocals` (the `+O(1)` spill-slot term
    /// is dwarfed by the `/ 32` margin below and so is not tracked precisely). Requiring `16 * nlocals <=
    /// i32::MAX / 2` — i.e. `nlocals <= i32::MAX / 32` — leaves a 2x margin for that additive term AND the
    /// `div_ceil(16)` rounding, AND a further 2x margin below `i32::MAX` itself. This evaluates to roughly
    /// 67 million locals — far beyond anything a real function needs; this bounds a pathological/
    /// adversarial value, not a realistic one.
    pub const MAX_SAFE_NLOCALS: usize = (i32::MAX as usize) / 32;

    /// `local` is a valid index into an `nlocals`-sized locals array. Checks the sign FIRST, as its own
    /// branch, before any arithmetic — promoting a possibly-negative `i32` straight into a
    /// same-signedness comparison against a `usize` is exactly the class of self-referential overflow bug
    /// this module exists to rule out in the code it checks (O-12 panel Lens C finding #2: the checker
    /// must not reintroduce, in itself, the very overflow pattern it is checking for).
    fn local_ok(local: i32, nlocals: usize) -> bool {
        if local < 0 {
            return false;
        }
        let nlocals_i64 = i64::try_from(nlocals).unwrap_or(i64::MAX);
        (local as i64) < nlocals_i64
    }

    /// `target` is a valid `code` index — strictly less than `code_len` (it becomes a `prog[pc]` index,
    /// never a one-past-the-end sentinel). Same sign-first discipline as [`local_ok`].
    fn jump_target_ok(target: i32, code_len: usize) -> bool {
        if target < 0 {
            return false;
        }
        let code_len_i64 = i64::try_from(code_len).unwrap_or(i64::MAX);
        (target as i64) < code_len_i64
    }

    /// `[base, base + 1 + argc)` fits inside an `nlocals`-sized locals array. `base`/`argc` are each
    /// individually bounded to `i32::MIN..=i32::MAX`, so `base as i64 + 1 + argc as i64` cannot overflow
    /// `i64` regardless of their values (no `checked_add` needed for that step) — but the sign check on
    /// each operand still comes first, same discipline as [`local_ok`]/[`jump_target_ok`] (panel Lens C
    /// finding #2).
    fn call_slot_ok(base: i32, argc: i32, nlocals: usize) -> bool {
        if base < 0 || argc < 0 {
            return false;
        }
        let nlocals_i64 = i64::try_from(nlocals).unwrap_or(i64::MAX);
        let end = base as i64 + 1 + argc as i64; // <= 2*i32::MAX + 1, far inside i64 — no overflow
        end <= nlocals_i64
    }

    /// Validate every operand [`eval`](super::interp::eval) (or its per-tier equivalent) would use as a
    /// raw index when evaluating `e` against an `nlocals`-sized locals array. Exhaustive on [`Expr`] — see
    /// the module doc for why that is load-bearing (panel Lens C finding #5), not stylistic.
    fn check_expr(e: &Expr, inst: &Inst, nlocals: usize, pc: usize) -> Result<(), ValidationError> {
        match e {
            Expr::Imm(_) | Expr::ImmF64(_) | Expr::OperandImm(_) => Ok(()),
            Expr::LocalAt(i) => {
                let local = inst.operand(*i);
                if local_ok(local, nlocals) {
                    Ok(())
                } else {
                    Err(ValidationError::LocalOutOfRange { pc, local, nlocals })
                }
            }
            Expr::Bin(_, a, b) => {
                check_expr(a, inst, nlocals, pc)?;
                check_expr(b, inst, nlocals, pc)
            }
            Expr::Un(_, a) => check_expr(a, inst, nlocals, pc),
            Expr::Host1(_, a) => check_expr(a, inst, nlocals, pc),
            Expr::Host2(_, a, b) => {
                check_expr(a, inst, nlocals, pc)?;
                check_expr(b, inst, nlocals, pc)
            }
            Expr::HostCtx1(_, a) => check_expr(a, inst, nlocals, pc),
            Expr::HostCtx2(_, a, b) => {
                check_expr(a, inst, nlocals, pc)?;
                check_expr(b, inst, nlocals, pc)
            }
        }
    }

    fn check_local_operand(slot: u8, inst: &Inst, nlocals: usize, pc: usize) -> Result<(), ValidationError> {
        let local = inst.operand(slot);
        if local_ok(local, nlocals) {
            Ok(())
        } else {
            Err(ValidationError::LocalOutOfRange { pc, local, nlocals })
        }
    }

    fn check_jump_operand(slot: u8, inst: &Inst, code_len: usize, pc: usize) -> Result<(), ValidationError> {
        let target = inst.operand(slot);
        if jump_target_ok(target, code_len) {
            Ok(())
        } else {
            Err(ValidationError::JumpTargetOutOfRange { pc, target, code_len })
        }
    }

    fn check_call_operands(
        base_slot: u8,
        argc_slot: u8,
        inst: &Inst,
        nlocals: usize,
        pc: usize,
    ) -> Result<(), ValidationError> {
        let base = inst.operand(base_slot);
        let argc = inst.operand(argc_slot);
        if call_slot_ok(base, argc, nlocals) {
            Ok(())
        } else {
            Err(ValidationError::CallSlotOutOfRange {
                pc,
                base,
                argc,
                nlocals,
            })
        }
    }

    /// Prove `f.code` cannot trigger the opcode/local-index/jump-target/call-slot indexing panics (or, in
    /// `otter-baseline`'s case, the panic-LESS memory corruption — O-12 design doc §1.4) any tier's hot
    /// path takes on faith under `isa` — see the module doc for exactly what this does and does not
    /// prove. `O(f.code.len())`, single pass, no allocation.
    pub fn check_func(isa: &Isa, f: &FuncDef) -> Result<(), ValidationError> {
        if f.code.is_empty() {
            return Err(ValidationError::EmptyCode);
        }
        let nlocals = f.nlocals;
        if nlocals > MAX_SAFE_NLOCALS {
            return Err(ValidationError::FrameTooLarge {
                nlocals,
                limit: MAX_SAFE_NLOCALS,
            });
        }
        // Checked via subtraction, not `nparams + consts.len() > nlocals` — `nlocals <= MAX_SAFE_NLOCALS`
        // above bounds `nlocals` only, NOT `f.nparams`/`f.consts.len()` individually, so a naive addition
        // of two attacker-controlled `usize` values could itself wrap (the exact self-referential overflow
        // pattern this module exists to rule out — panel Lens C finding #2's discipline, applied here too).
        if f.nparams > nlocals || f.consts.len() > nlocals - f.nparams {
            return Err(ValidationError::FrameLayoutTooSmall {
                nparams: f.nparams,
                consts_len: f.consts.len(),
                nlocals,
            });
        }
        let code_len = f.code.len();
        for (pc, inst) in f.code.iter().enumerate() {
            let op = isa
                .ops
                .get(inst.op as usize)
                .ok_or(ValidationError::OpcodeOutOfRange { pc, opcode: inst.op })?;
            for eff in &op.effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        check_local_operand(*reg, inst, nlocals, pc)?;
                        check_expr(value, inst, nlocals, pc)?;
                    }
                    Effect::JumpTo { target } => {
                        check_jump_operand(*target, inst, code_len, pc)?;
                    }
                    Effect::JumpIf { cond, target } => {
                        check_expr(cond, inst, nlocals, pc)?;
                        check_jump_operand(*target, inst, code_len, pc)?;
                    }
                    Effect::Halt { value } => {
                        check_expr(value, inst, nlocals, pc)?;
                    }
                    Effect::Guard { cond } => {
                        check_expr(cond, inst, nlocals, pc)?;
                    }
                    Effect::Call { dest, base, argc } => {
                        check_local_operand(*dest, inst, nlocals, pc)?;
                        check_call_operands(*base, *argc, inst, nlocals, pc)?;
                    }
                    Effect::TailCall { base, argc } => {
                        check_call_operands(*base, *argc, inst, nlocals, pc)?;
                    }
                    Effect::Return { value } => {
                        check_expr(value, inst, nlocals, pc)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// [`check_func`] applied to every function in `m`, plus [`Module::entry`]'s own range (which
    /// `interp::run_module`'s `make_frame` reads unguarded — O-12 design doc §1.2). Returns the index of
    /// the first function that fails alongside why — a deliberate, narrow deviation from the O-12 design
    /// doc's originally sketched `Result<(), ValidationError>` signature (§3.2): without the index, a
    /// multi-function `Module`'s caller has no way to tell WHICH function failed, which is exactly the
    /// piece of information an embedder needs to act on a rejection (e.g. reporting a REPL-level error at
    /// the right definition). `check_func` itself keeps the exact signature the design doc proposed.
    pub fn check_module(isa: &Isa, m: &Module) -> Result<(), (usize, ValidationError)> {
        if m.entry >= m.funcs.len() {
            return Err((
                m.entry,
                ValidationError::EntryOutOfRange {
                    entry: m.entry,
                    nfuncs: m.funcs.len(),
                },
            ));
        }
        for (idx, f) in m.funcs.iter().enumerate() {
            check_func(isa, f).map_err(|e| (idx, e))?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::sample;
        use crate::{OpDef, OpHints};
        use alloc::vec;

        fn wf_func() -> FuncDef {
            // Registers per `sample::sum_program`'s doc: 0=acc, 1=i, 2=n, 3=cond, 4=one — nlocals=5.
            FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 5,
                code: sample::sum_program(5),
            }
        }

        #[test]
        fn well_formed_func_passes() {
            let isa = sample::isa();
            assert_eq!(check_func(&isa, &wf_func()), Ok(()));
        }

        #[test]
        fn empty_code_rejected() {
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![],
            };
            assert_eq!(check_func(&isa, &f), Err(ValidationError::EmptyCode));
        }

        #[test]
        fn opcode_out_of_range_rejected() {
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(9999, 0, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::OpcodeOutOfRange { pc: 0, opcode: 9999 })
            );
        }

        #[test]
        fn local_out_of_range_rejected_on_write() {
            let isa = sample::isa();
            // MOV_IMM writes to local[100] (operand slot 0) but nlocals=1.
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(sample::MOV_IMM, 100, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::LocalOutOfRange {
                    pc: 0,
                    local: 100,
                    nlocals: 1
                })
            );
        }

        #[test]
        fn local_out_of_range_rejected_on_read() {
            let isa = sample::isa();
            // HALT reads local[operand 0] = 50, nlocals=1.
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(sample::HALT, 50, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::LocalOutOfRange {
                    pc: 0,
                    local: 50,
                    nlocals: 1
                })
            );
        }

        #[test]
        fn negative_local_rejected() {
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 2,
                code: vec![Inst::new(sample::HALT, -1, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::LocalOutOfRange {
                    pc: 0,
                    local: -1,
                    nlocals: 2
                })
            );
        }

        #[test]
        fn jump_target_out_of_range_rejected() {
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(sample::JUMP, 500, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::JumpTargetOutOfRange {
                    pc: 0,
                    target: 500,
                    code_len: 1
                })
            );
        }

        #[test]
        fn jump_target_one_past_end_rejected() {
            // target == code_len (one-past-the-end) must be rejected too — strict `<`, never `<=`.
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(sample::JUMP, 1, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::JumpTargetOutOfRange {
                    pc: 0,
                    target: 1,
                    code_len: 1
                })
            );
        }

        #[test]
        fn frame_too_large_rejected() {
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: MAX_SAFE_NLOCALS + 1,
                code: vec![Inst::new(sample::HALT, 0, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::FrameTooLarge {
                    nlocals: MAX_SAFE_NLOCALS + 1,
                    limit: MAX_SAFE_NLOCALS,
                })
            );
        }

        #[test]
        fn frame_at_limit_not_rejected() {
            // Off-by-one check: exactly AT the ceiling must be accepted (`FuncDef` doesn't allocate
            // `nlocals`-sized storage itself, so this stays a cheap test despite the large value).
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: MAX_SAFE_NLOCALS,
                code: vec![Inst::new(sample::HALT, 0, 0, 0)],
            };
            assert_eq!(check_func(&isa, &f), Ok(()));
        }

        #[test]
        fn frame_layout_too_small_for_nparams_plus_consts_rejected() {
            // Post-implementation review finding: nparams=5, consts.len()=8, nlocals=1 — every OTHER check
            // here passes (single HALT reading local 0), but the frame literally cannot fit its own
            // declared params+consts. `interp::run_module` panics on this today (`index out of bounds: the
            // len is 1 but the index is 5`); otter-baseline corrupts stack memory instead, with no panic at
            // all — exactly the O-12 §1.4 failure class this module exists to close.
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 5,
                consts: vec![1, 2, 3, 4, 5, 6, 7, 8],
                nlocals: 1,
                code: vec![Inst::new(sample::HALT, 0, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::FrameLayoutTooSmall {
                    nparams: 5,
                    consts_len: 8,
                    nlocals: 1,
                })
            );
        }

        #[test]
        fn frame_layout_exactly_fitting_nparams_plus_consts_not_rejected() {
            // Off-by-one check: nparams + consts.len() == nlocals exactly must be accepted.
            let isa = sample::isa();
            let f = FuncDef {
                nparams: 2,
                consts: vec![10, 20, 30],
                nlocals: 5,
                code: vec![Inst::new(sample::HALT, 0, 0, 0)],
            };
            assert_eq!(check_func(&isa, &f), Ok(()));
        }

        #[test]
        fn frame_layout_check_does_not_overflow_on_huge_nparams() {
            // nparams alone (usize::MAX) must be rejected without wrapping `nparams + consts.len()` back
            // into range — the exact self-referential overflow this module's sign-first/subtraction-based
            // checks exist to rule out (panel Lens C finding #2's discipline, applied to this check too).
            let isa = sample::isa();
            let f = FuncDef {
                nparams: usize::MAX,
                consts: vec![],
                nlocals: 5,
                code: vec![Inst::new(sample::HALT, 0, 0, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::FrameLayoutTooSmall {
                    nparams: usize::MAX,
                    consts_len: 0,
                    nlocals: 5,
                })
            );
        }

        // --- Call/TailCall/dest coverage: sample::isa() has no Call op, so build a tiny one (same shape
        // as otter-engine's test-only `call_isa()`).

        fn call_isa() -> Isa {
            Isa {
                ops: vec![
                    OpDef {
                        name: "halt",
                        effects: vec![Effect::Halt {
                            value: Expr::LocalAt(0),
                        }],
                        hints: OpHints {
                            pure: false,
                            terminator: true,
                        },
                    }, // 0
                    OpDef {
                        name: "call",
                        effects: vec![Effect::Call {
                            dest: 0,
                            base: 1,
                            argc: 2,
                        }],
                        hints: OpHints {
                            pure: false,
                            terminator: false,
                        },
                    }, // 1
                    OpDef {
                        name: "tailcall",
                        effects: vec![Effect::TailCall { base: 0, argc: 1 }],
                        hints: OpHints {
                            pure: false,
                            terminator: true,
                        },
                    }, // 2
                    OpDef {
                        name: "ret",
                        effects: vec![Effect::Return {
                            value: Expr::LocalAt(0),
                        }],
                        hints: OpHints {
                            pure: false,
                            terminator: true,
                        },
                    }, // 3
                ],
            }
        }
        const CALL_CALL: u16 = 1;
        const CALL_TAILCALL: u16 = 2;
        const CALL_RET: u16 = 3;

        #[test]
        fn call_dest_out_of_range_rejected() {
            let isa = call_isa();
            // dest=99 (invalid); base=0, argc=1 -> block [0,1] fits fine in nlocals=3.
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 3,
                code: vec![Inst::new(CALL_CALL, 99, 0, 1)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::LocalOutOfRange {
                    pc: 0,
                    local: 99,
                    nlocals: 3
                })
            );
        }

        #[test]
        fn call_slot_out_of_range_argc_too_big_rejected() {
            let isa = call_isa();
            // base=0, argc=10 -> needs slots [0,11) but nlocals=3.
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 3,
                code: vec![Inst::new(CALL_CALL, 0, 0, 10)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::CallSlotOutOfRange {
                    pc: 0,
                    base: 0,
                    argc: 10,
                    nlocals: 3
                })
            );
        }

        #[test]
        fn call_slot_negative_argc_rejected() {
            let isa = call_isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 3,
                code: vec![Inst::new(CALL_CALL, 0, 0, -1)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::CallSlotOutOfRange {
                    pc: 0,
                    base: 0,
                    argc: -1,
                    nlocals: 3
                })
            );
        }

        #[test]
        fn tailcall_slot_out_of_range_rejected() {
            let isa = call_isa();
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 2,
                code: vec![Inst::new(CALL_TAILCALL, 0, 5, 0)],
            };
            assert_eq!(
                check_func(&isa, &f),
                Err(ValidationError::CallSlotOutOfRange {
                    pc: 0,
                    base: 0,
                    argc: 5,
                    nlocals: 2
                })
            );
        }

        #[test]
        fn call_well_formed_accepted() {
            let isa = call_isa();
            // base=0 holds callee, base+1=1 holds arg0; argc=1 -> needs slots [0,2), fits in nlocals=3;
            // dest=2 also fits.
            let f = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 3,
                code: vec![Inst::new(CALL_CALL, 2, 0, 1), Inst::new(CALL_RET, 2, 0, 0)],
            };
            assert_eq!(check_func(&isa, &f), Ok(()));
        }

        #[test]
        fn entry_out_of_range_rejected() {
            let isa = sample::isa();
            let m = Module {
                funcs: vec![wf_func()],
                fill: 0,
                host_ctx: 0,
                entry: 5,
            };
            assert_eq!(
                check_module(&isa, &m),
                Err((5, ValidationError::EntryOutOfRange { entry: 5, nfuncs: 1 }))
            );
        }

        #[test]
        fn module_reports_failing_func_idx() {
            let isa = sample::isa();
            let bad = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(sample::HALT, 99, 0, 0)],
            };
            let m = Module {
                funcs: vec![wf_func(), bad],
                fill: 0,
                host_ctx: 0,
                entry: 0,
            };
            assert_eq!(
                check_module(&isa, &m),
                Err((
                    1,
                    ValidationError::LocalOutOfRange {
                        pc: 0,
                        local: 99,
                        nlocals: 1
                    }
                ))
            );
        }

        #[test]
        fn well_formed_module_passes() {
            let isa = sample::isa();
            let m = Module {
                funcs: vec![wf_func()],
                fill: 0,
                host_ctx: 0,
                entry: 0,
            };
            assert_eq!(check_module(&isa, &m), Ok(()));
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// Tier-up orchestration — the adaptive, multi-tier execution engine ("hybrid" axis).
// ---------------------------------------------------------------------------------------------------

pub mod engine {
    //! Adaptive **multi-tier** execution: run a function on the T0 interpreter, count how hot it gets,
    //! and promote it through an ordered stack of compilation tiers (e.g. T1 baseline copy-and-patch →
    //! T2 optimizing Cranelift) as it crosses ascending call thresholds. The orchestration *policy*
    //! lives here in the safe core; each tier is a [`Backend`] (which may need `unsafe`/`std` to emit and
    //! run machine code) injected from outside.
    //!
    //! Invariant (verified by callers): the value returned for a given `(function, args)` is identical
    //! no matter which tier served it — tiering is an optimization, never a semantic change.

    use super::interp::{Step, interpret_from, interpret_until_osr, interpret_with};
    use super::{Inst, Isa, Value};
    use alloc::boxed::Box;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Compiled, runnable code produced by a [`Backend`]. Kept alive while it may be called.
    pub trait Code {
        fn run(&self, args: &[Value]) -> Value;
    }

    /// A compilation tier: turns a function (program + arity) into [`Code`]. Backends are ordered by
    /// cost/quality (baseline before optimizing); the engine promotes through them as a function heats.
    pub trait Backend {
        fn name(&self) -> &'static str;
        fn compile(&self, isa: &Isa, prog: &[Inst], nlocals: usize, nargs: usize) -> Box<dyn Code>;
    }

    /// Outcome of running [`Resumable`] code on a frame.
    pub enum Run {
        /// Ran to a `Halt`; carries the return value.
        Completed(Value),
        /// Deoptimized at this pc; the frame now holds the live locals (resume the interpreter here).
        Deopt(usize),
    }

    /// Compiled code that runs over a caller-owned **frame** (the live `locals`) and may **deopt**
    /// (bail mid-function). Produced by an [`OsrBackend`] for on-stack replacement.
    pub trait Resumable {
        fn run_frame(&self, frame: &mut [Value]) -> Run;
    }

    /// A backend that can compile **resumable** code entering at an arbitrary basic-block leader — what
    /// the engine needs to OSR a hot loop mid-flight.
    pub trait OsrBackend {
        fn name(&self) -> &'static str;
        fn compile_resumable(
            &self,
            isa: &Isa,
            prog: &[Inst],
            nlocals: usize,
            entry_pc: usize,
        ) -> Box<dyn Resumable>;
    }

    /// Handle to a function registered with an [`Engine`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FuncId(pub usize);

    struct TierCfg {
        backend: Box<dyn Backend>,
        threshold: u32, // call count at which a function promotes to this tier
    }

    struct OsrCfg {
        backend: Box<dyn OsrBackend>,
        backedge_threshold: u32, // loop back-edges before a single in-flight call OSRs
    }

    struct FuncState {
        prog: Vec<Inst>,
        nlocals: usize,
        nargs: usize,
        calls: u32,
        level: usize, // 0 = interpreter; k = tiers[k-1]
        code: Option<Box<dyn Code>>,
    }

    /// Multi-tier adaptive execution engine. Level 0 is the interpreter; added tiers are JIT backends
    /// with strictly ascending call thresholds. Promotion is monotonic to the highest eligible tier.
    pub struct Engine {
        isa: Isa,
        tiers: Vec<TierCfg>,
        osr: Option<OsrCfg>,
        funcs: Vec<FuncState>,
    }

    impl Engine {
        pub fn new(isa: Isa) -> Self {
            Self {
                isa,
                tiers: Vec::new(),
                osr: None,
                funcs: Vec::new(),
            }
        }

        /// Enable **on-stack replacement**: while a function is still at the interpreter level, the
        /// engine counts loop back-edges and, once one reaches `backedge_threshold`, promotes the
        /// *in-flight* call into resumable compiled code (which may itself deopt — handled transparently
        /// by resuming the interpreter). This catches the "called once, loops forever" case that
        /// call-count tiering alone misses.
        pub fn enable_osr(&mut self, backend: Box<dyn OsrBackend>, backedge_threshold: u32) -> &mut Self {
            self.osr = Some(OsrCfg {
                backend,
                backedge_threshold,
            });
            self
        }

        /// Add a tier: its backend and the call count at which functions promote to it. Tiers must be
        /// added in strictly ascending threshold order (asserted).
        pub fn add_tier(&mut self, backend: Box<dyn Backend>, threshold: u32) -> &mut Self {
            if let Some(last) = self.tiers.last() {
                assert!(
                    threshold > last.threshold,
                    "tier thresholds must strictly increase"
                );
            }
            self.tiers.push(TierCfg { backend, threshold });
            self
        }

        /// Register a function (its bytecode, register count, and arity); returns its handle.
        pub fn register(&mut self, prog: Vec<Inst>, nlocals: usize, nargs: usize) -> FuncId {
            self.funcs.push(FuncState {
                prog,
                nlocals,
                nargs,
                calls: 0,
                level: 0,
                code: None,
            });
            FuncId(self.funcs.len() - 1)
        }

        /// Current tier level for `f` (0 = interpreter, 1 = first added tier, …).
        pub fn level(&self, f: FuncId) -> usize {
            self.funcs[f.0].level
        }

        /// Name of the tier currently serving `f` (`"interpreter"` or the backend's name).
        pub fn tier_name(&self, f: FuncId) -> &'static str {
            let lvl = self.funcs[f.0].level;
            if lvl == 0 {
                "interpreter"
            } else {
                self.tiers[lvl - 1].backend.name()
            }
        }

        pub fn calls(&self, f: FuncId) -> u32 {
            self.funcs[f.0].calls
        }

        /// Call function `f` with `args`. Promotes through tiers transparently; the result is identical
        /// regardless of which tier served it.
        pub fn call(&mut self, f: FuncId, args: &[Value]) -> Value {
            let i = f.0;
            self.funcs[i].calls += 1;
            let calls = self.funcs[i].calls;

            // highest tier whose threshold is satisfied (tiers are ascending)
            let mut target = 0usize;
            for (k, t) in self.tiers.iter().enumerate() {
                if calls >= t.threshold {
                    target = k + 1;
                }
            }
            if target > self.funcs[i].level {
                let code = self.tiers[target - 1].backend.compile(
                    &self.isa,
                    &self.funcs[i].prog,
                    self.funcs[i].nlocals,
                    self.funcs[i].nargs,
                );
                self.funcs[i].code = Some(code);
                self.funcs[i].level = target;
            }
            if let Some(code) = &self.funcs[i].code {
                return code.run(args);
            }
            // level 0 (interpreter). If OSR is enabled, run with back-edge counting so a hot loop in a
            // single in-flight call can promote; otherwise just interpret to completion.
            let nlocals = self.funcs[i].nlocals;
            match &self.osr {
                None => interpret_with(&self.isa, &self.funcs[i].prog, nlocals, args),
                Some(osr) => {
                    let mut frame = vec![0 as Value; nlocals];
                    for (slot, &a) in args.iter().enumerate() {
                        frame[slot] = a;
                    }
                    let prog = &self.funcs[i].prog;
                    match interpret_until_osr(&self.isa, prog, &mut frame, 0, osr.backedge_threshold) {
                        Step::Completed(v) => v,
                        Step::Osr(pc) => {
                            let code = osr.backend.compile_resumable(&self.isa, prog, nlocals, pc);
                            match code.run_frame(&mut frame) {
                                Run::Completed(v) => v,
                                Run::Deopt(dpc) => interpret_from(&self.isa, prog, &mut frame, dpc),
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::sample;

        /// A backend that "compiles" to nothing and runs via the interpreter — tests the engine's
        /// promotion STATE MACHINE (counting / thresholds / multi-level) independent of any real JIT.
        struct MockBackend(&'static str);
        struct MockCode {
            prog: Vec<Inst>,
            nlocals: usize,
        }
        impl Code for MockCode {
            fn run(&self, args: &[Value]) -> Value {
                interpret_with(&sample::isa(), &self.prog, self.nlocals, args)
            }
        }
        impl Backend for MockBackend {
            fn name(&self) -> &'static str {
                self.0
            }
            fn compile(&self, _isa: &Isa, prog: &[Inst], nlocals: usize, _nargs: usize) -> Box<dyn Code> {
                Box::new(MockCode {
                    prog: prog.to_vec(),
                    nlocals,
                })
            }
        }

        #[test]
        fn multi_tier_promotion() {
            let mut eng = Engine::new(sample::isa());
            eng.add_tier(Box::new(MockBackend("baseline")), 3);
            eng.add_tier(Box::new(MockBackend("optimizing")), 8);
            let f = eng.register(sample::sum_to_program(), 5, 1);
            assert_eq!(eng.level(f), 0);
            for k in 0..15i64 {
                let n = k % 7;
                let r = eng.call(f, &[n]);
                assert_eq!(r, n * (n + 1) / 2, "call k={k} n={n}");
                let c = eng.calls(f);
                let want = if c >= 8 {
                    2
                } else if c >= 3 {
                    1
                } else {
                    0
                };
                assert_eq!(eng.level(f), want, "level after {c} calls");
            }
            assert_eq!(eng.tier_name(f), "optimizing");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Inst;
    use super::interp::{interpret, interpret_with};
    use super::opt::constant_fold;
    use super::sample::{self, ADD, COPY, HALT, MOV_IMM, MUL};

    /// (dest_reg, imm) if `inst` is the sample materializer (MOV_IMM), else None.
    fn decode_mov(inst: Inst) -> Option<(i32, i32)> {
        if inst.op == MOV_IMM {
            Some((inst.operands[0], inst.operands[1]))
        } else {
            None
        }
    }

    #[test]
    fn layer_id() {
        assert!(super::layer().starts_with("L0"));
    }

    #[test]
    fn interpret_sum() {
        let isa = sample::isa();
        for n in [0, 1, 2, 10, 100, 1000] {
            let prog = sample::sum_program(n);
            assert_eq!(interpret(&isa, &prog, 5), (n as i64) * (n as i64 + 1) / 2);
        }
    }

    #[test]
    fn interpret_factorial() {
        let isa = sample::isa();
        let expected = [1i64, 1, 2, 6, 24, 120, 720];
        for (n, &e) in expected.iter().enumerate() {
            let prog = sample::factorial_program(n as i32);
            assert_eq!(interpret(&isa, &prog, 4), e, "fact({n})");
        }
    }

    #[test]
    fn interpret_float() {
        let isa = sample::isa();
        let prog = sample::float_expr_program(); // (x + y) * x
        for (x, y) in [(2.0_f64, 3.0_f64), (-1.5, 0.5), (0.0, 0.0), (1e10, -2.0)] {
            let r = interpret_with(&isa, &prog, 3, &[x.to_bits() as i64, y.to_bits() as i64]);
            assert_eq!(f64::from_bits(r as u64), (x + y) * x, "({x}+{y})*{x}");
        }
        // conversions round-trip: (i64)((f64)n + (f64)n) == 2n
        let conv = sample::convert_roundtrip_program();
        for n in [0i64, 1, 7, -3, 1000] {
            assert_eq!(interpret_with(&isa, &conv, 2, &[n]), 2 * n, "2*{n}");
        }
    }

    #[test]
    fn interpret_width_family() {
        use sample::{CONV_F64_U8, CONV_U8_I64, F32_ADD, F32_LT, F32_MUL, HALT, I8_ADD, LT, U8_ADD, U64_LT};
        let isa = sample::isa();
        // helper: build a 2-input op program (op dst=2, srcs 0,1) over two i64-immediate args
        let run2 = |op: u16, a: i64, b: i64| -> i64 {
            let prog = vec![Inst::new(op, 2, 0, 1), Inst::new(HALT, 2, 0, 0)];
            interpret_with(&isa, &prog, 3, &[a, b])
        };

        // 8-bit wrapping: 100 + 100 = 200 -> i8 = -56 ; 200 + 100 = 300 -> u8 = 44
        assert_eq!(run2(I8_ADD, 100, 100), -56);
        assert_eq!(run2(U8_ADD, 200, 100), 44);

        // signed vs unsigned compare: a = -1 (= u64::MAX), b = 1
        assert_eq!(run2(LT, -1, 1), 1, "signed: -1 < 1");
        assert_eq!(run2(U64_LT, -1, 1), 0, "unsigned: u64::MAX < 1 is false");

        // f32 arithmetic (args passed as f32 bit-words)
        let f32arg = |x: f32| x.to_bits() as i64;
        let r = run2(F32_ADD, f32arg(1.5), f32arg(2.25));
        assert_eq!(f32::from_bits(r as u32), 3.75);
        let r = run2(F32_MUL, f32arg(3.0), f32arg(0.5));
        assert_eq!(f32::from_bits(r as u32), 1.5);
        assert_eq!(run2(F32_LT, f32arg(1.0), f32arg(2.0)), 1);
        assert_eq!(run2(F32_LT, f32arg(f32::NAN), f32arg(2.0)), 0);

        // conversions: saturating f64->u8, then zero-extend u8->i64
        let run1 = |op: u16, a: i64| -> i64 {
            let prog = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            interpret_with(&isa, &prog, 2, &[a])
        };
        assert_eq!(
            run1(CONV_F64_U8, 300.0_f64.to_bits() as i64),
            255,
            "f64 300 -> u8 saturates 255"
        );
        assert_eq!(
            run1(CONV_F64_U8, (-5.0_f64).to_bits() as i64),
            0,
            "f64 -5 -> u8 saturates 0"
        );
        assert_eq!(run1(CONV_U8_I64, 0xFF), 255, "u8 0xFF -> i64 255 (zero-extend)");
    }

    #[test]
    fn interpret_f16() {
        use sample::{CONV_F16_F64, CONV_F64_F16, F16_ADD, F16_MUL, HALT};
        let isa = sample::isa();
        let run2 = |op: u16, a: i64, b: i64| -> i64 {
            let p = vec![Inst::new(op, 2, 0, 1), Inst::new(HALT, 2, 0, 0)];
            interpret_with(&isa, &p, 3, &[a, b])
        };
        let run1 = |op: u16, a: i64| -> i64 {
            let p = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            interpret_with(&isa, &p, 2, &[a])
        };
        // f16 add 1.0 + 0.5 = 1.5 (0x3C00 + 0x3800 = 0x3E00)
        assert_eq!(run2(F16_ADD, 0x3C00, 0x3800), 0x3E00);
        // f16 mul 2.0 * 0.5 = 1.0
        assert_eq!(run2(F16_MUL, 0x4000, 0x3800), 0x3C00);
        // f16 overflow -> +Inf (max normal 0x7BFF + itself)
        assert_eq!(run2(F16_ADD, 0x7BFF, 0x7BFF), 0x7C00);
        // f64 -> f16 (round) and f16 -> f64 (widen)
        assert_eq!(run1(CONV_F64_F16, 1.0_f64.to_bits() as i64), 0x3C00);
        assert_eq!(run1(CONV_F64_F16, 0.1_f64.to_bits() as i64), 0x2E66);
        assert_eq!(f64::from_bits(run1(CONV_F16_F64, 0x3C00) as u64), 1.0);
    }

    #[test]
    fn host_call_op() {
        use super::{Effect, Expr, Isa, OpDef, OpHints};
        use alloc::boxed::Box;
        extern "C" fn add3(a: i64, b: i64) -> i64 {
            a + b + 3
        }
        extern "C" fn negate(a: i64) -> i64 {
            -a
        }
        let h = OpHints {
            pure: false,
            terminator: false,
        };
        let isa = Isa {
            ops: alloc::vec![
                OpDef {
                    name: "h2",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::Host2(add3, Box::new(Expr::LocalAt(1)), Box::new(Expr::LocalAt(2))),
                    }],
                    hints: h,
                },
                OpDef {
                    name: "h1",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::Host1(negate, Box::new(Expr::LocalAt(1))),
                    }],
                    hints: h,
                },
                OpDef {
                    name: "halt",
                    effects: alloc::vec![Effect::Halt {
                        value: Expr::LocalAt(0)
                    }],
                    hints: OpHints {
                        pure: false,
                        terminator: true
                    },
                },
            ],
        };
        // r2 = add3(r0, r1); r2 = negate(r2); halt r2   ==  -((a+b)+3)
        let prog = alloc::vec![
            Inst::new(0, 2, 0, 1),
            Inst::new(1, 2, 2, 0),
            Inst::new(2, 2, 0, 0)
        ];
        assert_eq!(interpret_with(&isa, &prog, 3, &[10, 20]), -(10 + 20 + 3));
    }

    /// A minimal ISA with `Call`/`Return`, used to exercise the call-stack interpreter `run_module`.
    fn call_isa() -> super::Isa {
        use super::{ArithOp, BinOp, Effect, Expr, NumTy, OpDef, OpHints};
        use alloc::boxed::Box;
        let pure = OpHints {
            pure: true,
            terminator: false,
        };
        let term = OpHints {
            pure: false,
            terminator: true,
        };
        let alu = |name, op| OpDef {
            name,
            effects: alloc::vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::Bin(
                    BinOp::new(NumTy::I64, op),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::LocalAt(2))
                ),
            }],
            hints: pure,
        };
        super::Isa {
            ops: alloc::vec![
                // 0 MOV_IMM: local[op0] = sign-extended i32 op1
                OpDef {
                    name: "mov_imm",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::OperandImm(1)
                    }],
                    hints: pure
                },
                alu("mul", ArithOp::Mul), // 1
                alu("sub", ArithOp::Sub), // 2
                alu("lt", ArithOp::Lt),   // 3
                // 4 JUMP_IF: if local[op0] != 0 goto op1
                OpDef {
                    name: "jump_if",
                    effects: alloc::vec![Effect::JumpIf {
                        cond: Expr::LocalAt(0),
                        target: 1
                    }],
                    hints: term
                },
                // 5 COPY: local[op0] = local[op1]
                OpDef {
                    name: "copy",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::LocalAt(1)
                    }],
                    hints: pure
                },
                // 6 CALL: local[op0] = funcs[ local[op1] ]( local[op1+1 .. op1+1+op2] )
                OpDef {
                    name: "call",
                    effects: alloc::vec![Effect::Call {
                        dest: 0,
                        base: 1,
                        argc: 2
                    }],
                    hints: OpHints {
                        pure: false,
                        terminator: false
                    }
                },
                // 7 RET: return local[op0]
                OpDef {
                    name: "ret",
                    effects: alloc::vec![Effect::Return {
                        value: Expr::LocalAt(0)
                    }],
                    hints: term
                },
                // 8 HALT: return local[op0]
                OpDef {
                    name: "halt",
                    effects: alloc::vec![Effect::Halt {
                        value: Expr::LocalAt(0)
                    }],
                    hints: term
                },
            ],
        }
    }

    #[test]
    fn run_module_recursion() {
        use super::interp::run_module;
        use super::{FuncDef, Module};
        const MOV: u16 = 0; // local[op0] = imm op1
        const MUL_: u16 = 1;
        const SUB_: u16 = 2;
        const LT_: u16 = 3;
        const JIF: u16 = 4; // if local[op0] != 0 goto op1
        const CPY: u16 = 5;
        const CALL: u16 = 6; // local[op0] = funcs[local[op1]](local[op1+1 ..]); argc = op2
        const RET: u16 = 7;
        const HALT_: u16 = 8;

        // fact(n) [func 1], n = local0. Base case n < 2 → 1, else n * fact(n-1).
        // The recursive `RET prod` (pc 9) and the base-case `RET 1` (pc 11) are distinct returns.
        let fact = FuncDef {
            nparams: 1,
            consts: alloc::vec![],
            nlocals: 10,
            code: alloc::vec![
                Inst::new(MOV, 1, 2, 0),  // 0: two = 2
                Inst::new(LT_, 2, 0, 1),  // 1: cmp = (n < 2)
                Inst::new(JIF, 2, 10, 0), // 2: if cmp goto 10 (base case)
                Inst::new(MOV, 3, 1, 0),  // 3: one = 1
                Inst::new(SUB_, 4, 0, 3), // 4: nm1 = n - 1
                Inst::new(MOV, 5, 1, 0),  // 5: callee = func index 1
                Inst::new(CPY, 6, 4, 0),  // 6: arg = nm1  (call block = [callee@5, arg@6])
                Inst::new(CALL, 7, 5, 1), // 7: res = fact(nm1)
                Inst::new(MUL_, 8, 0, 7), // 8: prod = n * res
                Inst::new(RET, 8, 0, 0),  // 9: return prod
                Inst::new(MOV, 9, 1, 0),  // 10: baseval = 1
                Inst::new(RET, 9, 0, 0),  // 11: return 1
            ],
        };
        // main() [func 0]: return fact(6) = 720.
        let main = FuncDef {
            nparams: 0,
            consts: alloc::vec![],
            nlocals: 3,
            code: alloc::vec![
                Inst::new(MOV, 0, 1, 0),   // 0: callee = func index 1
                Inst::new(MOV, 1, 6, 0),   // 1: arg = 6
                Inst::new(CALL, 2, 0, 1),  // 2: res = fact(6)  (dest=2, base=0, argc=1)
                Inst::new(HALT_, 2, 0, 0), // 3: halt res
            ],
        };
        let module = Module {
            funcs: alloc::vec![main, fact],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        };
        assert_eq!(run_module(&call_isa(), &module, &[]), 720);
    }

    #[test]
    fn run_module_missing_args_use_fill() {
        use super::interp::run_module;
        use super::{FuncDef, Module};
        const CALL: u16 = 6;
        const RET: u16 = 7;
        const HALT_: u16 = 8;
        const MOV: u16 = 0;

        // second(a, b) returns its 2nd parameter (local1).
        let second = FuncDef {
            nparams: 2,
            consts: alloc::vec![],
            nlocals: 2,
            code: alloc::vec![Inst::new(RET, 1, 0, 0)],
        };
        // main(): call second with ONE argument → param `b` is the missing-arg `fill`.
        let main = FuncDef {
            nparams: 0,
            consts: alloc::vec![],
            nlocals: 3,
            code: alloc::vec![
                Inst::new(MOV, 0, 1, 0),  // callee = func 1
                Inst::new(MOV, 1, 7, 0),  // arg0 = 7
                Inst::new(CALL, 2, 0, 1), // res = second(7)  (argc=1; b missing)
                Inst::new(HALT_, 2, 0, 0),
            ],
        };
        let module = Module {
            funcs: alloc::vec![main, second],
            fill: 999,
            host_ctx: 0,
            entry: 0,
        };
        assert_eq!(run_module(&call_isa(), &module, &[]), 999); // missing `b` == fill
    }

    /// O-11: `run_module` threads `Module::host_ctx` (NOT `fill`) to `HostCtx1`/`HostCtx2` — and the two
    /// words stay independent even when a call uses both (an absent arg falling back to `fill` alongside
    /// a `HostCtx2` combining `host_ctx` with real operands). Values include the full 64-bit range
    /// (`i64::MIN`, a value with the top bit set) — not just something that fits a 32-bit immediate — so a
    /// backend that accidentally truncates/sign-extends via a 32-bit move would be caught by a differential
    /// test built on this same module (see the cross-tier tests in `otter-cranelift`/`otter-baseline`).
    #[test]
    fn run_module_threads_host_ctx_independent_of_fill() {
        use super::interp::run_module;
        use super::{Effect, Expr, FuncDef, Isa, Module, OpDef, OpHints, Value};
        use alloc::boxed::Box;

        extern "C" fn identity_ctx(host_ctx: Value, _x: Value) -> Value {
            host_ctx
        }
        extern "C" fn combine_ctx(host_ctx: Value, x: Value, y: Value) -> Value {
            host_ctx ^ x ^ y
        }

        const CTX1: u16 = 0; // local[op0] = HostCtx1(local[op1])
        const CTX2: u16 = 1; // local[op0] = HostCtx2(local[op1], local[op2])
        const RET: u16 = 2;
        const CALL: u16 = 3;
        const MOV: u16 = 4;

        let pure = OpHints {
            pure: false, // host calls are never `pure` (O-11 design §7 risk 2)
            terminator: false,
        };
        let term = OpHints {
            pure: false,
            terminator: true,
        };
        let isa = Isa {
            ops: alloc::vec![
                OpDef {
                    name: "ctx1",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::HostCtx1(identity_ctx, Box::new(Expr::LocalAt(1))),
                    }],
                    hints: pure,
                },
                OpDef {
                    name: "ctx2",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::HostCtx2(
                            combine_ctx,
                            Box::new(Expr::LocalAt(1)),
                            Box::new(Expr::LocalAt(2))
                        ),
                    }],
                    hints: pure,
                },
                OpDef {
                    name: "ret",
                    effects: alloc::vec![Effect::Return {
                        value: Expr::LocalAt(0)
                    }],
                    hints: term,
                },
                OpDef {
                    name: "call",
                    effects: alloc::vec![Effect::Call {
                        dest: 0,
                        base: 1,
                        argc: 2
                    }],
                    hints: OpHints {
                        pure: false,
                        terminator: false
                    },
                },
                OpDef {
                    name: "mov_imm",
                    effects: alloc::vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::OperandImm(1)
                    }],
                    hints: OpHints {
                        pure: true,
                        terminator: false
                    },
                },
            ],
        };

        // second(a, b): r2 = HostCtx1(a); r3 = HostCtx2(b, r2); return r3 ^ b_is_fill_marker-independent.
        // (b is deliberately left absent by the caller below, so it observes `fill`, not `host_ctx`.)
        let second = FuncDef {
            nparams: 2,
            consts: alloc::vec![],
            nlocals: 4,
            code: alloc::vec![
                Inst::new(CTX1, 2, 0, 0), // r2 = HostCtx1(a) == host_ctx
                Inst::new(CTX2, 3, 1, 2), // r3 = HostCtx2(b, r2) == host_ctx ^ b ^ r2
                Inst::new(RET, 3, 0, 0),
            ],
        };
        // main(): call second(7) — `b` is missing, so `second` observes `locals[1] == fill`.
        let main = FuncDef {
            nparams: 0,
            consts: alloc::vec![],
            nlocals: 3,
            code: alloc::vec![
                Inst::new(MOV, 0, 1, 0),  // callee = func 1
                Inst::new(MOV, 1, 7, 0),  // arg0 = 7 (only arg — `b` absent)
                Inst::new(CALL, 2, 0, 1), // res = second(7)
                Inst::new(RET, 2, 0, 0),
            ],
        };
        for (fill, host_ctx) in [(999i64, 0xDEAD_BEEFi64), (i64::MIN, i64::MIN ^ 1), (0, i64::MIN)] {
            let module = Module {
                funcs: alloc::vec![main.clone(), second.clone()],
                fill,
                host_ctx,
                entry: 0,
            };
            let got = run_module(&isa, &module, &[]);
            // r2 = host_ctx; r3 = host_ctx ^ fill ^ host_ctx == fill (since b is absent -> fill)
            let want = host_ctx ^ fill ^ host_ctx;
            assert_eq!(got, want, "fill={fill} host_ctx={host_ctx}");
            assert_eq!(
                got, fill,
                "HostCtx2 combining host_ctx with itself must reduce to fill exactly"
            );
        }

        // Independent sanity check: HostCtx1 alone returns EXACTLY host_ctx, never fill, even when
        // fill == 0 and host_ctx is the full-width i64::MIN.
        let ctx_only = FuncDef {
            nparams: 0,
            consts: alloc::vec![],
            nlocals: 1,
            code: alloc::vec![
                Inst::new(CTX1, 0, 0, 0), // r0 = HostCtx1(r0) (r0 starts at `fill`)
                Inst::new(RET, 0, 0, 0),
            ],
        };
        let module = Module {
            funcs: alloc::vec![ctx_only],
            fill: 0,
            host_ctx: i64::MIN,
            entry: 0,
        };
        assert_eq!(run_module(&isa, &module, &[]), i64::MIN);
    }

    /// **Bytecode-compat invariant gate**: f16 results MUST equal the IEEE-754 binary16 reference table
    /// regardless of the `half` vs `f16-intrinsic` build config (both implement round-to-nearest-even).
    /// CI runs this under both configs; identical assertions ⟹ identical bytecode results.
    #[test]
    fn f16_ieee_reference_table() {
        use sample::{CONV_F64_F16, HALT};
        let isa = sample::isa();
        let round = |x: f64| -> u16 {
            let p = vec![Inst::new(CONV_F64_F16, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            interpret_with(&isa, &p, 2, &[x.to_bits() as i64]) as u16
        };
        let table: [(f64, u16); 9] = [
            (0.0, 0x0000),
            (-0.0, 0x8000),
            (1.0, 0x3C00),
            (-1.0, 0xBC00),
            (0.5, 0x3800),
            (2.0, 0x4000),
            (0.1, 0x2E66),
            (65504.0, 0x7BFF), // max normal
            (65536.0, 0x7C00), // overflow -> +Inf
        ];
        for (x, bits) in table {
            assert_eq!(round(x), bits, "f16({x})");
        }
    }

    /// **W-0.5 canonical-semantics gate** (O-13 design §4.1 ①): `Neg(F16)` is an IEEE-754 binary16
    /// sign flip — numerically correct, NaN-payload-preserving, and normalized ("IEEE bits in the low
    /// 16 bits, the rest zero"). The old behavior (fall-through to the integer arm: full 64-bit
    /// two's complement) violated all three; these reference bits are the tier-parity anchor the
    /// cross-tier tests in `otter-baseline`/`otter-engine` compare against.
    #[test]
    fn neg_f16_is_ieee_sign_flip() {
        use super::{NumTy, UnOp};
        let neg = |w: i64| UnOp::Neg(NumTy::F16).eval(w);
        let table: [(i64, i64, &str); 4] = [
            (0x3C00, 0xBC00, "1.0 -> -1.0"),
            (0x0000, i64::from(0x8000u16), "+0.0 -> -0.0"),
            (0x7E01, i64::from(0xFE01u16), "NaN payload preserved"),
            (0xFC00, 0x7C00, "-inf -> +inf"),
        ];
        for (a, want, what) in table {
            let got = neg(a);
            assert_eq!(got, want, "{what}: neg(0x{a:04X})");
            // normalization invariant: high 48 bits zero
            assert_eq!(got as u64 >> 16, 0, "{what}: result not normalized");
        }
        // A non-normalized slot word (garbage high bits) is truncated to its 16 IEEE bits first —
        // the exact behavior the T1/T2 lowerings must reproduce bit-for-bit.
        assert_eq!(neg(0xFFFF_3C00u32 as i64 | (-1i64 << 32)), 0xBC00);
        // Involution: neg(neg(x)) == normalized x.
        for w in [0i64, 0x3C00, 0x7E01, 0x7C00, i64::from(0xFFFFu16)] {
            assert_eq!(neg(neg(w)), w & 0xFFFF, "neg∘neg(0x{w:04X})");
        }
    }

    /// `otter_unop` (W-0.5 ⑤, for the wasm-emit backend) must be a *verbatim* re-export of the
    /// canonical `UnOp::eval` — checked here for `Neg` across ALL 11 `NumTy`s over a diverse word
    /// set (full-width, sign-bit, non-normalized, float specials).
    #[test]
    fn otter_unop_matches_canonical_eval_all_numtys() {
        use super::{NumTy, UnOp, otter_unop};
        use NumTy::*;
        let words: [i64; 12] = [
            0,
            1,
            -1,
            i64::MIN,
            i64::MAX,
            0x3C00,                            // 1.0f16
            0x7E01,                            // f16 NaN w/ payload
            0x8000_0000u32 as i64,             // -0.0f32 bits
            f64::NAN.to_bits() as i64,         // f64 NaN
            (-0.0f64).to_bits() as i64,        // f64 sign bit only
            0x0123_4567_89AB_CDEFu64 as i64,   // arbitrary full-width
            (0xFFFFu64 << 48 | 0x3C00) as i64, // non-normalized f16 slot
        ];
        for ty in [I8, I16, I32, I64, U8, U16, U32, U64, F16, F32, F64] {
            for a in words {
                assert_eq!(
                    otter_unop(a, ty.tag(), UnOp::NEG_TAG),
                    UnOp::Neg(ty).eval(a),
                    "otter_unop != UnOp::eval for {ty:?} a={a:#018X}"
                );
            }
        }
    }

    #[test]
    fn fold_chained_constants() {
        let isa = sample::isa();
        let prog = vec![
            Inst::new(MOV_IMM, 0, 3, 0),
            Inst::new(MOV_IMM, 1, 4, 0),
            Inst::new(ADD, 2, 0, 1), // r2 = 3 + 4 = 7
            Inst::new(MUL, 3, 2, 0), // r3 = 7 * 3 = 21 (uses the just-folded r2)
            Inst::new(HALT, 3, 0, 0),
        ];
        let opt = constant_fold(&isa, &prog, 4);
        // both arithmetic ops fold to the discovered materializer
        assert_eq!(decode_mov(opt[2]), Some((2, 7)));
        assert_eq!(decode_mov(opt[3]), Some((3, 21)));
        // semantics preserved
        assert_eq!(interpret(&isa, &opt, 4), interpret(&isa, &prog, 4));
        assert_eq!(interpret(&isa, &opt, 4), 21);
    }

    #[test]
    fn overflow_constant_not_materialized() {
        let isa = sample::isa();
        let prog = vec![
            Inst::new(MOV_IMM, 0, 100_000, 0),
            Inst::new(MOV_IMM, 1, 100_000, 0),
            Inst::new(MUL, 2, 0, 1), // 1e10 > i32::MAX -> must stay a MUL (tracked, not materialized)
            Inst::new(HALT, 2, 0, 0),
        ];
        let opt = constant_fold(&isa, &prog, 3);
        assert_eq!(opt[2], prog[2], "constant exceeding i32 must NOT be materialized");
        assert_eq!(interpret(&isa, &opt, 3), 10_000_000_000);
    }

    #[test]
    fn fold_preserves_and_is_length_equal_on_samples() {
        let isa = sample::isa();
        for n in [0, 1, 2, 10, 100, 1000] {
            let p = sample::sum_program(n);
            let o = constant_fold(&isa, &p, 5);
            assert_eq!(o.len(), p.len(), "length-preserving (pc/targets stay valid)");
            assert_eq!(interpret(&isa, &o, 5), interpret(&isa, &p, 5), "sum({n})");
        }
        for n in 0..=10 {
            let p = sample::factorial_program(n);
            let o = constant_fold(&isa, &p, 4);
            assert_eq!(interpret(&isa, &o, 4), interpret(&isa, &p, 4), "fact({n})");
        }
    }

    // --- algebraic simplification driven by primitive hints (B0.5-2) -------------------------------

    /// `x * 0 → 0` even though `x` (arg 0) is unknown at compile time: the absorbing-element hint lets
    /// the optimizer materialize 0 where plain constant folding cannot.
    #[test]
    fn algebraic_mul_by_zero() {
        let isa = sample::isa();
        // regs: 0=x(arg), 1=zero, 2=prod
        let prog = vec![
            Inst::new(MOV_IMM, 1, 0, 0), // zero = 0
            Inst::new(MUL, 2, 0, 1),     // prod = x * 0
            Inst::new(HALT, 2, 0, 0),
        ];
        let opt = constant_fold(&isa, &prog, 3);
        assert_eq!(decode_mov(opt[1]), Some((2, 0)), "x*0 should materialize 0");
        for x in [0i64, 1, 7, -3, 1_000_000] {
            assert_eq!(interpret_with(&isa, &opt, 3, &[x]), 0);
            assert_eq!(
                interpret_with(&isa, &opt, 3, &[x]),
                interpret_with(&isa, &prog, 3, &[x])
            );
        }
    }

    /// `x + 0 → x` and `x * 1 → x` via the right-identity hint + a discovered copy op (`x` unknown).
    #[test]
    fn algebraic_identity_becomes_copy() {
        let isa = sample::isa();
        // x + 0  : regs 0=x(arg), 1=zero, 2=sum
        let add0 = vec![
            Inst::new(MOV_IMM, 1, 0, 0),
            Inst::new(ADD, 2, 0, 1), // sum = x + 0
            Inst::new(HALT, 2, 0, 0),
        ];
        let o = constant_fold(&isa, &add0, 3);
        assert_eq!(o[1].op, COPY, "x+0 should become a copy");
        assert_eq!((o[1].operands[0], o[1].operands[1]), (2, 0), "copy x -> sum");
        // x * 1
        let mul1 = vec![
            Inst::new(MOV_IMM, 1, 1, 0),
            Inst::new(MUL, 2, 0, 1), // prod = x * 1
            Inst::new(HALT, 2, 0, 0),
        ];
        let o2 = constant_fold(&isa, &mul1, 3);
        assert_eq!(o2[1].op, COPY, "x*1 should become a copy");
        for x in [0i64, 1, 9, -5, 123_456] {
            assert_eq!(interpret_with(&isa, &o, 3, &[x]), x);
            assert_eq!(interpret_with(&isa, &o2, 3, &[x]), x);
        }
    }

    /// Commutative left identity: `0 + x → x` (the identity is the *left* operand).
    #[test]
    fn algebraic_commutative_left_identity() {
        let isa = sample::isa();
        let prog = vec![
            Inst::new(MOV_IMM, 1, 0, 0),
            Inst::new(ADD, 2, 1, 0), // sum = 0 + x  (left operand is the identity)
            Inst::new(HALT, 2, 0, 0),
        ];
        let o = constant_fold(&isa, &prog, 3);
        assert_eq!(o[1].op, COPY, "0+x should become a copy");
        assert_eq!((o[1].operands[0], o[1].operands[1]), (2, 0), "copy x -> sum");
        for x in [0i64, 2, -8, 77_777] {
            assert_eq!(interpret_with(&isa, &o, 3, &[x]), x);
        }
    }

    // ---- O-14 §3.2a (A-C1): full-width callee conversion at the T0 sites ------------------------

    /// `call_isa()` + a `TailCall` op (9): base = op0, argc = op1 — the same additive-extension
    /// shape as otter-engine's test-only `call_isa_with_hostctx()`.
    fn call_isa_with_tailcall() -> super::Isa {
        use super::{Effect, OpDef, OpHints};
        let mut isa = call_isa();
        isa.ops.push(OpDef {
            name: "tailcall",
            effects: alloc::vec![Effect::TailCall { base: 0, argc: 1 }],
            hints: OpHints {
                pure: false,
                terminator: true,
            },
        }); // 9
        isa
    }

    /// A-C1: `run_one` must hand its `dispatch` closure (and `TailReq.callee`) the FULL-WIDTH
    /// callee index — never a 32-bit truncation of the word. On this 64-bit target the old bare
    /// `as usize` was already full-width, so this is a semantic PIN of "width check precedes the
    /// cast" (`interp::callee_index`, target-independent by construction): it goes red if a
    /// width-dependent truncation is ever reintroduced into the shared conversion.
    #[test]
    fn run_one_full_width_oob_callee_reaches_dispatch_untruncated() {
        use super::FuncDef;
        use super::interp::{TailReq, run_one};
        const CPY: u16 = 5;
        const CALL: u16 = 6;
        const RET: u16 = 7;
        const TAILCALL: u16 = 9;
        const FILL: i64 = -9999;
        const NFUNCS: usize = 3; // the mini function space the closure below range-checks against
        let isa = call_isa_with_tailcall();
        // caller_call(w): res = dispatch(funcs[w]); return res
        let caller_call = FuncDef {
            nparams: 1,
            consts: alloc::vec![],
            nlocals: 3,
            code: alloc::vec![
                Inst::new(CPY, 1, 0, 0),  // block[0] = callee word
                Inst::new(CALL, 2, 1, 0), // res = dispatch(w)   (dest=2, base=1, argc=0)
                Inst::new(RET, 2, 0, 0),
            ],
        };
        // Full-width OOB words whose LOW 32 BITS are in range (< NFUNCS): 2^32+2, -2^32, 2^32.
        for w in [(1i64 << 32) + 2, -(1i64 << 32), 1i64 << 32] {
            let mut seen: Option<usize> = None;
            let mut tail = TailReq::default();
            let got = run_one(
                &isa,
                &caller_call,
                &[w],
                FILL,
                0,
                &mut |idx, _args| {
                    seen = Some(idx);
                    // mini-engine: the engine's fail-closed loop-head shape (in_range → fill)
                    if idx < NFUNCS { 42 } else { FILL }
                },
                &mut tail,
            );
            let idx = seen.expect("Call must reach dispatch");
            assert!(
                idx >= NFUNCS,
                "callee={w}: dispatch must see an OOB index, got {idx}"
            );
            assert_ne!(
                idx,
                (w as u32) as usize,
                "callee={w}: index was truncated to 32 bits"
            );
            assert_eq!(got, FILL, "callee={w}: the OOB fallback value must flow back");
            assert!(!tail.pending, "an ordinary Call never signals a tail");
        }
        // caller_tail(w): tailcall funcs[w] — `TailReq.callee` gets the same conversion.
        let caller_tail = FuncDef {
            nparams: 1,
            consts: alloc::vec![],
            nlocals: 2,
            code: alloc::vec![
                Inst::new(CPY, 1, 0, 0),      // block[0] = callee word
                Inst::new(TAILCALL, 1, 0, 0), // base=1, argc=0
            ],
        };
        for w in [(1i64 << 32) + 2, -(1i64 << 32), 1i64 << 32] {
            let mut tail = TailReq::default();
            let got = run_one(
                &isa,
                &caller_tail,
                &[w],
                FILL,
                0,
                &mut |_idx, _args| panic!("a TailCall must signal, never recurse into dispatch"),
                &mut tail,
            );
            assert_eq!(got, FILL, "run_one returns fill on a tail signal");
            assert!(tail.pending, "tail must be signaled");
            assert!(
                tail.callee >= NFUNCS,
                "callee={w}: the trampoline must see an OOB index, got {}",
                tail.callee
            );
            assert_ne!(
                tail.callee,
                (w as u32) as usize,
                "callee={w}: TailReq.callee was truncated to 32 bits"
            );
        }
    }

    /// A-C1, reference-oracle side: `run_module` treats a full-width OOB callee word as what it
    /// IS — an out-of-range function index, i.e. the oracle's documented malformed-module panic
    /// (unchanged class) — and must never silently call the function named by its low 32 bits
    /// (here func 1, which would return 42 and make this `should_panic` test fail).
    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn run_module_full_width_oob_call_is_oob_not_a_truncated_call() {
        use super::interp::run_module;
        use super::{FuncDef, Module};
        const CALL: u16 = 6;
        const RET: u16 = 7;
        const MOV: u16 = 0;
        const HALT_: u16 = 8;
        // main(w): res = funcs[w](); halt res   — callee word arrives as the entry argument
        let main = FuncDef {
            nparams: 1,
            consts: alloc::vec![],
            nlocals: 2,
            code: alloc::vec![Inst::new(CALL, 1, 0, 0), Inst::new(HALT_, 1, 0, 0)],
        };
        let f1 = FuncDef {
            nparams: 0,
            consts: alloc::vec![],
            nlocals: 1,
            code: alloc::vec![Inst::new(MOV, 0, 42, 0), Inst::new(RET, 0, 0, 0)],
        };
        let module = Module {
            funcs: alloc::vec![main, f1],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        };
        run_module(&call_isa(), &module, &[(1i64 << 32) + 1]); // low 32 bits = 1 = f1
    }

    /// A-C1: the `TailCall` twin of the test above — same documented OOB panic, same
    /// never-a-truncated-call requirement.
    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn run_module_full_width_oob_tailcall_is_oob_not_a_truncated_call() {
        use super::interp::run_module;
        use super::{FuncDef, Module};
        const MOV: u16 = 0;
        const RET: u16 = 7;
        const TAILCALL: u16 = 9;
        // main(w): tailcall funcs[w]()
        let main = FuncDef {
            nparams: 1,
            consts: alloc::vec![],
            nlocals: 1,
            code: alloc::vec![Inst::new(TAILCALL, 0, 0, 0)],
        };
        let f1 = FuncDef {
            nparams: 0,
            consts: alloc::vec![],
            nlocals: 1,
            code: alloc::vec![Inst::new(MOV, 0, 42, 0), Inst::new(RET, 0, 0, 0)],
        };
        let module = Module {
            funcs: alloc::vec![main, f1],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        };
        run_module(&call_isa_with_tailcall(), &module, &[(1i64 << 32) + 1]);
    }
}
