//! L0 backend platform shim — a **generic** Cranelift JIT for any [`otter::Isa`].
//!
//! This is the "machine-code emission" half of the meta-hybrid toolchain, kept in a separate crate
//! because it needs `std`, the Cranelift toolchain, and concentrated `unsafe` (transmuting a code
//! pointer). It consumes the SAME [`OpDef`]/[`Effect`]/[`Expr`] table the interpreter consumes — there
//! is no second, hand-written semantics for the JIT. Add an op to the ISA → it interprets AND compiles.
//!
//! Lowering mirrors S3 increment 2b but is driven by the IR rather than a hardcoded `match`:
//!   * basic-block leaders are derived generically from control [`Effect`]s (`JumpTo`/`JumpIf`) and the
//!     `terminator` hint — not from specific opcodes;
//!   * locals become SSA `Variable`s (`declare_var`); `cranelift-frontend` inserts block params;
//!   * each [`Effect`]/[`Expr`] node has exactly one lowering.
//!
//! Verification (codegen is a dangerous subsystem → verify-before-implement): the differential tests
//! assert `jit(prog) == interpret(prog)` for hand-written control-flow programs AND randomized
//! straight-line programs — the interpreter ([`otter::interp`]) is the reference oracle.
#![allow(unsafe_code)] // dangerous subsystem: machine-code emission / code-pointer transmute concentrated here

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Function, InstBuilder, MemFlags, Signature, StackSlotData, StackSlotKind, Type,
    Value as ClValue, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use otter::engine::{OsrBackend, Resumable, Run};
use otter::{ArithOp, Effect, Expr, FuncDef, Inst, Isa, Module as OtterModule, NumTy, UnOp, Value};

// Addresses of the canonical interpreter helpers (single source of truth). The JIT `call_indirect`s
// these for ops it doesn't lower natively (currently `f16`), guaranteeing bit-identical results across
// tiers + across the half/f16-intrinsic configs. (SystemV ABI; host = x86-64 Linux.)
fn binop_addr() -> i64 {
    (otter::otter_binop as extern "C" fn(i64, i64, u32, u32) -> i64) as usize as i64
}
fn convert_addr() -> i64 {
    (otter::otter_convert as extern "C" fn(i64, u32, u32) -> i64) as usize as i64
}

/// Emit a call to `otter_binop(x, y, ty, op)`.
fn call_binop(b: &mut FunctionBuilder, x: ClValue, y: ClValue, ty: NumTy, op: ArithOp) -> ClValue {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(types::I32));
    sig.returns.push(AbiParam::new(types::I64));
    let sigref = b.import_signature(sig);
    let tc = b.ins().iconst(types::I32, i64::from(ty.tag()));
    let oc = b.ins().iconst(types::I32, i64::from(op.tag()));
    let addr = b.ins().iconst(types::I64, binop_addr());
    let call = b.ins().call_indirect(sigref, addr, &[x, y, tc, oc]);
    b.inst_results(call)[0]
}

/// Emit a call to `otter_convert(x, from, to)`.
fn call_convert(b: &mut FunctionBuilder, x: ClValue, from: NumTy, to: NumTy) -> ClValue {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(types::I32));
    sig.returns.push(AbiParam::new(types::I64));
    let sigref = b.import_signature(sig);
    let fc = b.ins().iconst(types::I32, i64::from(from.tag()));
    let tc = b.ins().iconst(types::I32, i64::from(to.tag()));
    let addr = b.ins().iconst(types::I64, convert_addr());
    let call = b.ins().call_indirect(sigref, addr, &[x, fc, tc]);
    b.inst_results(call)[0]
}

/// Emit a call to a baked host fn `f(x)` (`extern "C" fn(i64) -> i64`). The address is taken from the
/// fn pointer carried in the IR — no host table is threaded; the runtime builds the bytecode with the
/// pointer already resolved, so the JIT just bakes its address as an immediate.
fn call_host1(b: &mut FunctionBuilder, f: otter::HostFn1, x: ClValue) -> ClValue {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let sigref = b.import_signature(sig);
    let addr = b.ins().iconst(types::I64, f as usize as i64);
    let call = b.ins().call_indirect(sigref, addr, &[x]);
    b.inst_results(call)[0]
}

/// Emit a call to a baked host fn `f(x, y)` (`extern "C" fn(i64, i64) -> i64`). See [`call_host1`].
fn call_host2(b: &mut FunctionBuilder, f: otter::HostFn2, x: ClValue, y: ClValue) -> ClValue {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let sigref = b.import_signature(sig);
    let addr = b.ins().iconst(types::I64, f as usize as i64);
    let call = b.ins().call_indirect(sigref, addr, &[x, y]);
    b.inst_results(call)[0]
}

/// Emit a call to a baked host-with-context fn `f(host_ctx, x)` (`extern "C" fn(i64, i64) -> i64`) — the
/// [`otter::Expr::HostCtx1`] sibling of [`call_host1`]. `host_ctx` here is [`otter::Module::host_ctx`], a
/// per-module SSA value baked as a compile-time immediate at the call site (see `lower_module_func`) —
/// NOT `opaque_ptr` (the engine's own bookkeeping pointer for Dispatch-mode nested calls; see
/// [`otter::HostFnCtx1`]'s doc for why these two are deliberately different words, never conflated).
fn call_host_ctx1(b: &mut FunctionBuilder, f: otter::HostFnCtx1, host_ctx: ClValue, x: ClValue) -> ClValue {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // host_ctx
    sig.params.push(AbiParam::new(types::I64)); // x
    sig.returns.push(AbiParam::new(types::I64));
    let sigref = b.import_signature(sig);
    let addr = b.ins().iconst(types::I64, f as usize as i64);
    let call = b.ins().call_indirect(sigref, addr, &[host_ctx, x]);
    b.inst_results(call)[0]
}

/// Emit a call to a baked host-with-context fn `f(host_ctx, x, y)`. See [`call_host_ctx1`].
fn call_host_ctx2(
    b: &mut FunctionBuilder,
    f: otter::HostFnCtx2,
    host_ctx: ClValue,
    x: ClValue,
    y: ClValue,
) -> ClValue {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // host_ctx
    sig.params.push(AbiParam::new(types::I64)); // x
    sig.params.push(AbiParam::new(types::I64)); // y
    sig.returns.push(AbiParam::new(types::I64));
    let sigref = b.import_signature(sig);
    let addr = b.ins().iconst(types::I64, f as usize as i64);
    let call = b.ins().call_indirect(sigref, addr, &[host_ctx, x, y]);
    b.inst_results(call)[0]
}

fn new_module() -> JITModule {
    let builder =
        JITBuilder::new(cranelift_module::default_libcall_names()).expect("host ISA should be supported");
    JITModule::new(builder)
}

/// Does this opcode's semantics transfer control (end a basic block)? Derived from the IR + hint.
fn is_terminator(isa: &Isa, opcode: u16) -> bool {
    let op = isa.op(opcode);
    let by_effect = op.effects.iter().any(|e| {
        matches!(
            e,
            Effect::JumpTo { .. }
                | Effect::JumpIf { .. }
                | Effect::Halt { .. }
                | Effect::Return { .. }
                | Effect::TailCall { .. }
        )
    });
    debug_assert_eq!(
        by_effect, op.hints.terminator,
        "terminator hint must match effects ({})",
        op.name
    );
    by_effect
}

/// Collect the branch target pcs an instruction can jump to (resolved from its operand slots).
fn branch_targets(isa: &Isa, inst: &Inst) -> Vec<usize> {
    let mut t = Vec::new();
    for e in &isa.op(inst.op).effects {
        match e {
            Effect::JumpTo { target } | Effect::JumpIf { target, .. } => {
                t.push(inst.operand(*target) as usize);
            }
            _ => {}
        }
    }
    t
}

/// `i64` word -> `F64` (reinterpret bits).
fn word_to_f64(b: &mut FunctionBuilder, w: ClValue) -> ClValue {
    b.ins().bitcast(types::F64, MemFlags::new(), w)
}
/// `F64` -> `i64` word (reinterpret bits).
fn f64_to_word(b: &mut FunctionBuilder, f: ClValue) -> ClValue {
    b.ins().bitcast(types::I64, MemFlags::new(), f)
}
/// low 32 bits of the word -> `F32`.
fn word_to_f32(b: &mut FunctionBuilder, w: ClValue) -> ClValue {
    let lo = b.ins().ireduce(types::I32, w);
    b.ins().bitcast(types::F32, MemFlags::new(), lo)
}
/// `F32` -> `i64` word (zero-extend the 32-bit pattern).
fn f32_to_word(b: &mut FunctionBuilder, f: ClValue) -> ClValue {
    let bits = b.ins().bitcast(types::I32, MemFlags::new(), f);
    b.ins().uextend(types::I64, bits)
}

/// Cranelift integer type for a narrow `NumTy` (or `I64` for 64-bit).
fn int_clty(ty: NumTy) -> types::Type {
    match ty.bits() {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        _ => types::I64,
    }
}

/// Re-normalize an `I64` value to `ty`'s canonical slot form (truncate to width + sign/zero-extend).
fn norm_int(b: &mut FunctionBuilder, ty: NumTy, v: ClValue) -> ClValue {
    let it = int_clty(ty);
    if it == types::I64 {
        return v;
    }
    let r = b.ins().ireduce(it, v);
    if ty.is_signed_int() {
        b.ins().sextend(types::I64, r)
    } else {
        b.ins().uextend(types::I64, r)
    }
}

/// Clamp an in-`I64` value to `ty`'s representable range (the per-width saturation that Rust's float→int
/// `as` performs). For 64-bit types this is a no-op (the `*_sat` conversion already saturated).
fn clamp_to_width(b: &mut FunctionBuilder, ty: NumTy, v: ClValue) -> ClValue {
    let w = ty.bits();
    if w == 64 {
        return v;
    }
    if ty.is_signed_int() {
        let hi = b.ins().iconst(types::I64, (1i64 << (w - 1)) - 1);
        let lo = b.ins().iconst(types::I64, -(1i64 << (w - 1)));
        let v = b.ins().smin(v, hi);
        b.ins().smax(v, lo)
    } else {
        let hi = b.ins().iconst(types::I64, ((1u64 << w) - 1) as i64);
        b.ins().umin(v, hi)
    }
}

fn convert_cl(b: &mut FunctionBuilder, x: ClValue, from: NumTy, to: NumTy) -> ClValue {
    // f16 has no native Cranelift lowering here → route through the canonical helper (bit-identical to
    // the interpreter and across the half/f16-intrinsic configs).
    if from == NumTy::F16 || to == NumTy::F16 {
        return call_convert(b, x, from, to);
    }
    if from.is_float() {
        if to.is_float() {
            return match (from, to) {
                (NumTy::F32, NumTy::F64) => {
                    let f = word_to_f32(b, x);
                    let p = b.ins().fpromote(types::F64, f);
                    f64_to_word(b, p)
                }
                (NumTy::F64, NumTy::F32) => {
                    let f = word_to_f64(b, x);
                    let d = b.ins().fdemote(types::F32, f);
                    f32_to_word(b, d)
                }
                _ => x, // same float type
            };
        }
        // float -> int (saturating, round toward zero — matches Rust `as`). x64 only lowers the
        // I64-dest `*_sat` ops, so saturate to I64 then clamp to the target width.
        let f = if from == NumTy::F32 {
            word_to_f32(b, x)
        } else {
            word_to_f64(b, x)
        };
        let r = if to.is_signed_int() {
            b.ins().fcvt_to_sint_sat(types::I64, f)
        } else {
            b.ins().fcvt_to_uint_sat(types::I64, f)
        };
        return clamp_to_width(b, to, r);
    }
    if to.is_float() {
        // int -> float (x already holds the normalized from-value)
        let ff = if to == NumTy::F32 { types::F32 } else { types::F64 };
        let f = if from.is_signed_int() {
            b.ins().fcvt_from_sint(ff, x)
        } else {
            b.ins().fcvt_from_uint(ff, x)
        };
        return if to == NumTy::F32 {
            f32_to_word(b, f)
        } else {
            f64_to_word(b, f)
        };
    }
    // int -> int
    norm_int(b, to, x)
}

/// All otter locals are raw `i64` words; `lower_expr` always yields a normalized `I64` value. Narrow
/// and float ops `ireduce`/`bitcast` into the right Cranelift type, compute, and re-normalize. `host_ctx`
/// is the SSA value threaded to any [`Expr::HostCtx1`]/[`Expr::HostCtx2`] — see [`call_host_ctx1`]'s doc
/// for why this is a DIFFERENT value than `opaque_ptr` (the Dispatch-mode engine pointer), even though
/// both are `I64`-typed SSA values live in the same function.
fn lower_expr(
    b: &mut FunctionBuilder,
    inst: &Inst,
    vars: &[Variable],
    host_ctx: ClValue,
    e: &Expr,
) -> ClValue {
    match e {
        Expr::Imm(v) => b.ins().iconst(types::I64, *v),
        Expr::ImmF64(f) => b.ins().iconst(types::I64, f.to_bits() as i64),
        Expr::OperandImm(i) => b.ins().iconst(types::I64, inst.operand(*i) as i64),
        Expr::LocalAt(i) => b.use_var(vars[inst.operand(*i) as usize]),
        Expr::Bin(op, a, c) => {
            let x = lower_expr(b, inst, vars, host_ctx, a);
            let y = lower_expr(b, inst, vars, host_ctx, c);
            match op.ty {
                // f16 arithmetic → canonical helper (no native f16 lowering on x64 here)
                NumTy::F16 => call_binop(b, x, y, op.ty, op.op),
                NumTy::F64 => {
                    let (fx, fy) = (word_to_f64(b, x), word_to_f64(b, y));
                    match op.op {
                        ArithOp::Add => {
                            let r = b.ins().fadd(fx, fy);
                            f64_to_word(b, r)
                        }
                        ArithOp::Sub => {
                            let r = b.ins().fsub(fx, fy);
                            f64_to_word(b, r)
                        }
                        ArithOp::Mul => {
                            let r = b.ins().fmul(fx, fy);
                            f64_to_word(b, r)
                        }
                        ArithOp::Div => {
                            let r = b.ins().fdiv(fx, fy);
                            f64_to_word(b, r)
                        }
                        ArithOp::Lt => {
                            let c = b.ins().fcmp(FloatCC::LessThan, fx, fy);
                            b.ins().uextend(types::I64, c)
                        }
                        ArithOp::Eq => {
                            let c = b.ins().fcmp(FloatCC::Equal, fx, fy);
                            b.ins().uextend(types::I64, c)
                        }
                    }
                }
                NumTy::F32 => {
                    let (fx, fy) = (word_to_f32(b, x), word_to_f32(b, y));
                    match op.op {
                        ArithOp::Add => {
                            let r = b.ins().fadd(fx, fy);
                            f32_to_word(b, r)
                        }
                        ArithOp::Sub => {
                            let r = b.ins().fsub(fx, fy);
                            f32_to_word(b, r)
                        }
                        ArithOp::Mul => {
                            let r = b.ins().fmul(fx, fy);
                            f32_to_word(b, r)
                        }
                        ArithOp::Div => {
                            let r = b.ins().fdiv(fx, fy);
                            f32_to_word(b, r)
                        }
                        ArithOp::Lt => {
                            let c = b.ins().fcmp(FloatCC::LessThan, fx, fy);
                            b.ins().uextend(types::I64, c)
                        }
                        ArithOp::Eq => {
                            let c = b.ins().fcmp(FloatCC::Equal, fx, fy);
                            b.ins().uextend(types::I64, c)
                        }
                    }
                }
                _ => match op.op {
                    ArithOp::Add => {
                        let r = b.ins().iadd(x, y);
                        norm_int(b, op.ty, r)
                    }
                    ArithOp::Sub => {
                        let r = b.ins().isub(x, y);
                        norm_int(b, op.ty, r)
                    }
                    ArithOp::Mul => {
                        let r = b.ins().imul(x, y);
                        norm_int(b, op.ty, r)
                    }
                    ArithOp::Lt => {
                        let cc = if op.ty.is_signed_int() {
                            IntCC::SignedLessThan
                        } else {
                            IntCC::UnsignedLessThan
                        };
                        let c = b.ins().icmp(cc, x, y);
                        b.ins().uextend(types::I64, c)
                    }
                    ArithOp::Eq => {
                        let c = b.ins().icmp(IntCC::Equal, x, y);
                        b.ins().uextend(types::I64, c)
                    }
                    ArithOp::Div => unreachable!("integer division not yet supported"),
                },
            }
        }
        Expr::Un(op, a) => {
            let x = lower_expr(b, inst, vars, host_ctx, a);
            match op {
                UnOp::Neg(ty) => match ty {
                    NumTy::F64 => {
                        let fx = word_to_f64(b, x);
                        let r = b.ins().fneg(fx);
                        f64_to_word(b, r)
                    }
                    NumTy::F32 => {
                        let fx = word_to_f32(b, x);
                        let r = b.ins().fneg(fx);
                        f32_to_word(b, r)
                    }
                    // W-0.5 (O-13 design §4.1 ②): IEEE binary16 sign flip, bit-identical to the
                    // canonical `UnOp::eval` arm `((a as u16) ^ 0x8000) as Value`. Computed wholly
                    // in I64 as `(x & 0xFFFF) ^ 0x8000`: the `band` zeroes bits 16..64 — exactly
                    // the `as u16` truncation followed by the `as Value` zero-extension — and the
                    // `bxor` flips only bit 15. A plain 64-bit `x ^ 0x8000` would be identical
                    // ONLY for inputs already in normalized slot form (high 48 bits zero); the
                    // interpreter truncates non-normalized words (arbitrary args/`Imm`s), so the
                    // mask is required for unconditional bit-parity with T0. No `half` softfloat
                    // involved — pure bit manipulation, config-invariant, hence no helper call
                    // needed (unlike F16 Bin/Convert above).
                    NumTy::F16 => {
                        let m = b.ins().band_imm(x, 0xFFFF);
                        b.ins().bxor_imm(m, 0x8000)
                    }
                    _ => {
                        let r = b.ins().ineg(x);
                        norm_int(b, *ty, r)
                    }
                },
                UnOp::Convert { from, to } => convert_cl(b, x, *from, *to),
            }
        }
        Expr::Host1(f, a) => {
            let x = lower_expr(b, inst, vars, host_ctx, a);
            call_host1(b, *f, x)
        }
        Expr::Host2(f, a, c) => {
            let x = lower_expr(b, inst, vars, host_ctx, a);
            let y = lower_expr(b, inst, vars, host_ctx, c);
            call_host2(b, *f, x, y)
        }
        Expr::HostCtx1(f, a) => {
            let x = lower_expr(b, inst, vars, host_ctx, a);
            call_host_ctx1(b, *f, host_ctx, x)
        }
        Expr::HostCtx2(f, a, c) => {
            let x = lower_expr(b, inst, vars, host_ctx, a);
            let y = lower_expr(b, inst, vars, host_ctx, c);
            call_host_ctx2(b, *f, host_ctx, x, y)
        }
    }
}

/// Compile `prog` (under `isa`, `nlocals` registers, `nargs` arguments) to host machine code and return
/// a callable handle. The compiled function has signature `extern "C" fn(*const i64) -> i64`: it loads
/// the first `nargs` locals from the argument pointer, zero-initializes the rest, runs, and returns the
/// `Halt` value.
///
/// **No `Module`, so `host_ctx` is fixed at `0`** for any [`Expr::HostCtx1`]/[`Expr::HostCtx2`] in `prog`
/// — this is the `otter::engine::Engine` (single-function tier-up) compiled path; see
/// [`otter::interp::interpret_from`]'s doc for the matching interpreter-tier decision this mirrors, so
/// `Engine`'s own T0/T1/T2 stay consistent with EACH OTHER (all see `host_ctx = 0`) even though a
/// `Module` executing the identical `Expr` would see a different word.
pub fn compile_fn(isa: &Isa, prog: &[Inst], nlocals: usize, nargs: usize) -> CompiledFn {
    let mut module = new_module();
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // args pointer
    ctx.func.signature.returns.push(AbiParam::new(types::I64));

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);

        // basic-block leaders: entry + every branch target + every fall-through after a terminator
        let mut leader = vec![false; prog.len()];
        if !prog.is_empty() {
            leader[0] = true;
        }
        for (pc, inst) in prog.iter().enumerate() {
            if is_terminator(isa, inst.op) {
                for t in branch_targets(isa, inst) {
                    if t < leader.len() {
                        leader[t] = true;
                    }
                }
                if pc + 1 < leader.len() {
                    leader[pc + 1] = true;
                }
            }
        }
        let mut block_at: HashMap<usize, _> = HashMap::new();
        for (pc, &is_leader) in leader.iter().enumerate() {
            if is_leader {
                block_at.insert(pc, b.create_block());
            }
        }

        // locals -> SSA variables (declare_var returns a fresh Variable in this Cranelift version)
        let vars: Vec<Variable> = (0..nlocals).map(|_| b.declare_var(types::I64)).collect();

        let entry = block_at[&0];
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let args_ptr = b.block_params(entry)[0];
        let zero = b.ins().iconst(types::I64, 0);
        let host_ctx = zero; // no Module here — fixed at 0 (see this fn's doc)
        for (i, v) in vars.iter().enumerate() {
            if i < nargs {
                // load argument i from args_ptr[i] (8-byte slots)
                let val = b
                    .ins()
                    .load(types::I64, MemFlags::trusted(), args_ptr, (i * 8) as i32);
                b.def_var(*v, val);
            } else {
                b.def_var(*v, zero);
            }
        }

        let mut cur_leader = 0usize;
        let mut terminated = false;

        for pc in 0..prog.len() {
            if leader[pc] && pc != cur_leader {
                if !terminated {
                    b.ins().jump(block_at[&pc], &[]);
                }
                b.switch_to_block(block_at[&pc]);
                terminated = false;
                cur_leader = pc;
            }
            let inst = &prog[pc];
            for eff in &isa.op(inst.op).effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        let v = lower_expr(&mut b, inst, &vars, host_ctx, value);
                        b.def_var(vars[inst.operand(*reg) as usize], v);
                    }
                    Effect::JumpTo { target } => {
                        b.ins().jump(block_at[&(inst.operand(*target) as usize)], &[]);
                        terminated = true;
                    }
                    Effect::JumpIf { cond, target } => {
                        let v = lower_expr(&mut b, inst, &vars, host_ctx, cond);
                        let t = inst.operand(*target) as usize;
                        b.ins().brif(v, block_at[&t], &[], block_at[&(pc + 1)], &[]);
                        terminated = true;
                    }
                    Effect::Halt { value } => {
                        let r = lower_expr(&mut b, inst, &vars, host_ctx, value);
                        b.ins().return_(&[r]);
                        terminated = true;
                    }
                    // `compile_fn` does not deopt; a guard is a transparent no-op here. Deopt-capable
                    // code is produced by `compile_resumable`.
                    Effect::Guard { .. } => {}
                    // Function calls run on the interpreter oracle (`run_module`); JIT lowering of
                    // multi-frame calls is a later (risky) increment. `compile_fn` is only ever invoked
                    // on call-free single functions, so this is never reached.
                    Effect::Call { .. } | Effect::Return { .. } | Effect::TailCall { .. } => {
                        unimplemented!(
                            "Call/Return/TailCall lowering is not implemented (single-function tier)"
                        )
                    }
                }
            }
        }
        if !terminated {
            // well-formed programs end in Halt; this is a safety net.
            let r = b.use_var(vars[0]);
            b.ins().return_(&[r]);
        }

        b.seal_all_blocks();
        b.finalize();
    }

    let id = module
        .declare_function("toy", Linkage::Export, &ctx.func.signature)
        .expect("declare");
    module.define_function(id, &mut ctx).expect("define");
    module.clear_context(&mut ctx);
    module.finalize_definitions().expect("finalize");

    let code_ptr = module.get_finalized_function(id);
    CompiledFn {
        module,
        ptr: code_ptr,
        nargs,
    }
}

/// A compiled function: host machine code plus the JIT module that owns it (kept alive for calls).
pub struct CompiledFn {
    // kept alive so the code stays mapped; never read directly.
    #[allow(dead_code)]
    module: JITModule,
    ptr: *const u8,
    nargs: usize,
}

impl CompiledFn {
    /// Call the compiled function with `args` (must provide at least `nargs`). Result equals the
    /// interpreter's for the same program + args (verified by the differential tests).
    pub fn call(&self, args: &[Value]) -> Value {
        assert!(
            args.len() >= self.nargs,
            "compiled fn needs >= {} args",
            self.nargs
        );
        let argp: *const i64 = if args.is_empty() {
            core::ptr::null()
        } else {
            args.as_ptr()
        };
        // SAFETY: compiled with signature `extern "C" fn(*const i64) -> i64`; it reads only the first
        // `nargs` (<= args.len()) slots; `module` is kept alive in `self`.
        let f: extern "C" fn(*const i64) -> i64 = unsafe { core::mem::transmute(self.ptr) };
        f(argp)
    }
}

/// Convenience: compile a no-argument program, run it once, return the result (like `interpret`).
pub fn jit_run(isa: &Isa, prog: &[Inst], nlocals: usize) -> Value {
    compile_fn(isa, prog, nlocals, 0).call(&[])
}

// ---------------------------------------------------------------------------------------------------
// Module (function-call) lowering — the deferred risky half of B1.3.
//
// Every function compiles to the uniform ABI `extern "C" fn(table, args, argc) -> i64`. Inter-function
// calls go through a **read-only function-pointer table** (`call_indirect table[idx]`): the callee index
// is a runtime value (first-class functions), so a static `call` can't be used, and a read-only table
// (vs. a mutable engine context) sidesteps reentrant-`&mut` aliasing. Constants and the `fill` word are
// baked per function; arguments are marshalled into a small stack buffer per call site. No adaptive
// tiering / interpreter fallback yet — `compile_module` compiles the *whole* module at the optimizing
// tier. Verified bit-identical to `interp::run_module` (the oracle).
// ---------------------------------------------------------------------------------------------------

/// The uniform compiled-function ABI: `(table, args, argc) -> result`.
type ModuleEntry = extern "C" fn(*const usize, *const i64, i64) -> Value;

/// A compiled [`otter::Module`]: the JIT memory + the function-pointer table threaded into every call.
pub struct CompiledModule {
    #[allow(dead_code)] // kept alive so the code stays mapped
    module: JITModule,
    table: Box<[usize]>,
    entry: usize,
}

impl CompiledModule {
    /// Run the module's entry function with `args` (initialize its parameters). Returns the value of the
    /// first `Halt`/`Return` past the entry — equal to `interp::run_module` for the same module + args.
    pub fn run(&self, args: &[Value]) -> Value {
        let argp: *const i64 = if args.is_empty() {
            core::ptr::null()
        } else {
            args.as_ptr()
        };
        // SAFETY: every table entry was compiled with the `ModuleEntry` signature; the table + JIT
        // memory are kept alive in `self`; the entry reads only the first `argc` args.
        let f: ModuleEntry = unsafe { core::mem::transmute(self.table[self.entry]) };
        f(self.table.as_ptr(), argp, args.len() as i64)
    }
}

/// Compile an entire [`otter::Module`] (functions + calls) to machine code. See the section comment.
pub fn compile_module(isa: &Isa, m: &OtterModule) -> CompiledModule {
    let mut module = new_module();
    let ptr_ty = module.target_config().pointer_type();
    let conv = module.target_config().default_call_conv;

    let mut abi = Signature::new(conv);
    abi.params.push(AbiParam::new(ptr_ty)); // table
    abi.params.push(AbiParam::new(ptr_ty)); // args
    abi.params.push(AbiParam::new(types::I64)); // argc
    abi.returns.push(AbiParam::new(types::I64));

    // Declare every function up front so the table has an entry per index (recursion / forward refs).
    let ids: Vec<_> = (0..m.funcs.len())
        .map(|i| {
            module
                .declare_function(&alloc_name(i), Linkage::Local, &abi)
                .expect("declare")
        })
        .collect();

    let mut fbctx = FunctionBuilderContext::new();
    for (i, f) in m.funcs.iter().enumerate() {
        let mut ctx = module.make_context();
        ctx.func.signature = abi.clone();
        lower_module_func(
            &mut ctx.func,
            &mut fbctx,
            isa,
            m.fill,
            m.host_ctx,
            f,
            ptr_ty,
            conv,
            CallMode::Table,
        );
        module.define_function(ids[i], &mut ctx).expect("define");
        module.clear_context(&mut ctx);
    }
    module.finalize_definitions().expect("finalize");

    let table: Box<[usize]> = ids
        .iter()
        .map(|id| module.get_finalized_function(*id) as usize)
        .collect();
    CompiledModule {
        module,
        table,
        entry: m.entry,
    }
}

fn alloc_name(i: usize) -> std::string::String {
    std::format!("f{i}")
}

/// A single function compiled for the **adaptive engine**: ABI `extern "C" fn(ctx, args, argc) -> i64`,
/// with each `Call` lowered to `dispatch(ctx, callee_idx, args, argc)` (a baked Rust dispatcher) so the
/// callee runs at its current tier. The raw code pointer is read out and called by the engine's
/// dispatcher; the owning `JITModule` keeps it mapped.
pub struct CompiledDispatchFn {
    #[allow(dead_code)]
    module: JITModule,
    ptr: *const u8,
}

impl CompiledDispatchFn {
    /// The raw code pointer (an `extern "C" fn(*const u8 ctx, *const i64 args, i64 argc) -> i64`).
    pub fn code_ptr(&self) -> *const u8 {
        self.ptr
    }
}

/// Compile function `func_idx` (its `FuncDef` is `f`, and `fill`/`host_ctx` are the owning module's fill
/// and [`otter::Module::host_ctx`] words) for the adaptive engine (T2 tier). `dispatch_addr` is the
/// address of the engine's `extern "C" fn(ctx, callee_idx, args, argc) -> i64` dispatcher, baked into
/// every Call. `tail_addr` is the address of the engine's `tail_signal` trampoline-signal helper (same
/// ABI), baked into every `TailCall` (O-8c — proper tail calls at T2; see
/// `.claude-research-library/otter-o8-tailcall-design.md` §8.1/§8.2).
///
/// Takes `f: &FuncDef` / `fill: Value` directly rather than `&otter::Module` (O-9: `ModuleEngine::push_func`
/// injects functions that never live inside an `otter::Module` at all, so this signature can no longer
/// assume one exists — see the O-9 design doc §1.1 and its panel review §1). `func_idx` is still needed
/// (independently of `f`) purely to give the JIT-internal function symbol a unique name. `host_ctx`
/// (O-11) rides alongside `fill` for exactly the same reason — `ModuleEngine` always compiles a pushed
/// function with its OWN `module.host_ctx` (there is only ever one `Module`/`host_ctx` per engine), so a
/// pushed function transparently inherits the same context word every original function sees.
pub fn compile_dispatch_fn(
    isa: &Isa,
    func_idx: usize,
    f: &FuncDef,
    fill: Value,
    host_ctx: Value,
    dispatch_addr: usize,
    tail_addr: usize,
) -> CompiledDispatchFn {
    let mut module = new_module();
    let ptr_ty = module.target_config().pointer_type();
    let conv = module.target_config().default_call_conv;

    let mut abi = Signature::new(conv);
    abi.params.push(AbiParam::new(ptr_ty)); // ctx
    abi.params.push(AbiParam::new(ptr_ty)); // args
    abi.params.push(AbiParam::new(types::I64)); // argc
    abi.returns.push(AbiParam::new(types::I64));

    let id = module
        .declare_function(&alloc_name(func_idx), Linkage::Local, &abi)
        .expect("declare");
    let mut fbctx = FunctionBuilderContext::new();
    let mut ctx = module.make_context();
    ctx.func.signature = abi.clone();
    lower_module_func(
        &mut ctx.func,
        &mut fbctx,
        isa,
        fill,
        host_ctx,
        f,
        ptr_ty,
        conv,
        CallMode::Dispatch(dispatch_addr, tail_addr),
    );
    module.define_function(id, &mut ctx).expect("define");
    module.clear_context(&mut ctx);
    module.finalize_definitions().expect("finalize");
    let ptr = module.get_finalized_function(id);
    CompiledDispatchFn { module, ptr }
}

/// Lower one [`FuncDef`] to a Cranelift function with the module ABI.
/// How a compiled function lowers its `Call`s (and, for `Dispatch`, its `TailCall`s). `Table` =
/// `call_indirect` through a read-only function-pointer table (AOT whole-module compile — param 0 is the
/// table); `TailCall` is NOT lowered in this mode (out of scope — design §8.4/§8.8 pick C3: a separate
/// static AOT product, not the `ModuleEngine`). `Dispatch(dispatch_addr, tail_addr)` = call a baked Rust
/// dispatcher `dispatch(ctx, callee_idx, args, argc)` for `Call`, or the `tail_signal` trampoline-signal
/// helper (same ABI) for `TailCall`, so the callee runs at its *current* tier (the adaptive engine —
/// param 0 is the opaque `ctx`).
#[derive(Clone, Copy)]
enum CallMode {
    Table,
    Dispatch(usize, usize),
}

/// `fill`/`host_ctx` replace a `&OtterModule` parameter (O-9; O-11 extends it): the ONLY things this
/// function ever read off a module were `m.fill` and (as of O-11) `m.host_ctx` (`f` — the function
/// itself — was already passed separately), so two bare `Value`s are enough and let
/// [`compile_dispatch_fn`] compile a [`FuncDef`] that was never part of any `otter::Module` (an O-9
/// `ModuleEngine::push_func`-injected function) without fabricating one.
#[allow(clippy::too_many_arguments)] // a focused internal lowering helper; grouping would obscure it
fn lower_module_func(
    func: &mut Function,
    fbctx: &mut FunctionBuilderContext,
    isa: &Isa,
    fill: Value,
    host_ctx: Value,
    f: &FuncDef,
    ptr_ty: Type,
    conv: CallConv,
    call_mode: CallMode,
) {
    let prog = &f.code;
    let mut b = FunctionBuilder::new(func, fbctx);

    // entry first (it becomes the layout entry block carrying the function params)
    let entry = b.create_block();

    // basic-block leaders for the body (same derivation as `compile_fn`)
    let mut leader = vec![false; prog.len()];
    if !prog.is_empty() {
        leader[0] = true;
    }
    for (pc, inst) in prog.iter().enumerate() {
        if is_terminator(isa, inst.op) {
            for t in branch_targets(isa, inst) {
                if t < leader.len() {
                    leader[t] = true;
                }
            }
            if pc + 1 < leader.len() {
                leader[pc + 1] = true;
            }
        }
    }
    let mut block_at: HashMap<usize, _> = HashMap::new();
    for (pc, &is_leader) in leader.iter().enumerate() {
        if is_leader {
            block_at.insert(pc, b.create_block());
        }
    }

    let vars: Vec<Variable> = (0..f.nlocals).map(|_| b.declare_var(types::I64)).collect();

    // --- prologue: init the frame ---
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    // param 0 is an opaque pointer threaded through every call: the fn-ptr table (`Table` mode) or the
    // engine `ctx` (`Dispatch` mode). Compiled code only forwards it; it never dereferences the engine.
    //
    // NOTE (O-11): `opaque_ptr` is UNRELATED to `host_ctx` below, despite both being live `I64` SSA
    // values in this same function — `opaque_ptr` is the engine's own bookkeeping pointer (Dispatch mode)
    // or the fn-ptr table address (Table mode), forwarded to `dispatch`/`tail_signal`/`table[callee]`
    // lookups and NEVER passed to a host builtin; `host_ctx` is the embedder-owned
    // `otter::Module::host_ctx` word, passed ONLY to `HostCtx1`/`HostCtx2` calls (`call_host_ctx1`/`2`)
    // and never to `dispatch`/`table[callee]`. Do not swap them at a call site.
    let opaque_ptr = b.block_params(entry)[0];
    let args_ptr = b.block_params(entry)[1];
    let argc = b.block_params(entry)[2];

    let fill = b.ins().iconst(types::I64, fill);
    let host_ctx = b.ins().iconst(types::I64, host_ctx); // per-Module compile-time constant (never a live register)
    for v in &vars {
        b.def_var(*v, fill); // every local starts as `fill` (the embedder's "undefined")
    }
    // copy parameters, each guarded by `argc` so an absent argument stays `fill` (and is never loaded)
    #[allow(clippy::needless_range_loop)] // `i` is also a value (the arg offset / compared index)
    for i in 0..f.nparams {
        let load_blk = b.create_block();
        let cont_blk = b.create_block();
        let iv = b.ins().iconst(types::I64, i as i64);
        let cmp = b.ins().icmp(IntCC::SignedGreaterThan, argc, iv); // i < argc
        b.ins().brif(cmp, load_blk, &[], cont_blk, &[]);
        b.switch_to_block(load_blk);
        let v = b
            .ins()
            .load(types::I64, MemFlags::trusted(), args_ptr, (i * 8) as i32);
        b.def_var(vars[i], v);
        b.ins().jump(cont_blk, &[]);
        b.switch_to_block(cont_blk);
    }
    // bake the constant pool into its slots
    for (j, c) in f.consts.iter().enumerate() {
        let cv = b.ins().iconst(types::I64, *c);
        b.def_var(vars[f.nparams + j], cv);
    }

    let Some(&blk0) = block_at.get(&0) else {
        b.ins().return_(&[fill]); // empty body (shouldn't happen; well-formed funcs end in Halt/Return)
        b.seal_all_blocks();
        b.finalize();
        return;
    };
    b.ins().jump(blk0, &[]);

    // signature for the indirect call at each Call site (depends on the call mode)
    let mut callsig = Signature::new(conv);
    match call_mode {
        CallMode::Table => {
            callsig.params.push(AbiParam::new(ptr_ty)); // table
            callsig.params.push(AbiParam::new(ptr_ty)); // args
            callsig.params.push(AbiParam::new(types::I64)); // argc
        }
        CallMode::Dispatch(..) => {
            callsig.params.push(AbiParam::new(ptr_ty)); // ctx
            callsig.params.push(AbiParam::new(types::I64)); // callee idx
            callsig.params.push(AbiParam::new(ptr_ty)); // args
            callsig.params.push(AbiParam::new(types::I64)); // argc
        }
    }
    callsig.returns.push(AbiParam::new(types::I64));
    let callsigref = b.import_signature(callsig);

    // --- body ---
    b.switch_to_block(blk0);
    let mut cur_leader = 0usize;
    let mut terminated = false;
    for pc in 0..prog.len() {
        if leader[pc] && pc != cur_leader {
            if !terminated {
                b.ins().jump(block_at[&pc], &[]);
            }
            b.switch_to_block(block_at[&pc]);
            terminated = false;
            cur_leader = pc;
        }
        let inst = &prog[pc];
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::SetLocalAt { reg, value } => {
                    let v = lower_expr(&mut b, inst, &vars, host_ctx, value);
                    b.def_var(vars[inst.operand(*reg) as usize], v);
                }
                Effect::JumpTo { target } => {
                    b.ins().jump(block_at[&(inst.operand(*target) as usize)], &[]);
                    terminated = true;
                }
                Effect::JumpIf { cond, target } => {
                    let v = lower_expr(&mut b, inst, &vars, host_ctx, cond);
                    let t = inst.operand(*target) as usize;
                    b.ins().brif(v, block_at[&t], &[], block_at[&(pc + 1)], &[]);
                    terminated = true;
                }
                // Halt ends the module; Return ends a frame — both lower to a machine return here, since
                // each function is its own native frame and the caller resumes after the `call_indirect`.
                Effect::Halt { value } | Effect::Return { value } => {
                    let r = lower_expr(&mut b, inst, &vars, host_ctx, value);
                    b.ins().return_(&[r]);
                    terminated = true;
                }
                Effect::Guard { .. } => {}
                // O-8c: lower to the dispatcher-trampoline signal — marshal args exactly like `Call`
                // below, but call the `tail_signal` helper instead of `dispatch`, discard its result, and
                // `return_` the function's own `fill` immediately (no `dest` write: this IS a terminator,
                // and the callee's eventual result becomes THIS function's return value once the engine's
                // trampoline loop re-enters it — design §8.1/§8.2). Only `Dispatch` mode has a `tail_addr`
                // to call through; `Table` mode (AOT whole-module) does not lower `TailCall` (§8.4/§8.8 C3
                // — a separate static product, not the `ModuleEngine`; a known, documented gap).
                Effect::TailCall { base, argc: argc_op } => {
                    let base_slot = inst.operand(*base) as usize;
                    let n = inst.operand(*argc_op) as usize;
                    // marshal args locals[base+1 .. base+1+n] into a contiguous stack buffer (identical
                    // to `Call`'s marshaling below — same call-site shape, different target/tail).
                    let argbuf = if n > 0 {
                        let slot = b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            (n * 8) as u32,
                            3,
                        ));
                        #[allow(clippy::needless_range_loop)] // `k` is also the stack-buffer offset
                        for k in 0..n {
                            let av = b.use_var(vars[base_slot + 1 + k]);
                            b.ins().stack_store(av, slot, (k * 8) as i32);
                        }
                        b.ins().stack_addr(ptr_ty, slot, 0)
                    } else {
                        b.ins().iconst(ptr_ty, 0)
                    };
                    let callee = b.use_var(vars[base_slot]); // raw function index
                    let nv = b.ins().iconst(types::I64, n as i64);
                    match call_mode {
                        CallMode::Table => unimplemented!(
                            "TailCall is not lowered in Table (whole-module AOT) mode — the ModuleEngine \
                             (Dispatch mode) is the only O-8c target; see design §8.4/§8.8 pick C3"
                        ),
                        CallMode::Dispatch(_dispatch_addr, tail_addr) => {
                            // tail_signal(ctx, callee_idx, argbuf, n) — same ABI/call shape as `dispatch`
                            // (reuses `callsigref`); its return value is discarded (`tail_pending`, an
                            // engine-side flag, is the discriminator, never the value — design §8.1).
                            let tptr = b.ins().iconst(ptr_ty, tail_addr as i64);
                            let call =
                                b.ins()
                                    .call_indirect(callsigref, tptr, &[opaque_ptr, callee, argbuf, nv]);
                            let _ = b.inst_results(call); // discarded on purpose
                        }
                    }
                    b.ins().return_(&[fill]);
                    terminated = true;
                }
                Effect::Call {
                    dest,
                    base,
                    argc: argc_op,
                } => {
                    let dest_slot = inst.operand(*dest) as usize;
                    let base_slot = inst.operand(*base) as usize;
                    let n = inst.operand(*argc_op) as usize;
                    // marshal args locals[base+1 .. base+1+n] into a contiguous stack buffer
                    let argbuf = if n > 0 {
                        let slot = b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            (n * 8) as u32,
                            3,
                        ));
                        #[allow(clippy::needless_range_loop)] // `k` is also the stack-buffer offset
                        for k in 0..n {
                            let av = b.use_var(vars[base_slot + 1 + k]);
                            b.ins().stack_store(av, slot, (k * 8) as i32);
                        }
                        b.ins().stack_addr(ptr_ty, slot, 0)
                    } else {
                        b.ins().iconst(ptr_ty, 0)
                    };
                    let callee = b.use_var(vars[base_slot]); // raw function index
                    let nv = b.ins().iconst(types::I64, n as i64);
                    let call = match call_mode {
                        CallMode::Table => {
                            // fnptr = table[callee]; fnptr(table, argbuf, n)
                            let off = b.ins().imul_imm(callee, 8);
                            let addr = b.ins().iadd(opaque_ptr, off);
                            let fnptr = b.ins().load(ptr_ty, MemFlags::trusted(), addr, 0);
                            b.ins()
                                .call_indirect(callsigref, fnptr, &[opaque_ptr, argbuf, nv])
                        }
                        CallMode::Dispatch(addr, _tail_addr) => {
                            // dispatch(ctx, callee_idx, argbuf, n) — routes to the callee's current tier
                            let dptr = b.ins().iconst(ptr_ty, addr as i64);
                            b.ins()
                                .call_indirect(callsigref, dptr, &[opaque_ptr, callee, argbuf, nv])
                        }
                    };
                    let res = b.inst_results(call)[0];
                    b.def_var(vars[dest_slot], res);
                }
            }
        }
    }
    if !terminated {
        let r = b.use_var(vars[0]);
        b.ins().return_(&[r]);
    }
    b.seal_all_blocks();
    b.finalize();
}

impl otter::engine::Code for CompiledFn {
    fn run(&self, args: &[Value]) -> Value {
        self.call(args)
    }
}

/// The Cranelift JIT as an [`otter::engine::Backend`] — the optimizing tier (T2) of the tier-up engine.
pub struct CraneliftBackend;

impl otter::engine::Backend for CraneliftBackend {
    fn name(&self) -> &'static str {
        "cranelift"
    }
    fn compile(
        &self,
        isa: &Isa,
        prog: &[Inst],
        nlocals: usize,
        nargs: usize,
    ) -> Box<dyn otter::engine::Code> {
        Box::new(compile_fn(isa, prog, nlocals, nargs))
    }
}

// ===================================================================================================
// Resumable (OSR / deopt-capable) compilation.
//
// A resumable function operates on a caller-owned **frame** (the live `locals` array) instead of an
// args pointer, so the same code can be *entered* mid-function (OSR) and can *exit* mid-function
// (deopt). ABI: `extern "C" fn(frame: *mut i64, deopt_out: *mut i64) -> i64`.
//   * entry: load all locals from `frame`, jump to the baked `entry_pc`'s block.
//   * normal `Halt`: write `*deopt_out = -1`, return the halt value.
//   * `Guard{cond}` fails: write the live locals back to `frame`, write `*deopt_out = pc`, return.
// The caller resolves a deopt by resuming the interpreter at that pc with the (written-back) frame.
// ===================================================================================================

/// A resumable-compiled function (frame ABI). Entry pc is baked in at compile time.
pub struct ResumableCode {
    #[allow(dead_code)]
    module: JITModule,
    ptr: *const u8,
    nlocals: usize,
}

impl otter::engine::Resumable for ResumableCode {
    /// Run with `frame` as the live locals (length must be `>= nlocals`).
    fn run_frame(&self, frame: &mut [Value]) -> Run {
        assert!(frame.len() >= self.nlocals, "frame shorter than nlocals");
        let mut deopt: i64 = -1;
        // SAFETY: compiled with signature `extern "C" fn(*mut i64, *mut i64) -> i64`; it touches only
        // frame[0..nlocals] and `deopt`; `module` is kept alive in `self`.
        let f: extern "C" fn(*mut i64, *mut i64) -> i64 = unsafe { core::mem::transmute(self.ptr) };
        let r = f(frame.as_mut_ptr(), &mut deopt as *mut i64);
        if deopt < 0 {
            Run::Completed(r)
        } else {
            Run::Deopt(deopt as usize)
        }
    }
}

/// Compile `prog` as a resumable function entering at `entry_pc` (must be a basic-block leader; pc 0
/// and any loop header qualify). The function loads its whole frame at entry and can deopt on a guard.
///
/// **No `Module`, so `host_ctx` is fixed at `0`** for any [`Expr::HostCtx1`]/[`Expr::HostCtx2`] in
/// `prog` — same decision, and the same reasoning, as [`compile_fn`]'s doc.
pub fn compile_resumable(isa: &Isa, prog: &[Inst], nlocals: usize, entry_pc: usize) -> ResumableCode {
    let mut module = new_module();
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // frame
    ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // deopt_out
    ctx.func.signature.returns.push(AbiParam::new(types::I64));

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);

        // entry block first (so it's the function's parameter/entry block in layout)
        let entry = b.create_block();

        // basic-block leaders (same rule as compile_fn) + force entry_pc to be one
        let mut leader = vec![false; prog.len()];
        if !prog.is_empty() {
            leader[0] = true;
        }
        for (pc, inst) in prog.iter().enumerate() {
            if is_terminator(isa, inst.op) {
                for t in branch_targets(isa, inst) {
                    if t < leader.len() {
                        leader[t] = true;
                    }
                }
                if pc + 1 < leader.len() {
                    leader[pc + 1] = true;
                }
            }
        }
        if entry_pc < leader.len() {
            leader[entry_pc] = true;
        }
        let mut block_at: HashMap<usize, _> = HashMap::new();
        for (pc, &is_leader) in leader.iter().enumerate() {
            if is_leader {
                block_at.insert(pc, b.create_block());
            }
        }

        let vars: Vec<Variable> = (0..nlocals).map(|_| b.declare_var(types::I64)).collect();
        let frame_var = b.declare_var(ptr_ty);
        let deopt_var = b.declare_var(ptr_ty);

        // entry: bind params, load the whole frame into SSA locals, jump to entry_pc's block
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let frame_p = b.block_params(entry)[0];
        let deopt_p = b.block_params(entry)[1];
        b.def_var(frame_var, frame_p);
        b.def_var(deopt_var, deopt_p);
        let host_ctx = b.ins().iconst(types::I64, 0); // no Module here — fixed at 0 (see this fn's doc)
        for (i, v) in vars.iter().enumerate() {
            let val = b
                .ins()
                .load(types::I64, MemFlags::trusted(), frame_p, (i * 8) as i32);
            b.def_var(*v, val);
        }
        b.ins().jump(block_at[&entry_pc], &[]);

        // helper to write all live locals back to the frame (used on deopt)
        let mut cur_leader = usize::MAX;
        let mut terminated = true; // current block (entry) is terminated by the jump above

        for pc in 0..prog.len() {
            if leader[pc] && pc != cur_leader {
                if !terminated {
                    b.ins().jump(block_at[&pc], &[]);
                }
                b.switch_to_block(block_at[&pc]);
                terminated = false;
                cur_leader = pc;
            }
            let inst = &prog[pc];
            for eff in &isa.op(inst.op).effects {
                match eff {
                    Effect::SetLocalAt { reg, value } => {
                        let v = lower_expr(&mut b, inst, &vars, host_ctx, value);
                        b.def_var(vars[inst.operand(*reg) as usize], v);
                    }
                    Effect::JumpTo { target } => {
                        b.ins().jump(block_at[&(inst.operand(*target) as usize)], &[]);
                        terminated = true;
                    }
                    Effect::JumpIf { cond, target } => {
                        let v = lower_expr(&mut b, inst, &vars, host_ctx, cond);
                        let t = inst.operand(*target) as usize;
                        b.ins().brif(v, block_at[&t], &[], block_at[&(pc + 1)], &[]);
                        terminated = true;
                    }
                    Effect::Halt { value } => {
                        let r = lower_expr(&mut b, inst, &vars, host_ctx, value);
                        let dp = b.use_var(deopt_var);
                        let neg1 = b.ins().iconst(types::I64, -1);
                        b.ins().store(MemFlags::trusted(), neg1, dp, 0); // no deopt
                        b.ins().return_(&[r]);
                        terminated = true;
                    }
                    Effect::Guard { cond } => {
                        let c = lower_expr(&mut b, inst, &vars, host_ctx, cond);
                        let deopt_block = b.create_block();
                        let post = b.create_block();
                        b.ins().brif(c, post, &[], deopt_block, &[]);
                        // deopt: spill live locals to the frame, record the pc, return
                        b.switch_to_block(deopt_block);
                        let fp = b.use_var(frame_var);
                        for (i, v) in vars.iter().enumerate() {
                            let val = b.use_var(*v);
                            b.ins().store(MemFlags::trusted(), val, fp, (i * 8) as i32);
                        }
                        let dp = b.use_var(deopt_var);
                        let pcv = b.ins().iconst(types::I64, pc as i64);
                        b.ins().store(MemFlags::trusted(), pcv, dp, 0);
                        let zero = b.ins().iconst(types::I64, 0);
                        b.ins().return_(&[zero]);
                        // continue (guard held) in the post block
                        b.switch_to_block(post);
                        terminated = false;
                    }
                    Effect::Call { .. } | Effect::Return { .. } | Effect::TailCall { .. } => {
                        unimplemented!(
                            "Call/Return/TailCall lowering is not implemented (single-function tier)"
                        )
                    }
                }
            }
        }
        if !terminated {
            let dp = b.use_var(deopt_var);
            let neg1 = b.ins().iconst(types::I64, -1);
            b.ins().store(MemFlags::trusted(), neg1, dp, 0);
            let r = b.use_var(vars[0]);
            b.ins().return_(&[r]);
        }

        b.seal_all_blocks();
        b.finalize();
    }

    let id = module
        .declare_function("toy_resumable", Linkage::Export, &ctx.func.signature)
        .expect("declare");
    module.define_function(id, &mut ctx).expect("define");
    module.clear_context(&mut ctx);
    module.finalize_definitions().expect("finalize");

    let ptr = module.get_finalized_function(id);
    ResumableCode { module, ptr, nlocals }
}

/// A normal call through the optimizing tier, but **deopt-capable**: compile entering at pc 0, run with
/// `args`, and if a guard fails, finish in the interpreter (the oracle). Result is always correct.
pub fn run_resumable(isa: &Isa, prog: &[Inst], nlocals: usize, args: &[Value]) -> Value {
    let code = compile_resumable(isa, prog, nlocals, 0);
    let mut frame = vec![0 as Value; nlocals];
    for (i, &a) in args.iter().enumerate() {
        frame[i] = a;
    }
    match code.run_frame(&mut frame) {
        Run::Completed(v) => v,
        Run::Deopt(pc) => otter::interp::interpret_from(isa, prog, &mut frame, pc),
    }
}

/// **OSR**: continue execution in compiled code, entering at `entry_pc` with the live `frame`; resolve
/// any later deopt via the interpreter.
pub fn osr_continue(isa: &Isa, prog: &[Inst], nlocals: usize, frame: &mut [Value], entry_pc: usize) -> Value {
    let code = compile_resumable(isa, prog, nlocals, entry_pc);
    match code.run_frame(frame) {
        Run::Completed(v) => v,
        Run::Deopt(pc) => otter::interp::interpret_from(isa, prog, frame, pc),
    }
}

/// The resumable Cranelift backend as an [`otter::engine::OsrBackend`] — lets the engine OSR a hot loop
/// (and the resumable code deopt) automatically.
pub struct ResumableCraneliftBackend;

impl OsrBackend for ResumableCraneliftBackend {
    fn name(&self) -> &'static str {
        "cranelift-osr"
    }
    fn compile_resumable(
        &self,
        isa: &Isa,
        prog: &[Inst],
        nlocals: usize,
        entry_pc: usize,
    ) -> Box<dyn Resumable> {
        Box::new(compile_resumable(isa, prog, nlocals, entry_pc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otter::interp::{interpret, interpret_with};
    use otter::sample::{self, ADD, HALT, LT, MOV_IMM, MUL, SUB};

    /// New codegen path (function arguments loaded from a pointer at entry) — differential vs interp.
    #[test]
    fn compiled_fn_with_args_matches_interp() {
        let isa = sample::isa();
        let prog = sample::sum_to_program();
        let code = compile_fn(&isa, &prog, 5, 1);
        for n in [0i64, 1, 2, 10, 100, 1000, 100_000] {
            let want = interpret_with(&isa, &prog, 5, &[n]);
            assert_eq!(code.call(&[n]), want, "n={n}");
            assert_eq!(want, n * (n + 1) / 2);
        }
    }

    /// The tier-up engine produces identical results across the interpreter→JIT boundary, and promotes
    /// after the hotness threshold.
    #[test]
    fn engine_tiers_up_with_identical_results() {
        use otter::engine::Engine;
        let mut eng = Engine::new(sample::isa());
        eng.add_tier(Box::new(CraneliftBackend), 5);
        let f = eng.register(sample::sum_to_program(), 5, 1);
        assert_eq!(eng.level(f), 0);
        for k in 0..30i64 {
            let n = (k * 7) % 53; // vary the argument across calls
            let r = eng.call(f, &[n]);
            assert_eq!(r, n * (n + 1) / 2, "call k={k} n={n}");
            let want = if eng.calls(f) >= 5 { 1 } else { 0 };
            assert_eq!(eng.level(f), want, "level after {} calls", eng.calls(f));
        }
        assert_eq!(eng.tier_name(f), "cranelift");
    }

    #[test]
    fn jit_matches_interp_sum() {
        let isa = sample::isa();
        for n in [0, 1, 2, 10, 100, 1000, 100_000] {
            let prog = sample::sum_program(n);
            let j = jit_run(&isa, &prog, 5);
            let i = interpret(&isa, &prog, 5);
            assert_eq!(j, i, "sum({n}): jit {j} vs interp {i}");
            assert_eq!(j, (n as i64) * (n as i64 + 1) / 2);
        }
    }

    /// Float + conversion ops: JIT (bitcast/fadd/fcvt) must agree with the interpreter bit-for-bit.
    #[test]
    fn jit_matches_interp_float() {
        let isa = sample::isa();
        let prog = sample::float_expr_program(); // (x + y) * x
        let code = compile_fn(&isa, &prog, 3, 2);
        let cases: [(f64, f64); 6] = [
            (2.0, 3.0),
            (-1.5, 0.5),
            (0.0, -0.0),
            (1e10, -2.0),
            (f64::INFINITY, 1.0),
            (f64::NAN, 1.0),
        ];
        for (x, y) in cases {
            let args = [x.to_bits() as i64, y.to_bits() as i64];
            let j = code.call(&args);
            assert_eq!(
                j,
                interpret_with(&isa, &prog, 3, &args),
                "jit≠interp for ({x},{y})"
            );
        }
        // conversions: (i64)((f64)n + (f64)n) == 2n
        let conv = sample::convert_roundtrip_program();
        let cc = compile_fn(&isa, &conv, 2, 1);
        for n in [0i64, 1, 7, -3, 1000, -100_000] {
            assert_eq!(cc.call(&[n]), 2 * n, "2*{n}");
            assert_eq!(cc.call(&[n]), interpret_with(&isa, &conv, 2, &[n]));
        }
    }

    /// Width family (i8/u8/i32/u32/u64 + f32 + conversions): JIT must match the interpreter bit-for-bit.
    #[test]
    fn jit_matches_interp_width() {
        use otter::sample::{
            CONV_F64_U8, CONV_I64_F32, CONV_U8_I64, F32_ADD, F32_LT, F32_MUL, HALT, I8_ADD, U8_ADD, U64_LT,
        };
        let isa = sample::isa();
        let f32b = |x: f32| x.to_bits() as i64;

        // binary ops over two i64-word args
        let bin = [
            (I8_ADD, 100, 100),
            (U8_ADD, 200, 100),
            (U64_LT, -1, 1),
            (U64_LT, 1, -1),
            (F32_ADD, f32b(1.5), f32b(2.25)),
            (F32_MUL, f32b(3.0), f32b(-0.5)),
            (F32_LT, f32b(1.0), f32b(2.0)),
            (F32_LT, f32b(f32::NAN), f32b(0.0)),
        ];
        for (op, a, b) in bin {
            let prog = vec![Inst::new(op, 2, 0, 1), Inst::new(HALT, 2, 0, 0)];
            let j = compile_fn(&isa, &prog, 3, 2).call(&[a, b]);
            assert_eq!(j, interpret_with(&isa, &prog, 3, &[a, b]), "op {op} a={a} b={b}");
        }

        // unary conversions over one arg
        let un = [
            (CONV_F64_U8, 300.0_f64.to_bits() as i64),
            (CONV_F64_U8, (-5.0_f64).to_bits() as i64),
            (CONV_U8_I64, 0xFF),
            (CONV_I64_F32, 12345),
        ];
        for (op, a) in un {
            let prog = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            let j = compile_fn(&isa, &prog, 2, 1).call(&[a]);
            assert_eq!(j, interpret_with(&isa, &prog, 2, &[a]), "conv {op} a={a}");
        }
    }

    /// f16 ops route through the canonical helper; the JIT must still match the interpreter exactly.
    #[test]
    fn jit_matches_interp_f16() {
        use otter::sample::{CONV_F16_F64, CONV_F64_F16, F16_ADD, F16_MUL, HALT};
        let isa = sample::isa();
        let bin = [
            (F16_ADD, 0x3C00i64, 0x3800i64),
            (F16_MUL, 0x4000, 0x3800),
            (F16_ADD, 0x7BFF, 0x7BFF),
        ];
        for (op, a, b) in bin {
            let prog = vec![Inst::new(op, 2, 0, 1), Inst::new(HALT, 2, 0, 0)];
            assert_eq!(
                compile_fn(&isa, &prog, 3, 2).call(&[a, b]),
                interpret_with(&isa, &prog, 3, &[a, b]),
                "f16 op {op}"
            );
        }
        for (op, a) in [(CONV_F64_F16, 0.1_f64.to_bits() as i64), (CONV_F16_F64, 0x3C00)] {
            let prog = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            assert_eq!(
                compile_fn(&isa, &prog, 2, 1).call(&[a]),
                interpret_with(&isa, &prog, 2, &[a]),
                "f16 conv {op}"
            );
        }
    }

    #[test]
    fn jit_matches_interp_factorial() {
        let isa = sample::isa();
        for n in 0..=12 {
            let prog = sample::factorial_program(n);
            assert_eq!(jit_run(&isa, &prog, 4), interpret(&isa, &prog, 4), "fact({n})");
        }
    }

    const NREGS: usize = 6;

    /// Deterministic LCG (no time/random in this environment, and reproducibility is wanted anyway).
    fn lcg(seed: u64) -> impl FnMut() -> u32 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        }
    }

    /// A random straight-line program: initialize all registers, then random ALU ops, then Halt.
    fn gen_straightline(next: &mut impl FnMut() -> u32) -> Vec<Inst> {
        let mut prog = Vec::new();
        for r in 0..NREGS {
            let imm = (next() % 21) as i32 - 10; // [-10, 10]
            prog.push(Inst::new(MOV_IMM, r as i32, imm, 0));
        }
        let body = 3 + (next() % 12) as usize;
        for _ in 0..body {
            let opcode = match next() % 4 {
                0 => ADD,
                1 => SUB,
                2 => MUL,
                _ => LT,
            };
            let dst = (next() as usize % NREGS) as i32;
            let a = (next() as usize % NREGS) as i32;
            let b = (next() as usize % NREGS) as i32;
            prog.push(Inst::new(opcode, dst, a, b));
        }
        let ret = (next() as usize % NREGS) as i32;
        prog.push(Inst::new(HALT, ret, 0, 0));
        prog
    }

    /// Differential fuzz: the JIT must agree with the interpreter on every generated program.
    #[test]
    fn jit_matches_interp_fuzz_straightline() {
        let isa = sample::isa();
        let mut next = lcg(0x9E37_79B9_7F4A_7C15);
        for _ in 0..2000 {
            let prog = gen_straightline(&mut next);
            let j = jit_run(&isa, &prog, NREGS);
            let i = interpret(&isa, &prog, NREGS);
            assert_eq!(j, i, "fuzz mismatch: jit {j} vs interp {i}\nprog={prog:?}");
        }
    }

    /// B0.2: the optimizer (`constant_fold`) preserves semantics under BOTH tiers. Straight-line
    /// programs fold heavily (every register becomes constant after the MovImm prelude) — a strong test
    /// of the mixed materialized/kept path and the i32 overflow guard; the control-flow samples exercise
    /// the block-boundary lattice reset.
    #[test]
    fn opt_preserves_under_interp_and_jit() {
        use otter::opt::constant_fold;
        let isa = sample::isa();

        for n in [0, 1, 2, 10, 100, 1000, 100_000] {
            let p = sample::sum_program(n);
            let o = constant_fold(&isa, &p, 5);
            let want = interpret(&isa, &p, 5);
            assert_eq!(interpret(&isa, &o, 5), want, "opt/interp sum({n})");
            assert_eq!(jit_run(&isa, &o, 5), want, "opt/jit sum({n})");
        }
        for n in 0..=12 {
            let p = sample::factorial_program(n);
            let o = constant_fold(&isa, &p, 4);
            let want = interpret(&isa, &p, 4);
            assert_eq!(interpret(&isa, &o, 4), want, "opt/interp fact({n})");
            assert_eq!(jit_run(&isa, &o, 4), want, "opt/jit fact({n})");
        }

        let mut next = lcg(0xD1B5_4A32_D192_ED03);
        for _ in 0..2000 {
            let p = gen_straightline(&mut next);
            let o = constant_fold(&isa, &p, NREGS);
            assert_eq!(o.len(), p.len(), "length-preserving");
            let want = interpret(&isa, &p, NREGS);
            assert_eq!(interpret(&isa, &o, NREGS), want, "opt/interp fuzz\np={p:?}");
            assert_eq!(jit_run(&isa, &o, NREGS), want, "opt/jit fuzz\np={p:?}");
        }
    }

    // --- OSR / deopt (B0.5-1) -----------------------------------------------------------------------

    /// Deopt preserves semantics: a guarded program gives the interpreter's result whether or not the
    /// guard fails (n < limit → no deopt; n >= limit → deopt then interpreter finishes).
    #[test]
    fn deopt_preserves_semantics() {
        let isa = sample::isa();
        let limit = 50;
        let prog = sample::sum_to_guarded_program(limit);
        for n in [0i64, 1, 10, 49, 50, 51, 100, 1000] {
            let want = interpret_with(&isa, &prog, 6, &[n]);
            assert_eq!(run_resumable(&isa, &prog, 6, &[n]), want, "n={n}");
            assert_eq!(want, n * (n + 1) / 2);
        }
    }

    /// The deopt path is genuinely exercised: n >= limit bails at the guard pc; n < limit completes in
    /// compiled code.
    #[test]
    fn deopt_path_is_actually_taken() {
        let isa = sample::isa();
        let limit = 50;
        let prog = sample::sum_to_guarded_program(limit);

        let mut frame = vec![0i64; 6];
        frame[0] = 10; // < limit
        assert!(
            matches!(
                compile_resumable(&isa, &prog, 6, 0).run_frame(&mut frame),
                Run::Completed(_)
            ),
            "small n should complete in compiled code (no deopt)"
        );

        let mut frame = vec![0i64; 6];
        frame[0] = 100; // >= limit
        match compile_resumable(&isa, &prog, 6, 0).run_frame(&mut frame) {
            Run::Deopt(pc) => assert_eq!(pc, 6, "deopt should occur at the guard pc"),
            Run::Completed(_) => panic!("expected a deopt for n >= limit"),
        }
    }

    /// OSR preserves semantics: a single-call long loop promotes mid-flight (via `interpret_until_osr`
    /// → `osr_continue`) and yields the interpreter's result; a short loop just completes in interp.
    #[test]
    fn osr_preserves_semantics() {
        use otter::interp::{Step, interpret_until_osr};
        let isa = sample::isa();
        let prog = sample::sum_to_program();
        for n in [0i64, 1, 5, 99, 100, 500, 100_000] {
            let want = interpret_with(&isa, &prog, 5, &[n]);
            let mut frame = vec![0i64; 5];
            frame[0] = n;
            let got = match interpret_until_osr(&isa, &prog, &mut frame, 0, 100) {
                Step::Completed(v) => v,
                Step::Osr(pc) => osr_continue(&isa, &prog, 5, &mut frame, pc),
            };
            assert_eq!(got, want, "n={n}");
            assert_eq!(want, n * (n + 1) / 2);
        }
    }

    /// OSR actually fires for a hot loop and stays off for a cold one.
    #[test]
    fn osr_fires_only_when_hot() {
        use otter::interp::{Step, interpret_until_osr};
        let isa = sample::isa();
        let prog = sample::sum_to_program();
        let mut hot = vec![0i64; 5];
        hot[0] = 100_000;
        assert!(matches!(
            interpret_until_osr(&isa, &prog, &mut hot, 0, 100),
            Step::Osr(_)
        ));
        let mut cold = vec![0i64; 5];
        cold[0] = 3;
        assert!(matches!(
            interpret_until_osr(&isa, &prog, &mut cold, 0, 100),
            Step::Completed(_)
        ));
    }

    /// The two halves compose: OSR into a guarded loop that then deopts; the interpreter finishes.
    #[test]
    fn osr_then_deopt() {
        use otter::interp::{Step, interpret_until_osr};
        let isa = sample::isa();
        let prog = sample::sum_to_guarded_program(5000);
        let n = 100_000i64; // >> limit → OSR in, then deopt out
        let want = interpret_with(&isa, &prog, 6, &[n]);
        let mut frame = vec![0i64; 6];
        frame[0] = n;
        let got = match interpret_until_osr(&isa, &prog, &mut frame, 0, 100) {
            Step::Completed(v) => v,
            Step::Osr(pc) => osr_continue(&isa, &prog, 6, &mut frame, pc),
        };
        assert_eq!(got, want);
        assert_eq!(want, n * (n + 1) / 2);
    }

    /// OSR auto-wired into the engine: a single long call promotes mid-flight with no manual driving.
    #[test]
    fn engine_auto_osr() {
        use otter::engine::Engine;
        let mut eng = Engine::new(sample::isa());
        eng.enable_osr(Box::new(ResumableCraneliftBackend), 100);
        let f = eng.register(sample::sum_to_program(), 5, 1);
        assert_eq!(eng.call(f, &[100_000]), 100_000i64 * 100_001 / 2); // OSR fires internally
        assert_eq!(eng.call(f, &[5]), 15); // short call: completes in the interpreter
    }

    /// Engine-driven OSR into a guarded loop that then deopts — both handled transparently inside `call`.
    #[test]
    fn engine_auto_osr_then_deopt() {
        use otter::engine::Engine;
        let mut eng = Engine::new(sample::isa());
        eng.enable_osr(Box::new(ResumableCraneliftBackend), 100);
        let f = eng.register(sample::sum_to_guarded_program(5000), 6, 1);
        let n = 100_000i64;
        assert_eq!(eng.call(f, &[n]), n * (n + 1) / 2);
    }
}
