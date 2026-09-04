//! L0 wasm-emit backend (**Twasm**) — lowers one [`otter`] function to a standalone **wasm module's
//! bytes** (O-13, design: `.claude-research-library/otter-o13-wasm-emit-design.md`).
//!
//! This is the 4th codegen backend (after interp/T0, baseline/T1, cranelift/T2), and the only one
//! whose output is *data* rather than live machine code: [`compile_dispatch_fn`] returns the bytes
//! of a minimal wasm module that a host (the `otter_jit.compile` contract of design §5, or the
//! differential test harness in `tests/`) compiles, instantiates against a shared memory + funcref
//! table, and registers in that table. Like the other backends it consumes the SAME
//! [`OpDef`](otter::OpDef)/[`Effect`]/[`Expr`] tables the interpreter consumes — there is no second,
//! hand-written semantics.
//!
//! # Emitted module shape (design §2.2 / §5.2 — normative)
//!
//! * sections: **type, import, function, export, code only** — in particular **no start section**
//!   (design §5.2 norm: instantiation must never run guest code) and no custom sections;
//! * imports: exactly `(import "otter" "memory" …)` + `(import "otter" "table" (table 0 funcref))`
//!   — **zero function imports** (every out-of-module target is a `call_indirect` through the
//!   shared table, §2 measured result). The memory import's TYPE is parameterized since O-17
//!   ([`EmitOpts::shared_memory`], design §5.2 v1.1): `(memory 0)` for the default non-shared
//!   guest, `(memory 0 65536 shared)` for a shared-linear-memory (+atomics-built) guest — see
//!   [`SHARED_MEMORY_IMPORT_MAX_PAGES`] for why that exact shape links against EVERY wasm32
//!   shared memory;
//! * exports: exactly one — `(export "f" ...)` of the `CompiledFn` wasm type
//!   `(param i32 i32 i64) (result i64)` = `(ctx, args, argc) -> result` (§2.2).
//!
//! All table indices are **caller-supplied** ([`HelperIdx`] + the `resolve` host-fn resolver, design
//! §2.3 index-externalization): in a wasm deployment the resolver is the identity on the baked fn
//! pointer (a wasm32 fn pointer IS its table index — §2 measured), while the native differential
//! harness maps native stub addresses to mock table slots it populated itself.
//!
//! # Increment status (W-2 — design §10)
//!
//! **Every IR variant now lowers**: all 10 [`Expr`] variants and all 8 [`Effect`] variants —
//! `SetLocalAt`/`Guard`/`Halt`/`Return` (W-1a), `JumpTo`/`JumpIf` through the §3 dispatcher loop
//! (W-1b: `loop` + `br_table` over basic-block leaders; handles ANY otter CFG, irreducible ones
//! included, without a relooper), and since W-2 the call family: `Call`/`TailCall` marshal their
//! argument block into the function's **caller-owned marshaling scratch** in the shared linear
//! memory ([`scratch_words`] sizes it, the `scratch_addr` parameter locates it — §2.5-1) and
//! `call_indirect` the engine's `module_dispatch`/`tail_signal` table slots
//! ([`HelperIdx::dispatch`]/[`HelperIdx::tail`], §2.3 #7-#8/§4.3). `TailCall` keeps the engine's
//! trampoline (signal + `return fill`; wasm `return_call` deliberately unused — the callee may be
//! T0, §4.3), so unbounded tail recursion stays native-stack-bounded through the SAME dispatcher
//! loop the native tiers use. The §4.6 structural precheck is complete: end fall-through
//! (§4.6-1), trailing `JumpIf` (§4.6-2), integer `Div` (§4.6-3: the helper would panic →
//! nounwind abort → uncatchable wasm trap; refusing keeps the function on T0 where the panic
//! stays isolated), control-effect-not-last (§4.6-4), plus the wasm implementation cap on
//! per-function locals ([`MAX_WASM_FUNCTION_LOCALS`] — the W-1a panel #2a lens A#2 carry-over).
//!
//! Since O-14 A-1 (design: `otter-o14-aot-core-design.md`) the crate also has a **whole-module
//! build-time entry point**, [`compile_module`]: N functions in ONE module against a module-scope
//! type interner, per-function marshaling scratch **self-assigned** by the emitter (§2.5 — the
//! `scratch_addr` parameter disappears from that surface; a manifest records the map), and every
//! per-function [`Refusal`] **totalized** into a whole-module [`AotError`] (§4.2 — AOT has no T0
//! fallback slot, so there is no sticky-`Refused` degradation to hide behind). Both public entry
//! points share ONE body emitter ([`compile_dispatch_fn_with`] wraps it with a fresh interner) —
//! the §5.1 "no second emitter" review rule in code form.
//!
//! **A-2 (internal link, §9 A-2)** replaced A-1's `otter.table` IMPORT with a module-**declared**
//! table + active element segment (§2.3: "otter function index = table slot"), and inverted the
//! three canonical helpers from table-slot-only to real FUNCTION IMPORTS (§2.2:
//! `otter_helpers.otter_{binop,convert,unop}`, `otter_host.<name>` for every resolved host fn).
//! Call lowering itself was left UNCHANGED at A-2 (dispatch-form: `Call`/`TailCall` marshal then
//! `call_indirect` a table slot) — only WHERE the callee lived changed, not HOW it was called; A-2
//! provisionally imported two trampoline stubs (`otter_helpers.{dispatch,tail}`) under that
//! namespace, replacing A-1's unexecutable OOB sentinel values with real, table-backed slots,
//! which made an A-2 artifact call-EXECUTABLE for the first time.
//!
//! **A-3 (call-lowering inversion, §9 A-3 — current)** deletes A-2's two provisional trampoline
//! imports/slots WHOLE and inverts `Call`/`TailCall` themselves (design §3.1/§3.2): a fail-closed
//! `i64.ge_u` guard against [`AotManifest::nfuncs`], then `call_indirect`/`return_call_indirect`
//! the callee word DIRECTLY — no dispatch-helper indirection, no engine trampoline loop, and the
//! declared table SHRINKS back to its §2.3 canonical size `T` (the A-2 `+2` oversizing is gone).
//! `TailCall` chains are now bounded by wasm's OWN tail-call mechanism instead of the engine's
//! Rust-side trampoline — [스파이크] M2 measured 2,000,000/2,000,001-deep mutual tail recursion
//! completing in a 64 KiB wasm stack. Internally, both public entry points still share the ONE
//! body emitter (§5.1): [`compile_dispatch_fn_with`] always lowers `Call`/`TailCall` in
//! `CallMode::Dispatch` (unchanged, the pre-A-1 shape), [`compile_module`] always in
//! `CallMode::Table` (A-3's new shape) — an internal enum parameter, not a second emitter.
//!
//! # Config decisions (recorded per W-1a scope)
//!
//! * **`wasm-encoder = "0.258"`** — the current crates.io stable line at implementation time
//!   (0.258.0 published 2026-08-24; checked against the crates.io API on 2026-09-02), with
//!   `default-features = false` (drops the component-model encoder; emitted modules are core wasm).
//! * **`no_std` + `alloc`**: wasm-encoder 0.258 is unconditionally `#![no_std]` (its `std` cargo
//!   feature only forwards to its *optional* `wasmparser` dep, which we don't enable), so this crate
//!   is `#![no_std]` + `alloc` like `otter` itself — verified by building this crate for
//!   `x86_64-unknown-none`. (`otter-cranelift`/`otter-baseline` are `std`; had wasm-encoder required
//!   `std` that fallback was pre-approved — design §9 — but it isn't needed.)
//! * **`forbid(unsafe_code)`**: pure lowering (IR in, bytes out) — no code pointers are formed or
//!   called here, so unlike the other backends there is nothing to concentrate `unsafe` for.
//! * **wasm32 only as a *target*** (panel B#3, W-1a share): the emitted module bakes wasm32 types
//!   (`i32` pointers). The crate itself builds and runs everywhere (native differential testing is
//!   the point — §8a), but building it *for* a non-wasm32 wasm target is a hard error below.
//!
//! # Semver posture (O-14 A-5 closeout — design §4.3)
//!
//! This crate is a **path-dep, pre-1.0** L0 component (version rides the workspace `0.0.0`;
//! `publish = false`): its consumers are this repository's own crates/tests/hosts plus, later,
//! the O-16c′ link tooling — the `otter-reusability-plan` track owns any future move to real
//! versioning. Concretely:
//!
//! * **Per-increment discipline is additive-only** on frozen surfaces ([`compile_dispatch_fn`]/
//!   [`HelperIdx`]/[`EmitOpts`]/[`Refusal`] byte-pin contract, §4.3), with growth points marked
//!   `#[non_exhaustive]` ([`EmitOpts`], [`Refusal`], [`AotOpts`], [`AotImportKind`],
//!   [`AotImport`], [`AotScratchEntry`], [`AotManifest`], [`AotError`], [`AotValidationError`]).
//! * **Breaking changes remain possible pre-1.0 but must be recorded with rationale** — precedent:
//!   A-2's retirement of `AotImportKind::Table`, A-3's retirement of the `Trampoline` variant +
//!   `TrampolineKind` (both documented at [`AotImportKind`]).
//! * **Three types are deliberately exhaustive**, each with its rationale at the type:
//!   [`HelperKind`] (frozen 3-helper set; downstream extern-wiring must break loudly),
//!   [`HostImportName`] (embedders construct it inside `resolve_host`), and [`AotArtifact`]
//!   (the total product pair; the §8.3 mutation harness constructs doctored ones downstream).
#![no_std]
#![forbid(unsafe_code)]
// O-14 A-5 (semver closeout): every public item of this crate carries a contract-stating doc —
// promoted from convention to a machine-checked lint (`just lint` runs clippy with -D warnings,
// so a missing doc is a CI red, not a nag).
#![warn(missing_docs)]

// Panel B#3 (design §13-3, W-1a share): `target_family = "wasm"` would include wasm64 — refuse a
// wasm64 TARGET BUILD of this crate at compile time rather than miscompile. Precision note (O-19
// C6, the C0-C2 caveat): this guard is about where the emitter ITSELF runs as deployed wasm, not
// what it can emit — since O-19 C2 the AOT surface emits A64 (memory64) artifacts from any
// supported host ([`AotOpts::with_memory64`]). What genuinely breaks on a wasm64 target build is
// the JIT (J-mode) deployment surface this crate compiles INTO: the §2.2 dispatch ABI (i32
// ctx/args) and the `otter_jit.compile(ptr: u32, len: u32)` host contract assume the engine's
// OWN memory is wasm32, and `EmitOpts` is deliberately not addrtype-parameterized (O-19 §2.1).
// Engine-on-wasm64 is the gate-external backlog item (pin-measured green only, no upstream
// contract — O-19 §1.3 ①/[[arch-support-tiers]]); lifting this guard belongs to THAT work.
#[cfg(all(target_family = "wasm", not(target_arch = "wasm32")))]
compile_error!(
    "otter-wasmemit builds for wasm32 wasm targets only: the JIT-surface §2.2 ABI (i32 ctx/args + \
     the u32 otter_jit contract) assumes the deployed engine's own memory is wasm32. This does not \
     limit what the crate EMITS — the AOT surface emits memory64 (A64) artifacts via \
     AotOpts::with_memory64 on any supported host. Engine-on-wasm64 is the O-19 backlog item; lift \
     this guard as part of that work only."
);

extern crate alloc;

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

use otter::validate::{self, ValidationError};
use otter::{ArithOp, Effect, Expr, FuncDef, Inst, Isa, UnOp, Value};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ElementSection, Elements, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, ImportSection, Instruction, MemArg, MemoryType, Module, RefType, TableSection,
    TableType, TypeSection, ValType,
};

/// Table indices of the five engine-side helpers every emitted module may `call_indirect` (design
/// §2.3 — the wasm-emit equivalents of the addresses `otter-cranelift` bakes). The **caller** owns
/// index acquisition (§2.3 index-externalization): on wasm32 a helper's fn pointer already IS its
/// table index (§2 measured result); the native test harness instead picks slots in a mock table it
/// populates itself.
///
/// All five are consumed since W-2: `binop`/`convert`/`unop` by the `Bin`/`Un` lowering (W-1a),
/// `dispatch`/`tail` by the `Call`/`TailCall` lowering in [`CallMode::Dispatch`] (§4.3 — every
/// call site marshals into the function's scratch and `call_indirect`s one of these two, §2.3
/// #7-#8). **Since O-14 A-3, `dispatch`/`tail` are read ONLY in `CallMode::Dispatch`**
/// ([`compile_dispatch_fn`]/[`compile_dispatch_fn_with`]'s JIT surface) — `CallMode::Table`
/// ([`compile_module`]'s AOT surface) never reads either field (§3.1/§3.2: the callee word is
/// `call_indirect`ed directly, no dispatch-helper indirection). This whole struct's shape is
/// frozen regardless (design §4.3: `compile_dispatch_fn`/`HelperIdx` are unchanged public API),
/// so `compile_module` still constructs a full `HelperIdx` — with inert placeholder values in
/// these two fields — purely to share [`emit_function_body`] with the JIT surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelperIdx {
    /// `module_dispatch` — `(i32, i64, i32, i64) -> i64` `(ctx, callee_idx, args, argc)` (§2.3 #7).
    /// Unused in `CallMode::Table` (see struct doc).
    pub dispatch: u32,
    /// `tail_signal` — same type as `dispatch` (§2.3 #8; the result is dropped, `tail_pending` is
    /// the discriminator). Unused in `CallMode::Table` (see struct doc).
    pub tail: u32,
    /// [`otter::otter_binop`] — `(i64, i64, i32, i32) -> i64` `(a, b, ty_tag, op_tag)`.
    pub binop: u32,
    /// [`otter::otter_convert`] — `(i64, i32, i32) -> i64` `(a, from_tag, to_tag)`.
    pub convert: u32,
    /// [`otter::otter_unop`] — `(i64, i32, i32) -> i64` `(a, ty_tag, unop_tag)`.
    pub unop: u32,
}

/// Declared `max` (in 64 KiB pages) of the **shared** `otter.memory` import shape (O-17):
/// 65,536 pages = the full wasm32 address space (4 GiB). Why this exact value links against
/// EVERY wasm32 shared linear memory (O-17 spike M5, measured on wasmtime 48): wasm import
/// matching uses standard limits subtyping — `declared.min <= actual.min` ∧
/// `declared.max >= actual.max` ∧ `shared` equal EXACTLY (no coercion in either direction) —
/// and a *shared* memory is REQUIRED by validation to declare a max, which on wasm32 can never
/// exceed 65,536 pages. So `(min 0, max 65536, shared)` is the universal supertype of every
/// real wasm32 shared memory, the same way the non-shared `(min 0, no max)` shape is for
/// non-shared ones. `memory.grow` never changes the memory's TYPE, so the match is stable for
/// the lifetime of the instance (M5).
pub const SHARED_MEMORY_IMPORT_MAX_PAGES: u64 = 65_536;

/// Module-shape options consumed by [`compile_dispatch_fn_with`] (O-17, design §5.2 v1.1).
///
/// Why a struct and not a 9th `compile_dispatch_fn` parameter: that signature is the design-§9
/// fixed 8-parameter shape shared (in spirit) with `otter-cranelift`/`otter-baseline`, already
/// at the `too_many_arguments` waiver — growing it would churn every existing call site (three
/// test suites + the engine) for a flag 100% of pre-O-17 callers leave at its default. The
/// struct is `#[non_exhaustive]` (construct via [`EmitOpts::new`]/`Default` + the `with_*`
/// builders) so future module-shape switches ride along without another signature change —
/// the same forward-compatibility posture as [`Refusal`]'s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EmitOpts {
    /// Emit the `otter.memory` import as **shared**, with limits
    /// `(0, `[`SHARED_MEMORY_IMPORT_MAX_PAGES`]`)` (O-17 L0 — passive shared-memory
    /// compatibility). `false` (the default) keeps the pre-O-17 non-shared `(memory 0)` shape
    /// byte-for-byte.
    ///
    /// The CALLER owns knowing the deployment's actual memory sharedness (the backend lowers
    /// the same bytes either way — sharedness only changes this import's declared type):
    /// sharedness must match the imported memory EXACTLY or instantiation fails (M5), which the
    /// engine's §5.1 degradation path absorbs as sticky-`Refused` T0 — a mis-declared flag is a
    /// tier-up loss, never a miscompile. `otter-engine` keys it on
    /// `cfg!(target_feature = "atomics")` (see its `compile_wasm`).
    pub shared_memory: bool,
}

impl EmitOpts {
    /// The defaults: non-shared memory import — [`compile_dispatch_fn_with`] with this value is
    /// byte-identical to [`compile_dispatch_fn`] (pinned by the structure tests).
    pub const fn new() -> Self {
        EmitOpts { shared_memory: false }
    }

    /// Builder toggle for [`EmitOpts::shared_memory`].
    #[must_use]
    pub const fn with_shared_memory(mut self, shared: bool) -> Self {
        self.shared_memory = shared;
        self
    }
}

/// Why [`compile_dispatch_fn`] declined to emit a module. **Never a panic** — for any `FuncDef`,
/// given a well-formed [`Isa`]: every input this backend cannot lower (or refuses to lower — §4.6)
/// comes back as a `Refusal`, and the engine's planned reaction (design §6) is a sticky `Refused`
/// slot: the function simply stays on T0. A refusal mirrors what is a *compile-time* failure on
/// the native backends into a clean value — it is not a tier-parity violation (§4.6).
///
/// The "well-formed `Isa`" qualifier is the same trust boundary EVERY tier shares (the `Isa` is an
/// embedder-built, trusted artifact — `otter`'s module doc): an `OpDef` carrying an operand slot
/// `>= 3` panics inside `otter::validate`/`Inst::operand` itself, on all tiers alike, before this
/// crate's code runs (W-1a panel #2a, lens C finding #3). This crate's own code contains zero
/// panic paths (no unwrap/expect/panic!/indexing — grep-verified).
///
/// The variant set is **per-increment** (hence `#[non_exhaustive]`): W-1b landed the full §4.6
/// structural precheck (end fall-through, trailing `JumpIf`, control-effect-not-last, the locals
/// cap) and shrank [`Refusal::UnsupportedEffect`] to the call family; W-2 (Call/TailCall) removed
/// that too — every [`Effect`]/[`Expr`] variant now lowers, and `UnsupportedEffect` survives only
/// as the defensive-marker channel (see its doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// `otter::validate::check_func` rejected the function (§8d: validation is mandatory *before*
    /// lowering — the emitted code indexes locals/targets on the strength of this proof).
    ValidationFailed(ValidationError),
    /// An **integer** `Bin { op: Div }` exists at `pc` (§4.6-3). `BinOp::eval`'s integer `Div` arm
    /// is `unreachable!` — on T0 that panic is caught and isolated, but inside the emitted module
    /// the helper's panic would cross a nounwind `extern "C"` boundary → abort → an uncatchable
    /// wasm trap killing the whole activation. Refusing keeps the function on T0, preserving the
    /// native failure mode.
    IntegerDiv {
        /// Instruction index of the offending integer `Div`.
        pc: usize,
    },
    /// Since W-2 every [`Effect`] variant lowers, so this is no longer returned for any variant by
    /// name; it survives solely as the **defensive-marker channel** for the genuinely-unreachable
    /// internal paths (`effect` is then an internal marker string, not a variant name — the
    /// documented exception to this field's original contract, W-1a panel #2a lens C finding #2),
    /// which stay a clean `Err` rather than a panic per this crate's no-panic contract.
    UnsupportedEffect {
        /// Instruction index the marker fired at.
        pc: usize,
        /// The internal marker string (NOT a variant name — see the variant doc).
        effect: &'static str,
    },
    /// `resolve` returned `None` for the host-fn pointer (as `usize`) carried by a
    /// `Host1`/`Host2`/`HostCtx1`/`HostCtx2` at `pc` — the caller's table has no slot for it, so
    /// there is no index to bake into the `call_indirect` (§2.3 #3-#6).
    HostFnUnresolved {
        /// Instruction index of the unresolvable host call.
        pc: usize,
    },
    /// §4.6-1: the function's last instruction (`last_pc`) is not a terminator, so execution
    /// would fall off the end of `code`. On T0 that is a pc-out-of-range panic (caught and
    /// isolated); in the §3 dispatcher loop the shape has no lowering at all (there is no "next
    /// leader" to fall to). Statically detectable ⇒ refused, never a runtime `unreachable`
    /// (§3 norm). `check_func` deliberately does not check this (design §3 correction).
    FallsOffEnd {
        /// Index of the function's non-terminator last instruction.
        last_pc: usize,
    },
    /// §4.6-2: a `JumpIf` at `pc` with `pc + 1 == code.len()` — its not-taken edge falls off the
    /// end of `code`, the same shape as [`Refusal::FallsOffEnd`] one edge earlier. (`check_func`
    /// bounds only the *explicit* jump target, not the implicit fall-through.)
    TrailingJumpIf {
        /// Instruction index of the trailing `JumpIf` (`== code.len() - 1`).
        pc: usize,
    },
    /// §4.6-4: a control [`Effect`] (`JumpTo`/`JumpIf`/`Halt`/`Return`/`TailCall`, named by
    /// `effect`) is not the LAST effect of its `OpDef` at `pc`. T0 keeps executing the residual
    /// effects after a taken `JumpTo`/`JumpIf` (and returns immediately from
    /// `Halt`/`Return`/`TailCall`), but every compiled tier ends its block at the control effect —
    /// here the residue would become *validating dead wasm code*, i.e. silent tier divergence
    /// (native cranelift at least fails loudly at compile time). See the framework contract on
    /// [`Effect`]'s own doc.
    ControlEffectNotLast {
        /// Instruction index of the offending `OpDef` use.
        pc: usize,
        /// Name of the non-last control effect (`"JumpTo"`, `"Halt"`, …).
        effect: &'static str,
    },
    /// §4.6-5 (the W-1a "known implementation-limit gap", panel #2a lens A#2 — closed): the
    /// emitted function would declare `wasm_locals` locals (otter `nlocals` + the 3 `CompiledFn`
    /// params + the dispatcher's `$state`), over the engines' [`MAX_WASM_FUNCTION_LOCALS`] cap.
    /// Refusing up front avoids returning dead-on-arrival `Ok` bytes that would surface as the
    /// host's `-1` misattributed to a host limit. `wasm_locals` is the **uniform upper-bound**
    /// figure (`EMITTED_EXTRA_LOCALS` counted for every function): on the dispatcher path the
    /// bound is exact — the first refused function would really declare `limit + 1` locals and be
    /// rejected by every engine's validator — while a straight-line function (no `$state`) is
    /// refused one slot early, i.e. its actual declaration would have been `limit` and valid
    /// (panel #2b lens C#2: the uniform accounting errs only in the conservative direction, never
    /// the reverse).
    WasmLocalsLimitExceeded {
        /// The uniform-upper-bound local count the emitted function would declare.
        wasm_locals: usize,
        /// [`MAX_WASM_FUNCTION_LOCALS`], echoed for the diagnostic.
        limit: usize,
    },
    /// The caller-provided `scratch_addr` plus this function's marshaling span
    /// (`8 * scratch_words`) would extend past the wasm32 address space (`> 2^32`), so every
    /// marshaling store would deterministically trap at runtime — a statically detectable
    /// always-trap shape, refused per this crate's §3/§4.6 norm instead of emitted (panel #3
    /// lens A finding P2). A real allocation can never produce such an address; a span ending
    /// exactly at `2^32` is legal, and a call-free function never bakes the address at all.
    ScratchOutOfAddressSpace {
        /// First byte of the function's marshaling scratch (caller-supplied on the JIT surface,
        /// §2.5 self-assigned on the AOT surface). `u64` since O-19 C1 (§2.2 #6 — the u64
        /// representation is upward-compatible on A32; the diagnostic now reports the EXACT
        /// out-of-range address instead of a `u32`-saturated stand-in).
        scratch_addr: u64,
        /// The function's [`scratch_words`] (its span is `8 * words` bytes).
        words: usize,
    },
    /// Anti-wrap guard, not an engine limit: the dispatcher's `$state` is an `i32` and `br_table`
    /// labels are `u32`s, so more than `i32::MAX` basic-block leaders cannot be encoded without a
    /// silently wrapping cast (the worst failure class — a miscompile, not a panic). Reaching this
    /// needs a >2³¹-instruction function (a ≥32 GiB `code` vec), so it is untestable in practice;
    /// the variant exists to keep the "`Ok` ⇒ validating module" contract *total* rather than
    /// probabilistic. (Sub-`i32::MAX` `br_table`s a host still rejects — e.g. V8's ~65,520-target
    /// cap — stay a §5.1 host-side `-1` → sticky-`Refused` degradation, by design.)
    TooManyLeaders {
        /// The leader count that would not fit the `i32` dispatcher `$state`.
        nleaders: usize,
    },
}

// --- wasm local-index layout of the emitted function (§2.2) ---------------------------------------
// params: 0 = ctx (i32, opaque — forwarded only, never dereferenced), 1 = args (i32, pointer into
// the shared linear memory), 2 = argc (i64). Declared locals (all i64) follow, so otter local `j`
// lives at wasm local `FIRST_LOCAL + j`.
const CTX_PARAM: u32 = 0; // opaque — forwarded to dispatch/tail as their first arg, never dereferenced
const ARGS_PARAM: u32 = 1;
const ARGC_PARAM: u32 = 2;
const FIRST_LOCAL: u32 = 3;

/// Otter local `slot` (validated `0 <= slot < nlocals` by `check_func`) as a wasm local index.
fn local(slot: i32) -> u32 {
    // `slot` is non-negative and `nlocals <= MAX_SAFE_NLOCALS` (~2^26), so this cannot wrap.
    FIRST_LOCAL + slot as u32
}

/// Hard cap wasm engines place on one function's locals — **params included** (wasmparser
/// `limits::MAX_WASM_FUNCTION_LOCALS = 50_000`, enforced in its validator with the params
/// pre-counted as locals; wasmtime shares the wasmparser limit, and the V8 family uses the same
/// 50,000 figure). `check_func`'s own `MAX_SAFE_NLOCALS` (~67M) is far above this, so a function
/// can be perfectly valid otter IR and still be un-emittable — without the §4.6-5 precheck the
/// backend would return `Ok` bytes **no engine will validate** (the W-1a "known
/// implementation-limit gap", panel #2a lens A#2; closed by [`Refusal::WasmLocalsLimitExceeded`]).
pub const MAX_WASM_FUNCTION_LOCALS: usize = 50_000;

/// Wasm locals the emitted function adds on top of the otter frame: the 3 `CompiledFn` params
/// (ctx/args/argc — params count toward [`MAX_WASM_FUNCTION_LOCALS`], see its doc) plus the
/// dispatcher's `$state` i32 (§3). Counted for EVERY function, even straight-line ones that skip
/// `$state`: one slot of slack is a fair trade for a single uniform precheck bound.
const EMITTED_EXTRA_LOCALS: usize = 4;

/// Is `e` a **control effect** — one that ends a basic block? The SAME predicate as
/// `otter-cranelift::is_terminator` (`:141-159`), `TailCall` included — the O-8c omission class
/// this list must never regress on. Exhaustive with **no wildcard arm**, on purpose
/// ([[check-completeness-periodically]]): a new [`Effect`] variant must be classified here before
/// this crate compiles again.
fn effect_is_control(e: &Effect) -> bool {
    match e {
        Effect::JumpTo { .. }
        | Effect::JumpIf { .. }
        | Effect::Halt { .. }
        | Effect::Return { .. }
        | Effect::TailCall { .. } => true,
        Effect::SetLocalAt { .. } | Effect::Guard { .. } | Effect::Call { .. } => false,
    }
}

/// The variant name reported by [`Refusal::ControlEffectNotLast`]. Exhaustive, no wildcard.
fn effect_name(e: &Effect) -> &'static str {
    match e {
        Effect::SetLocalAt { .. } => "SetLocalAt",
        Effect::JumpTo { .. } => "JumpTo",
        Effect::JumpIf { .. } => "JumpIf",
        Effect::Halt { .. } => "Halt",
        Effect::Guard { .. } => "Guard",
        Effect::Call { .. } => "Call",
        Effect::TailCall { .. } => "TailCall",
        Effect::Return { .. } => "Return",
    }
}

/// Does this opcode transfer control? Same derivation as `otter-cranelift::is_terminator`, minus
/// its `debug_assert` on the `hints.terminator` mirror — this crate has a no-panic contract, and
/// the hint/effect consistency tripwire already lives on the cranelift tier.
fn op_is_terminator(isa: &Isa, opcode: u16) -> bool {
    isa.op(opcode).effects.iter().any(effect_is_control)
}

// --- structural type interning --------------------------------------------------------------------

/// The emitted module's address width (O-19 design §2.1/§2.2 — the addrtype abstraction). Flows
/// through the whole emitter via [`Types`] (the type-table injection point: the pointer-typed
/// params of [`COMPILED_TY`] are what the width changes first), so every width-sensitive site
/// reads ONE source. `A64` is reachable from the AOT surface only ([`AotOpts::with_memory64`] —
/// the JIT surface's `EmitOpts` is deliberately NOT extended, §2.1): the JIT deployment's memory
/// is the engine's own, and engine-on-wasm64 is a gate-external backlog item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddrType {
    /// wasm32 addressing — the pre-O-19 shape, byte-for-byte (pinned by the golden vectors).
    A32,
    /// memory64 addressing, MIXED shape (O-19 §2.1 [결정]): the linear memory is 64-bit, the
    /// funcref table stays 32-bit (`table64: false`) — addrtype is per-memory/per-table in wasm,
    /// and the table/`call_indirect` machinery is byte-identical either way (S2-M1-3 measured).
    A64,
}

/// Appends function types to the type section, deduplicating structurally (wasm type equality is
/// structural, so reuse is cosmetic — but it keeps the §2.3 type table visibly 1:1 in the output).
/// Since O-19 the interner also carries the module's [`AddrType`] — the injection point that
/// makes the type table itself addrtype-parameterized (design §2.2 #1).
struct Types {
    section: TypeSection,
    seen: Vec<(Vec<ValType>, Vec<ValType>)>,
    /// The module's address width — read by [`Types::compiled`] and every width-branching
    /// emission site (via `Ctx::types`), so the width decision has ONE source.
    addr: AddrType,
}

impl Types {
    fn new(addr: AddrType) -> Self {
        Types {
            section: TypeSection::new(),
            seen: Vec::new(),
            addr,
        }
    }

    fn index_of(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        if let Some(i) = self.seen.iter().position(|(p, r)| p == params && r == results) {
            return i as u32;
        }
        self.section
            .ty()
            .function(params.iter().copied(), results.iter().copied());
        self.seen.push((params.to_vec(), results.to_vec()));
        (self.seen.len() - 1) as u32
    }

    /// Intern THIS module's `CompiledFn` type — [`COMPILED_TY`] on A32, [`COMPILED_TY_A64`] on
    /// A64 (design §2.2 #1: the ctx/args params are pointers, so their width follows the
    /// addrtype; argc/result stay i64). Every `CompiledFn`-typed site (the exported function,
    /// the Table-mode `call_indirect`/`return_call_indirect`) goes through here, never through
    /// a raw `index_of(COMPILED_TY…)` — the single-source rule for the width decision.
    fn compiled(&mut self) -> u32 {
        let (params, results) = match self.addr {
            AddrType::A32 => COMPILED_TY,
            AddrType::A64 => COMPILED_TY_A64,
        };
        self.index_of(params, results)
    }
}

// §2.3 helper/host call_indirect types (the full W-1a slice of the design's exhaustive table).
const I32: ValType = ValType::I32;
const I64: ValType = ValType::I64;
const COMPILED_TY: (&[ValType], &[ValType]) = (&[I32, I32, I64], &[I64]); // (ctx,args,argc)->r
/// [`COMPILED_TY`]'s A64 twin (O-19 §2.2 #1): ctx and args are POINTERS, so both widen to i64
/// under memory64; argc and the result were i64 already. Interned only via [`Types::compiled`].
const COMPILED_TY_A64: (&[ValType], &[ValType]) = (&[I64, I64, I64], &[I64]);
const BINOP_TY: (&[ValType], &[ValType]) = (&[I64, I64, I32, I32], &[I64]); // §2.3 #1
const UNARY_HELPER_TY: (&[ValType], &[ValType]) = (&[I64, I32, I32], &[I64]); // #2 (convert) + unop
const HOST1_TY: (&[ValType], &[ValType]) = (&[I64], &[I64]); // #3
const HOST2_TY: (&[ValType], &[ValType]) = (&[I64, I64], &[I64]); // #4, #5 (host_ctx1)
const HOSTCTX2_TY: (&[ValType], &[ValType]) = (&[I64, I64, I64], &[I64]); // #6
/// #7 (`module_dispatch`) and #8 (`tail_signal`) share this type: `(ctx, callee_idx, args, argc)`.
const DISPATCH_TY: (&[ValType], &[ValType]) = (&[I32, I64, I32, I64], &[I64]);

// --- marshaling-scratch sizing (design §2.5-1, W-2) -----------------------------------------------

/// The argument count of one `Call`/`TailCall` site — a **compile-time constant** (the `argc`
/// field is an operand-slot index and `Inst` operands are fixed at IR-construction time; the same
/// reading every tier does, e.g. `otter-cranelift` :1026/:980). This one function is the single
/// source both [`scratch_words`]' scan and the `Call`/`TailCall` lowering read the count through
/// (§2.5-1's no-dual-source rule): the size the caller allocates and the offsets the emitted code
/// stores to can never disagree. `check_func` proves `argc >= 0` (`CallSlotOutOfRange` checks the
/// sign); the clamp keeps this total (no-panic contract) rather than trusting that proof twice.
fn site_argc(inst: &Inst, argc_op: u8) -> usize {
    let n = inst.operand(argc_op);
    if n > 0 { n as usize } else { 0 }
}

/// Words the marshaling scratch of `f` needs: the **max [`site_argc`] over every `Call`/`TailCall`
/// site** in the function (design §2.5-1 — exact-size, per-function). `0` when the function
/// contains no call site at all (its `scratch_addr` is then never baked into anything — see
/// [`compile_dispatch_fn`]). Exhaustive on [`Effect`], no wildcard
/// ([[check-completeness-periodically]]).
fn max_scratch_words(isa: &Isa, f: &FuncDef) -> usize {
    let mut max = 0usize;
    for inst in &f.code {
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::Call { argc, .. } | Effect::TailCall { argc, .. } => {
                    max = max.max(site_argc(inst, *argc));
                }
                Effect::SetLocalAt { .. }
                | Effect::JumpTo { .. }
                | Effect::JumpIf { .. }
                | Effect::Halt { .. }
                | Effect::Guard { .. }
                | Effect::Return { .. } => {}
            }
        }
    }
    max
}

/// How many 8-byte words of argument-marshaling scratch the emitted code for `f` will store
/// through its `scratch_addr` (design §2.5-1): the maximum `argc` over every `Call`/`TailCall`
/// site (each site's count is a compile-time constant — [`site_argc`]), or `0` for a call-free
/// function. The **caller** of [`compile_dispatch_fn`] allocates (and owns, for the lifetime of
/// the emitted code — add-only) `scratch_words(isa, f) * 8` bytes of shared linear memory at an
/// 8-byte-aligned `scratch_addr` and passes that address in; this helper is how it learns the
/// size. The lowering derives every per-site count through the SAME [`site_argc`] this scan uses
/// (§2.5-1 no-dual-source rule), so a `scratch_words`-sized buffer is exact by construction:
/// every store the emitted code issues lands in `[scratch_addr, scratch_addr + 8 * words)`.
///
/// Runs `otter::validate::check_func` first (same reason [`compile_dispatch_fn`] does — the walk's
/// panic-freedom leans on the opcode/operand proofs), so a malformed `f` is a clean
/// [`Refusal::ValidationFailed`], never a panic. Also mirrors the §4.6-5 locals-cap refusal
/// (panel #3 lens A finding P1): without it, a function `compile_dispatch_fn` is certain to
/// refuse could still report a scratch size up to ~512 MB here, luring an allocate-then-compile
/// caller (the natural W-3 ordering) into a large dead allocation. With the cap mirrored, the
/// returned size is bounded by `MAX_WASM_FUNCTION_LOCALS` words.
pub fn scratch_words(isa: &Isa, f: &FuncDef) -> Result<usize, Refusal> {
    validate::check_func(isa, f).map_err(Refusal::ValidationFailed)?;
    check_locals_cap(f)?;
    Ok(max_scratch_words(isa, f))
}

// --- structural precheck (design §4.6 — complete as of W-1b) --------------------------------------

/// Static walk performed after `check_func`, before lowering: refuse every shape this increment
/// cannot (or must not) lower, so lowering itself can never fail halfway. Exhaustive on [`Effect`]
/// and (via [`precheck_expr`]) [`Expr`] with **no wildcard arm**, on purpose
/// ([[check-completeness-periodically]]): a new IR variant fails compilation here instead of
/// silently slipping past the precheck.
///
/// Deterministic order (pinned by tests): the two function-shaped checks first — §4.6-5 locals
/// cap, then §4.6-1 end fall-through (mirroring `check_func`'s own frame-checks-first layout) —
/// then the per-pc walk, which reports the first offending `(pc, effect-position)`.
/// §4.6-5, shared by [`precheck`] and [`scratch_words`] (the latter mirrors it so a
/// certain-to-refuse function cannot first lure an allocate-then-compile caller into a huge dead
/// scratch allocation — panel #3 lens A finding P1). `check_func` already bounded
/// `nlocals <= MAX_SAFE_NLOCALS` (~67M), so the addition cannot wrap.
fn check_locals_cap(f: &FuncDef) -> Result<(), Refusal> {
    let wasm_locals = f.nlocals + EMITTED_EXTRA_LOCALS;
    if wasm_locals > MAX_WASM_FUNCTION_LOCALS {
        return Err(Refusal::WasmLocalsLimitExceeded {
            wasm_locals,
            limit: MAX_WASM_FUNCTION_LOCALS,
        });
    }
    Ok(())
}

fn precheck(isa: &Isa, f: &FuncDef, resolve: &dyn Fn(usize) -> Option<u32>) -> Result<(), Refusal> {
    check_locals_cap(f)?; // §4.6-5
    // §4.6-1: the last instruction must be a terminator. (`check_func` proved `code` non-empty —
    // the `if let` keeps this walk panic-free rather than trusting that proof twice.)
    let code_len = f.code.len();
    if let Some(last) = f.code.last()
        && !op_is_terminator(isa, last.op)
    {
        return Err(Refusal::FallsOffEnd {
            last_pc: code_len - 1,
        });
    }
    for (pc, inst) in f.code.iter().enumerate() {
        let effects = &isa.op(inst.op).effects;
        for (i, eff) in effects.iter().enumerate() {
            // §4.6-4, checked before the per-variant handling: covers `TailCall` too (it was
            // already caught here while still on the unsupported list pre-W-2 — W-2 only deleted
            // that list's arms, never re-derived the position rule).
            if effect_is_control(eff) && i + 1 != effects.len() {
                return Err(Refusal::ControlEffectNotLast {
                    pc,
                    effect: effect_name(eff),
                });
            }
            match eff {
                Effect::SetLocalAt { reg: _, value } => precheck_expr(value, pc, resolve)?,
                Effect::Guard { cond } => precheck_expr(cond, pc, resolve)?,
                Effect::Halt { value } | Effect::Return { value } => precheck_expr(value, pc, resolve)?,
                Effect::JumpTo { target: _ } => {}
                Effect::JumpIf { cond, target: _ } => {
                    precheck_expr(cond, pc, resolve)?;
                    // §4.6-2: the not-taken edge is pc+1 — off the end of `code` when trailing.
                    if pc + 1 == code_len {
                        return Err(Refusal::TrailingJumpIf { pc });
                    }
                }
                // Lowered since W-2. Their three fields are operand-slot indices with no nested
                // `Expr` to walk, and `check_func`'s `CallSlotOutOfRange` already proved
                // `base >= 0 && argc >= 0 && base + 1 + argc <= nlocals` — the exact static proof
                // the marshaling loop's local indices lean on (§8d). `TailCall`'s must-be-last
                // rule is the generic §4.6-4 check above (it was already covered there pre-W-2,
                // so removing the arms here is the whole change — no position rule re-derived).
                Effect::Call { .. } | Effect::TailCall { .. } => {}
            }
        }
    }
    Ok(())
}

/// [`precheck`]'s expression walk: integer `Div` (§4.6-3) and unresolvable host fns, at any depth.
fn precheck_expr(e: &Expr, pc: usize, resolve: &dyn Fn(usize) -> Option<u32>) -> Result<(), Refusal> {
    let resolved = |p: usize| -> Result<(), Refusal> {
        match resolve(p) {
            Some(_) => Ok(()),
            None => Err(Refusal::HostFnUnresolved { pc }),
        }
    };
    match e {
        Expr::Imm(_) | Expr::ImmF64(_) | Expr::OperandImm(_) | Expr::LocalAt(_) => Ok(()),
        Expr::Bin(op, a, b) => {
            if !op.ty.is_float() && op.op == ArithOp::Div {
                return Err(Refusal::IntegerDiv { pc });
            }
            precheck_expr(a, pc, resolve)?;
            precheck_expr(b, pc, resolve)
        }
        Expr::Un(_, a) => precheck_expr(a, pc, resolve),
        Expr::Host1(f, a) => {
            resolved(*f as usize)?;
            precheck_expr(a, pc, resolve)
        }
        Expr::Host2(f, a, b) => {
            resolved(*f as usize)?;
            precheck_expr(a, pc, resolve)?;
            precheck_expr(b, pc, resolve)
        }
        Expr::HostCtx1(f, a) => {
            resolved(*f as usize)?;
            precheck_expr(a, pc, resolve)
        }
        Expr::HostCtx2(f, a, b) => {
            resolved(*f as usize)?;
            precheck_expr(a, pc, resolve)?;
            precheck_expr(b, pc, resolve)
        }
    }
}

// --- call-site lowering mode (O-14 A-3, design §5.1) ------------------------------------------------

/// How `Call`/`TailCall` lower — the ONE shared body emitter's internal call-lowering-mode
/// parameter (design §5.1: "제2 방출 경로 없음" review rule in code form — marshaling, the
/// prologue, and every `Bin`/`Un`/`Host*` helper call stay IDENTICAL either way; only the
/// `Call`/`TailCall` call-site SHAPE itself switches). Named after, and playing the same role as,
/// `otter-cranelift::CallMode::Dispatch/Table` (design §5.1: "1:1 이식") — a DIFFERENT type in a
/// DIFFERENT crate, not shared code; the cranelift `Table` variant additionally has no `TailCall`
/// lowering and no bounds guard (a separate native AOT mechanism, design §10-5 risk note), so the
/// analogy is structural (the Dispatch/Table split itself), not a byte-for-byte precedent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallMode {
    /// The JIT-shaped single-function module's ONLY mode, unchanged since before A-1 (§9 A-1 row:
    /// "호출 lowering은 무변경") and unchanged BY A-3 (§9 A-3 scope: A-3 only inverts the AOT
    /// surface): marshal, then `call_indirect` the engine's `module_dispatch`/`tail_signal` table
    /// slots (design §4.3, [`emit_dispatch_call`]) — the engine's trampoline loop is what keeps
    /// tail recursion bounded on THIS path, unaffected by anything in this crate.
    Dispatch,
    /// `compile_module`'s ONLY mode since A-3 (design §3.1/§3.2): marshal, then a fail-closed
    /// `i64.ge_u` guard against `nfuncs` (the §3 guard boundary — [`AotManifest::nfuncs`]) directly
    /// followed by `call_indirect`/`return_call_indirect` on the callee word ITSELF (§2.3: otter
    /// function index = table slot, so no dispatch-helper indirection is needed at all — that
    /// helper, and the engine trampoline loop it fed, are exactly what A-3 retires for AOT
    /// artifacts; wasm's own `return_call_indirect` is what keeps tail recursion bounded here,
    /// [스파이크] M2).
    Table { nfuncs: u32 },
}

// --- expression lowering (design §4.2) ------------------------------------------------------------

/// Everything a lowered instruction needs besides the [`Expr`] itself.
struct Ctx<'a> {
    types: &'a mut Types,
    helper: HelperIdx,
    resolve: &'a dyn Fn(usize) -> Option<u32>,
    host_ctx: Value,
    /// This function's caller-owned argument-marshaling scratch in the shared linear memory
    /// (design §2.5-1) — baked into every `Call`/`TailCall` site by [`marshal_call_args`].
    /// `u64` since O-19 C1 (§2.2 #6: the u64 representation is upward-compatible on A32 too);
    /// the width the BAKED constant takes follows the module's [`AddrType`]
    /// ([`emit_scratch_addr_const`]).
    scratch_addr: u64,
    /// How `Call`/`TailCall` lower for THIS module (design §5.1) — see [`CallMode`].
    call_mode: CallMode,
    /// The pc of the instruction currently being lowered — kept current by the body loop so any
    /// `Refusal` raised from inside expression lowering (a stateful `resolve` returning `None` after
    /// `precheck` saw `Some` — the resolver is an arbitrary `&dyn Fn` and may not be pure) reports
    /// the real site instead of a sentinel (W-1a panel #2a, lens C finding #2).
    pc: usize,
}

/// Lower `e`, leaving exactly one i64 on the wasm stack. Mirrors `otter-cranelift::lower_expr`
/// shape-for-shape, with every `Bin`/`Un` routed through the canonical helpers (design §4 v1
/// all-helper policy: the helpers ARE the interpreter semantics, so 11-NumTy-wide bit-identity —
/// `half` softfloat f16 included — holds by construction). Exhaustive on [`Expr`], no wildcard.
fn lower_expr(body: &mut Function, cx: &mut Ctx<'_>, inst: &Inst, e: &Expr) -> Result<(), Refusal> {
    match e {
        Expr::Imm(v) => {
            body.instruction(&Instruction::I64Const(*v));
        }
        Expr::ImmF64(f) => {
            body.instruction(&Instruction::I64Const(f.to_bits() as i64));
        }
        // Compile-time constant; i32 -> i64 SIGN-extension (`i64::from` on the i32 operand), same
        // as cranelift's `operand as i64` — a negative operand immediate stays negative.
        Expr::OperandImm(i) => {
            body.instruction(&Instruction::I64Const(i64::from(inst.operand(*i))));
        }
        Expr::LocalAt(i) => {
            body.instruction(&Instruction::LocalGet(local(inst.operand(*i))));
        }
        Expr::Bin(op, a, b) => {
            // Integer Div was refused by `precheck` (§4.6-3); everything else is total in the
            // helper (design §4 helper-totality table).
            lower_expr(body, cx, inst, a)?;
            lower_expr(body, cx, inst, b)?;
            body.instruction(&Instruction::I32Const(op.ty.tag() as i32));
            body.instruction(&Instruction::I32Const(op.op.tag() as i32));
            emit_call_indirect(body, cx, cx.helper.binop, BINOP_TY);
        }
        Expr::Un(op, a) => {
            lower_expr(body, cx, inst, a)?;
            match op {
                UnOp::Neg(ty) => {
                    body.instruction(&Instruction::I32Const(ty.tag() as i32));
                    body.instruction(&Instruction::I32Const(UnOp::NEG_TAG as i32));
                    emit_call_indirect(body, cx, cx.helper.unop, UNARY_HELPER_TY);
                }
                UnOp::Convert { from, to } => {
                    body.instruction(&Instruction::I32Const(from.tag() as i32));
                    body.instruction(&Instruction::I32Const(to.tag() as i32));
                    emit_call_indirect(body, cx, cx.helper.convert, UNARY_HELPER_TY);
                }
            }
        }
        Expr::Host1(f, a) => {
            lower_expr(body, cx, inst, a)?;
            let idx = resolve_host(cx, *f as usize)?;
            emit_call_indirect(body, cx, idx, HOST1_TY);
        }
        Expr::Host2(f, a, b) => {
            lower_expr(body, cx, inst, a)?;
            lower_expr(body, cx, inst, b)?;
            let idx = resolve_host(cx, *f as usize)?;
            emit_call_indirect(body, cx, idx, HOST2_TY);
        }
        // host_ctx is PREPENDED as a baked constant (§2.3 #5/#6) — and it is `Module::host_ctx`,
        // never the `ctx` param (design §2.2 opaque-ptr discipline; the two must not be conflated).
        Expr::HostCtx1(f, a) => {
            body.instruction(&Instruction::I64Const(cx.host_ctx));
            lower_expr(body, cx, inst, a)?;
            let idx = resolve_host(cx, *f as usize)?;
            emit_call_indirect(body, cx, idx, HOST2_TY);
        }
        Expr::HostCtx2(f, a, b) => {
            body.instruction(&Instruction::I64Const(cx.host_ctx));
            lower_expr(body, cx, inst, a)?;
            lower_expr(body, cx, inst, b)?;
            let idx = resolve_host(cx, *f as usize)?;
            emit_call_indirect(body, cx, idx, HOSTCTX2_TY);
        }
    }
    Ok(())
}

/// Push this function's scratch address as a pointer-typed constant (O-19 §2.2 #2/#3 — the ONE
/// place the address-constant width branches, read off the [`Types`]-carried [`AddrType`]).
///
/// * A32: `i32.const` — byte-identical to the pre-O-19 shape for every accepted A32 input (the
///   §4.6 bound keeps an accepted A32 `scratch_addr` under `2^32`, so the narrowing is exact
///   there; a `words == 0` function's dead bake may wrap, which its contract documents as
///   harmless — [`AotScratchEntry::addr`]).
/// * A64: `i64.const`, full width — not a style choice but validator-forced: an `i32.const`
///   address operand against a 64-bit memory is a validation error ("expected i64, found i32")
///   [실측 S2-M1-2]. The §2.4 cap bounds an accepted A64 address under `2^63`, so the `as i64`
///   reinterpretation is exact (non-negative) for every accepted input.
fn emit_scratch_addr_const(body: &mut Function, cx: &Ctx<'_>) {
    match cx.types.addr {
        AddrType::A32 => body.instruction(&Instruction::I32Const(cx.scratch_addr as u32 as i32)),
        AddrType::A64 => body.instruction(&Instruction::I64Const(cx.scratch_addr as i64)),
    };
}

/// `resolve` a host fn pointer to its table index. `precheck` already proved this resolves, but
/// lowering stays fallible anyway — the no-panic contract must not rest on two walks agreeing (the
/// resolver is an arbitrary `&dyn Fn` and may be stateful/impure, so "precheck saw `Some`" does not
/// guarantee this call does too). Reports the instruction's real pc via [`Ctx::pc`].
fn resolve_host(cx: &Ctx<'_>, ptr: usize) -> Result<u32, Refusal> {
    (cx.resolve)(ptr).ok_or(Refusal::HostFnUnresolved { pc: cx.pc })
}

/// `i32.const idx; call_indirect (type ...)` against the shared table (table index 0 — bound by
/// `(import "otter" "table")` on the JIT path, by the module's own declared table on the AOT path
/// since O-14 A-2 — either way it is table 0 and this call-site shape doesn't change).
fn emit_call_indirect(body: &mut Function, cx: &mut Ctx<'_>, table_slot: u32, ty: (&[ValType], &[ValType])) {
    body.instruction(&Instruction::I32Const(table_slot as i32));
    let type_index = cx.types.index_of(ty.0, ty.1);
    body.instruction(&Instruction::CallIndirect {
        type_index,
        table_index: 0,
    });
}

/// **Marshal** the `n = ` [`site_argc`] argument slots `l(base+1+k)` into this function's scratch
/// — `i32.const scratch_addr; local.get l(base+1+k); i64.store offset=8k` for each `k` (design
/// §2.5-1; the cranelift equivalent is an activation-local `StackSlot`, the ONE structural
/// difference §2.5 redesigned). `n` comes through [`site_argc`], the same single source
/// [`scratch_words`]' sizing scan reads (§2.5-1 no-dual-source rule), so every store offset
/// `8k < 8n <= 8 * scratch_words` — in-bounds of the caller's allocation by construction.
/// `check_func`'s `CallSlotOutOfRange` proof bounds every `l(base+1+k)` (§8d).
///
/// Shared by EVERY `Call`/`TailCall` lowering regardless of [`CallMode`] (O-14 A-3, design §5.1:
/// the marshaling step is explicitly the shared-not-diverging part of the call-site shape — only
/// what comes AFTER it switches on the mode, which is exactly the §5.2 R2 window boundary). Emits
/// no marshaling bytes at all when `n == 0` (a call-free-in-effect site — nothing to store).
/// Returns `(base_slot, n)` for the caller to finish the call-site shape with.
fn marshal_call_args(body: &mut Function, cx: &Ctx<'_>, inst: &Inst, base: u8, argc_op: u8) -> (i32, usize) {
    let base_slot = inst.operand(base);
    let n = site_argc(inst, argc_op);
    for k in 0..n {
        emit_scratch_addr_const(body, cx);
        // `base_slot + 1 + k` fits i32 without wrap: check_func proved base+1+argc <= nlocals and
        // the §4.6-5 precheck bounded nlocals below MAX_WASM_FUNCTION_LOCALS (50k).
        body.instruction(&Instruction::LocalGet(local(base_slot + 1 + k as i32)));
        body.instruction(&Instruction::I64Store(MemArg {
            offset: (k as u64) * 8,
            align: 3,
            memory_index: 0,
        }));
    }
    (base_slot, n)
}

/// [`CallMode::Dispatch`]'s call-site shape (design §4.3, mirroring `otter-cranelift`'s identical
/// marshal-then-call twins, its `Effect::Call`/`Effect::TailCall` lowering arms — same shape,
/// different target; structural anchors throughout, line numbers go stale). After
/// [`marshal_call_args`]:
///
/// **Call**: `local.get $ctx; local.get l(base); i32.const scratch_addr; i64.const n;`
/// `call_indirect (i32,i64,i32,i64)->i64` through `helper_slot` (`dispatch` or `tail`, §2.3
/// #7-#8). The callee word `l(base)` is a **runtime** i64 value passed as-is (a raw function
/// index — the receiving FFI boundary converts it FULL-WIDTH via `otter::interp::callee_index`
/// (O-14 §3.2a / A-C1: the u64 width check precedes any narrowing cast, so this wasm32 build
/// cannot truncate an out-of-range word into range), and the range check itself happens at the
/// head of `ModuleEngine::dispatch`'s trampoline loop, which a dispatched call reaches directly
/// and a tail signal reaches on the trampoline's NEXT iteration; `tail_signal` itself stores the
/// callee width-checked but not range-checked); `$ctx` is forwarded opaquely (§2.2). For `n == 0`
/// the scratch address is still passed but the callee side never reads it (the `argc <= 0 ||
/// args.is_null()` guard short-circuits to the empty slice in both `module_dispatch` and
/// `tail_signal`), matching cranelift's null-pointer convention in effect if not in value.
///
/// Leaves the helper's i64 result on the wasm stack: `Call` stores it to `l(dest)`, `TailCall`
/// drops it (`tail_pending`, not the value, is the discriminator — §4.3). Both call-site arms
/// live in the caller ([`emit_straightline_body`]/[`emit_dispatcher_body`]), same as before A-3
/// — this function is the marshal-through-`call_indirect` core they share.
fn emit_dispatch_call(
    body: &mut Function,
    cx: &mut Ctx<'_>,
    inst: &Inst,
    base: u8,
    argc_op: u8,
    helper_slot: u32,
) {
    let (base_slot, n) = marshal_call_args(body, cx, inst, base, argc_op);
    body.instruction(&Instruction::LocalGet(CTX_PARAM));
    body.instruction(&Instruction::LocalGet(local(base_slot)));
    // Deliberately a direct `i32.const`, NOT `emit_scratch_addr_const` (O-19 §2.1): Dispatch mode
    // is the JIT surface's only mode and that surface is A32-only (`EmitOpts` is not addrtype-
    // extended), so `DISPATCH_TY`'s `args` param is i32 unconditionally — routing this through
    // the addrtype branch would merely disguise that invariant.
    body.instruction(&Instruction::I32Const(cx.scratch_addr as u32 as i32));
    body.instruction(&Instruction::I64Const(n as i64));
    emit_call_indirect(body, cx, helper_slot, DISPATCH_TY);
}

/// [`CallMode::Table`]'s `Call` call-site shape (O-14 A-3, design §3.1 — literal). After
/// [`marshal_call_args`]:
///
/// ```wat
/// local.get l(base) ; i64.const NFUNCS ; i64.ge_u
/// if (result i64)
///   i64.const FILL                                  ;; OOB/negative callee → fill IS the result
/// else
///   local.get $ctx ; i32.const scratch_addr ; i64.const n
///   local.get l(base) ; i32.wrap_i64
///   call_indirect (type COMPILED_TY)                 ;; (i32 i32 i64)->i64, table 0
/// end
/// ```
///
/// **Why `i64.ge_u` alone is a correct fail-closed guard** (design §3.1 fail-closed-equivalence
/// note): the callee word is compared UNSIGNED, full 64-bit, as a plain wasm `i64` value — this is
/// a NATIVE wasm comparison, not a Rust `as` cast, so there is no narrowing step for a negative or
/// over-wide word to survive: `-1i64` reinterprets as `0xFFFF_FFFF_FFFF_FFFF`, `2^32` reinterprets
/// as itself, both `>= NFUNCS` for any realistic `NFUNCS`, so both take the `FILL` arm. This is
/// NOT the same class of correctness argument as O-14 §3.2a/A-C1 (which fixed the ENGINE's
/// `module_dispatch`/`tail_signal`/T0-interp receiving-side cast, `idx as usize`, where a genuine
/// narrowing truncation could otherwise happen on a wasm32-deployed engine BEFORE the range check
/// ran) — this emitted guard has no cast to truncate in the first place, so A-C1's fix is
/// orthogonal to this code path, not a precondition of it. Do not read "A-3 needed A-C1 to be
/// correct" into the §9 A-3 row's "A-C1 is a precondition" language: A-C1 gates A-3 because A-3's
/// callee word is the SAME runtime value the engine tiers also convert (so their two guards must
/// agree — §6's cross-mode observational-equivalence contract), not because this guard reuses (or
/// needs) A-C1's cast fix.
///
/// The `if (result i64)`/`else` shape (design §3.1: "가드의 return은 전체 함수 반환이 되어
/// 오답" — unlike `TailCall`'s guard, this one must NOT `return` early, so it produces a VALUE
/// instead) is [`BlockType::Result`] — the census-honest reading (design §3.1: a new BLOCK TYPE
/// shape, not a new instruction; [`CallMode::Table`]'s introduction is what turns this from
/// spec-legal-but-unexercised into REAL emitted output, closing the design's own "[추정→A-3에서
/// 실측화]" tag on this shape).
fn emit_table_call(
    body: &mut Function,
    cx: &mut Ctx<'_>,
    inst: &Inst,
    base: u8,
    argc_op: u8,
    nfuncs: u32,
    fill: Value,
) {
    let (base_slot, n) = marshal_call_args(body, cx, inst, base, argc_op);
    body.instruction(&Instruction::LocalGet(local(base_slot)));
    body.instruction(&Instruction::I64Const(nfuncs as i64));
    body.instruction(&Instruction::I64GeU);
    body.instruction(&Instruction::If(BlockType::Result(I64)));
    body.instruction(&Instruction::I64Const(fill));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(CTX_PARAM));
    emit_scratch_addr_const(body, cx);
    body.instruction(&Instruction::I64Const(n as i64));
    body.instruction(&Instruction::LocalGet(local(base_slot)));
    body.instruction(&Instruction::I32WrapI64);
    // NOT `emit_call_indirect` (design §3.1): that helper pushes a CONSTANT table-slot literal
    // before `call_indirect` (the Bin/Un/Host*/Dispatch-mode shape) — here the callee index is
    // already the dynamic value just pushed (`local.get l(base); i32.wrap_i64`), so emitting one
    // more `i32.const` would push a SECOND, wrong operand onto the stack.
    let type_index = cx.types.compiled();
    body.instruction(&Instruction::CallIndirect {
        type_index,
        table_index: 0,
    });
    body.instruction(&Instruction::End);
}

/// [`CallMode::Table`]'s `TailCall` call-site shape (O-14 A-3, design §3.2 — [스파이크] M4
/// sequence literal). After [`marshal_call_args`]:
///
/// ```wat
/// local.get l(base) ; i64.const NFUNCS ; i64.ge_u
/// if ; i64.const FILL ; return ; end                 ;; fail-closed: fill IS the tail result
/// local.get $ctx ; i32.const scratch_addr ; i64.const n
/// local.get l(base) ; i32.wrap_i64
/// return_call_indirect (type COMPILED_TY)
/// ```
///
/// The guard's `return` exits FIRST, so the block itself carries no value — [`BlockType::Empty`],
/// unlike [`emit_table_call`]'s `if (result i64)` (design §3.2: "가드의 return이 앞서 탈출하므로
/// terminator 제약과 충돌 없음" — [스파이크] M4 measured both the validator and a real execution
/// accept this). `return_call_indirect` is itself a terminator (a tail call IS the function's
/// return), so nothing follows it — this is what makes deep tail recursion native-stack-bounded
/// WITHOUT the engine trampoline loop (wasm's own tail-call mechanism does the looping, [스파이크]
/// M2: 2,000,000/2,000,001-deep mutual tail recursion completes in a 64 KiB wasm stack).
fn emit_table_tailcall(
    body: &mut Function,
    cx: &mut Ctx<'_>,
    inst: &Inst,
    base: u8,
    argc_op: u8,
    nfuncs: u32,
    fill: Value,
) {
    let (base_slot, n) = marshal_call_args(body, cx, inst, base, argc_op);
    body.instruction(&Instruction::LocalGet(local(base_slot)));
    body.instruction(&Instruction::I64Const(nfuncs as i64));
    body.instruction(&Instruction::I64GeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I64Const(fill));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(CTX_PARAM));
    emit_scratch_addr_const(body, cx);
    body.instruction(&Instruction::I64Const(n as i64));
    body.instruction(&Instruction::LocalGet(local(base_slot)));
    body.instruction(&Instruction::I32WrapI64);
    let type_index = cx.types.compiled();
    body.instruction(&Instruction::ReturnCallIndirect {
        type_index,
        table_index: 0,
    });
}

/// `Effect::Call`'s full lowering, mode-dispatched over [`CallMode`] (design §5.1 — the ONE spot
/// both body emitters, [`emit_straightline_body`] and [`emit_dispatcher_body`], call through, so
/// the mode switch is written once). The `local.set l(dest)` epilogue is OUTSIDE both call-window
/// definitions (design §5.2 R2: "공통 접미 `local.set l(dest)`는 창 밖") — same bytes in either
/// mode, emitted once here rather than inside each `emit_*_call`.
fn lower_call(body: &mut Function, cx: &mut Ctx<'_>, inst: &Inst, dest: u8, base: u8, argc: u8, fill: Value) {
    match cx.call_mode {
        CallMode::Dispatch => {
            let helper_slot = cx.helper.dispatch;
            emit_dispatch_call(body, cx, inst, base, argc, helper_slot);
        }
        CallMode::Table { nfuncs } => emit_table_call(body, cx, inst, base, argc, nfuncs, fill),
    }
    body.instruction(&Instruction::LocalSet(local(inst.operand(dest))));
}

/// `Effect::TailCall`'s full lowering, mode-dispatched over [`CallMode`] (design §5.1 — see
/// [`lower_call`]'s doc for why this lives in one shared spot). `CallMode::Dispatch` drops the
/// signal helper's result and returns `fill` (the engine trampoline consumes the SIGNAL, never
/// this value — design §4.3); `CallMode::Table` needs no epilogue here at all —
/// `return_call_indirect` (or the guard's own early `return`) already ended the function inside
/// [`emit_table_tailcall`].
fn lower_tailcall(body: &mut Function, cx: &mut Ctx<'_>, inst: &Inst, base: u8, argc: u8, fill: Value) {
    match cx.call_mode {
        CallMode::Dispatch => {
            let helper_slot = cx.helper.tail;
            emit_dispatch_call(body, cx, inst, base, argc, helper_slot);
            body.instruction(&Instruction::Drop);
            body.instruction(&Instruction::I64Const(fill));
            body.instruction(&Instruction::Return);
        }
        CallMode::Table { nfuncs } => emit_table_tailcall(body, cx, inst, base, argc, nfuncs, fill),
    }
}

// --- basic-block leaders (design §3; same derivation as otter-cranelift :141-173 / :838-854) ------

/// The sorted basic-block leader pcs: pc 0, every `JumpTo`/`JumpIf` target, and every
/// terminator-successor pc — semantically identical to `otter-cranelift`'s
/// `is_terminator`+`branch_targets`+fall-leader derivation. `check_func` proved every explicit
/// target lies in `0..code_len`; the bounds guards below keep this walk total (and panic-free)
/// anyway rather than trusting that proof twice.
fn leaders(isa: &Isa, f: &FuncDef) -> Vec<usize> {
    let mut leader = vec![false; f.code.len()];
    if let Some(l) = leader.first_mut() {
        *l = true;
    }
    for (pc, inst) in f.code.iter().enumerate() {
        let mut term = false;
        for eff in &isa.op(inst.op).effects {
            // Exhaustive, no wildcard — same completeness rule as `effect_is_control`.
            match eff {
                Effect::JumpTo { target } | Effect::JumpIf { target, .. } => {
                    term = true;
                    let t = inst.operand(*target);
                    if t >= 0
                        && let Some(l) = leader.get_mut(t as usize)
                    {
                        *l = true;
                    }
                }
                Effect::Halt { .. } | Effect::Return { .. } | Effect::TailCall { .. } => term = true,
                Effect::SetLocalAt { .. } | Effect::Guard { .. } | Effect::Call { .. } => {}
            }
        }
        if term && let Some(l) = leader.get_mut(pc + 1) {
            *l = true;
        }
    }
    leader
        .iter()
        .enumerate()
        .filter_map(|(pc, &is_l)| is_l.then_some(pc))
        .collect()
}

/// `br_table` ordinal of the leader at `target_pc`. Every call site passes a pc that IS a leader
/// by construction ([`leaders`] marked every jump target and every terminator successor, and the
/// §4.6-1/-2 prechecks refused every fall-off-the-end shape), so the `Err` arm is genuinely
/// unreachable — but it stays a defensive `Refusal` (the documented marker-string exception on
/// [`Refusal::UnsupportedEffect`], same as the body walk's), never a panic.
fn leader_ord(leaders: &[usize], target_pc: usize, at_pc: usize) -> Result<i32, Refusal> {
    match leaders.binary_search(&target_pc) {
        // ord < leaders.len() <= i32::MAX (the `TooManyLeaders` guard), so the cast cannot wrap.
        Ok(ord) => Ok(ord as i32),
        Err(_) => Err(Refusal::UnsupportedEffect {
            pc: at_pc,
            effect: "jump target is not a leader",
        }),
    }
}

// --- body emission --------------------------------------------------------------------------------

/// The W-1a straight-line body: pc order IS execution order (the caller verified the function has
/// no `JumpTo`/`JumpIf`), so instructions are lowered in sequence with no dispatcher. A
/// mid-function `Halt`/`Return`/`TailCall` (leader-splitting on the other backends) just makes
/// the tail dead code — T0 never reaches it either, and it lowers to *validating* dead wasm code.
fn emit_straightline_body(
    body: &mut Function,
    cx: &mut Ctx<'_>,
    isa: &Isa,
    f: &FuncDef,
    fill: Value,
) -> Result<(), Refusal> {
    let mut terminated = false;
    for (pc, inst) in f.code.iter().enumerate() {
        cx.pc = pc;
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::SetLocalAt { reg, value } => {
                    lower_expr(body, cx, inst, value)?;
                    body.instruction(&Instruction::LocalSet(local(inst.operand(*reg))));
                }
                // Halt ends the module, Return ends a frame — both are a wasm `return` here (the
                // emitted function is one native frame), same collapse as cranelift :963-969.
                Effect::Halt { value } | Effect::Return { value } => {
                    lower_expr(body, cx, inst, value)?;
                    body.instruction(&Instruction::Return);
                    terminated = true;
                }
                // Speculation marker — transparent no-op on every non-deopting tier (cranelift
                // :970, T0 :674); deopt is the single-function `Engine` layer's concern, not ours.
                Effect::Guard { cond: _ } => {}
                // W-2 (§4.3, mode-dispatched via `lower_call` since O-14 A-3 §5.1): marshal, then
                // either call_indirect `module_dispatch` (Dispatch mode) or guard + call_indirect
                // the callee word directly (Table mode, §3.1) — a `Call` is NOT a terminator
                // either way (execution resumes right after it).
                Effect::Call { dest, base, argc } => {
                    lower_call(body, cx, inst, *dest, *base, *argc, fill);
                }
                // W-2 (§4.3, mode-dispatched via `lower_tailcall` since O-14 A-3 §5.1): Dispatch
                // mode signals `tail_signal` and returns `fill` immediately (the engine's
                // trampoline loop re-enters the callee — `tail_pending`, not this value,
                // discriminates tail-vs-return, cranelift :1005-1016 shape-for-shape); Table mode
                // guards then `return_call_indirect`s the callee word directly (§3.2) — wasm's own
                // tail-call mechanism does the looping, no engine trampoline involved at all.
                Effect::TailCall { base, argc } => {
                    lower_tailcall(body, cx, inst, *base, *argc, fill);
                    terminated = true;
                }
                // JumpTo/JumpIf: the caller routed jump-containing functions to the dispatcher
                // path, so both are unreachable here — but stay a defensive `Err`, never a panic,
                // and explicit (no wildcard) so a new Effect variant is a compile error in EVERY
                // walk, not a silent skip. `effect` is an internal marker string, not a variant
                // name — the documented exception to `UnsupportedEffect`'s field contract (W-1a
                // panel #2a, lens C finding #2).
                Effect::JumpTo { .. } | Effect::JumpIf { .. } => {
                    return Err(Refusal::UnsupportedEffect {
                        pc,
                        effect: "jump on the straight-line path",
                    });
                }
            }
        }
    }
    if !terminated {
        // W-1b's §4.6-1 precheck (end fall-through ⇒ Refuse) makes this net genuinely
        // unreachable; it is kept defensively — never replace it with `unreachable` (design §3:
        // no runtime trap for a statically detectable shape).
        //
        // nlocals == 0 has no local 0 to read — emitting `local.get FIRST_LOCAL` there produced an
        // INVALID module returned as `Ok` (caught by W-1a panel #2a, lenses A#1/B#1: `check_func`
        // accepts a locals-free straight-line function, wasm validation then fails with "unknown
        // local 3"). A local 0, had it existed, would still hold `fill` at this point (nothing
        // wrote it), so `i64.const fill` is the same value by construction — and keeps the "Ok ⇒
        // validating module" contract for every accepted input.
        if f.nlocals == 0 {
            body.instruction(&Instruction::I64Const(fill));
        } else {
            body.instruction(&Instruction::LocalGet(FIRST_LOCAL));
        }
        body.instruction(&Instruction::Return);
    }
    Ok(())
}

/// The §3 dispatcher-loop body, for functions with `JumpTo`/`JumpIf` — handles ANY otter CFG
/// (irreducible ones included) without a relooper. Shape (leader ordinals `0..N`, innermost block
/// listed last):
///
/// ```wat
/// (block $trap                    ;; br_table default — depth N+1 from the dispatch point
///   (loop $top                    ;; depth N-1-k from inside leader k's region
///     (block $b{N-1} … (block $b1 (block $b0
///       local.get $state          ;; zero-initialized ⇒ first iteration dispatches to leader 0 (pc 0)
///       (br_table $b0 $b1 … $b{N-1} $trap))
///      ;; leader 0's region — ends in `return` or `local.set $state; br $top`
///     ) ;; leader 1's region … ) ;; … leader N-1's region
///   ))
/// unreachable                     ;; $trap lands here — see the coverage note in the body
/// ```
///
/// Every control transfer re-enters through the dispatcher: `JumpTo` and both `JumpIf` edges set
/// `$state` to the target leader's ordinal and `br $top` (§3 — the fall-through edge is a leader
/// too), and a non-terminator run reaching the next leader gets the same sequence (§3's B#3
/// fall-through boundary rule — cranelift's `if !terminated { jump }` :938-941 equivalent).
fn emit_dispatcher_body(
    body: &mut Function,
    cx: &mut Ctx<'_>,
    isa: &Isa,
    f: &FuncDef,
    leaders: &[usize],
    fill: Value,
    state_local: u32,
) -> Result<(), Refusal> {
    // <= i32::MAX by the caller's `TooManyLeaders` guard: every ordinal/depth cast below is exact.
    let nleaders = leaders.len() as u32;
    // br depth of $top from inside leader region `ord`: the still-open inner blocks are
    // $b{ord+1}..$b{N-1} (N-1-ord of them), then $top.
    let top_depth = |ord: usize| nleaders - 1 - ord as u32;

    body.instruction(&Instruction::Block(BlockType::Empty)); // $trap
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $top
    for _ in 0..nleaders {
        body.instruction(&Instruction::Block(BlockType::Empty)); // $b{N-1} … $b0 (innermost last)
    }
    // Dispatch: leader ordinal k → `br k` (= $bk, whose `end` is exactly where region k starts).
    // Default → $trap at depth N+1 (N leader blocks + the loop).
    body.instruction(&Instruction::LocalGet(state_local));
    let table: Vec<u32> = (0..nleaders).collect();
    body.instruction(&Instruction::BrTable(table.into(), nleaders + 1));
    body.instruction(&Instruction::End); // closes $b0 — leader 0's region begins

    let mut cur = 0usize; // ordinal of the region being emitted
    let mut terminated = false;
    for (pc, inst) in f.code.iter().enumerate() {
        cx.pc = pc;
        if pc != 0 && leaders.get(cur + 1) == Some(&pc) {
            // §3 B#3 fall-through boundary rule: a non-terminator run reaching the next leader
            // re-enters through the dispatcher, exactly like a jump to it.
            if !terminated {
                body.instruction(&Instruction::I32Const((cur + 1) as i32));
                body.instruction(&Instruction::LocalSet(state_local));
                body.instruction(&Instruction::Br(top_depth(cur)));
            }
            body.instruction(&Instruction::End); // closes $b{cur+1} — the next region begins
            cur += 1;
            terminated = false;
        }
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::SetLocalAt { reg, value } => {
                    lower_expr(body, cx, inst, value)?;
                    body.instruction(&Instruction::LocalSet(local(inst.operand(*reg))));
                }
                Effect::JumpTo { target } => {
                    let ord = leader_ord(leaders, inst.operand(*target) as usize, pc)?;
                    body.instruction(&Instruction::I32Const(ord));
                    body.instruction(&Instruction::LocalSet(state_local));
                    body.instruction(&Instruction::Br(top_depth(cur)));
                    terminated = true;
                }
                Effect::JumpIf { cond, target } => {
                    // Both edges re-enter through the dispatcher (§3: the fall-through is a
                    // leader too — pc+1 exists because §4.6-2 refused the trailing shape).
                    let taken = leader_ord(leaders, inst.operand(*target) as usize, pc)?;
                    let fallthrough = leader_ord(leaders, pc + 1, pc)?;
                    lower_expr(body, cx, inst, cond)?;
                    body.instruction(&Instruction::I64Eqz); // 1 ⇔ cond == 0 ⇔ NOT taken (T0 :911)
                    body.instruction(&Instruction::If(BlockType::Empty));
                    body.instruction(&Instruction::I32Const(fallthrough));
                    body.instruction(&Instruction::LocalSet(state_local));
                    body.instruction(&Instruction::Else);
                    body.instruction(&Instruction::I32Const(taken));
                    body.instruction(&Instruction::LocalSet(state_local));
                    body.instruction(&Instruction::End);
                    body.instruction(&Instruction::Br(top_depth(cur)));
                    terminated = true;
                }
                Effect::Halt { value } | Effect::Return { value } => {
                    lower_expr(body, cx, inst, value)?;
                    body.instruction(&Instruction::Return);
                    terminated = true;
                }
                Effect::Guard { cond: _ } => {}
                // W-2/A-3 (§4.3/§5.1): identical lowering to the straight-line path's arms via the
                // SAME shared `lower_call`/`lower_tailcall` (see those for the full rationale) —
                // `Call` is a plain non-terminator effect inside its leader region, `TailCall` ends
                // the region (it was ALREADY a terminator in `leaders`/`effect_is_control`, so no
                // leader-derivation change from A-3's call-site-shape swap).
                Effect::Call { dest, base, argc } => {
                    lower_call(body, cx, inst, *dest, *base, *argc, fill);
                }
                Effect::TailCall { base, argc } => {
                    lower_tailcall(body, cx, inst, *base, *argc, fill);
                    terminated = true;
                }
            }
        }
    }
    if !terminated {
        // §4.6-1 refused every end-fall-through shape, so this net is unreachable — kept
        // defensively, mirroring the straight-line net (never replace with `unreachable`).
        body.instruction(&Instruction::I64Const(fill));
        body.instruction(&Instruction::Return);
    }
    body.instruction(&Instruction::End); // closes $top
    body.instruction(&Instruction::End); // closes $trap
    // $trap lands here. The §4.6-1/-2 prechecks statically excluded every fall-off shape and
    // every explicit target / fall-through edge is a leader with its own br_table entry, so this
    // trap's coverage is exactly "the i32 $state got corrupted" — nothing else (§3 correction:
    // the earlier "default is unreachable because check_func excludes it" claim was narrowed to
    // this, and statically-detectable shapes are refused instead of trapped).
    body.instruction(&Instruction::Unreachable);
    Ok(())
}

// --- the backend entry point ----------------------------------------------------------------------

/// Lower function `func_idx` (its [`FuncDef`] is `f`; `fill`/`host_ctx` are the owning module's
/// fill and `Module::host_ctx` words, exactly as in `otter-cranelift::compile_dispatch_fn`) to the
/// bytes of a standalone wasm module (shape: crate doc / design §2.2), or refuse ([`Refusal`]).
///
/// * `helper_idx` — table indices of the engine helpers (see [`HelperIdx`]).
/// * `resolve` — host-fn pointer (as `usize`) → table index, for the fn pointers the IR carries in
///   `Host1`/`Host2`/`HostCtx1`/`HostCtx2` (§2.3 #3-#6). `None` ⇒ [`Refusal::HostFnUnresolved`].
/// * `scratch_addr` (since W-2) — the linear-memory address of **this function's** argument-
///   marshaling scratch (design §2.5-1), which the **caller allocates and owns**: at least
///   [`scratch_words`]`(isa, f) * 8` bytes, 8-byte aligned, live (and address-stable — add-only)
///   for as long as the emitted code may run, and aliased by **no live Rust borrow while the
///   emitted code executes** (§13-7's dual invariant; the engine-side allocation norms are
///   §13-2/§13-5 — W-3's concern). Every `Call`/`TailCall` site bakes this address as the store
///   target and the `args` argument of dispatch/tail (§4.3). For a function whose
///   `scratch_words` is `0` the value is never baked into a store, and never read by a callee
///   either (a zero-argc site passes it with `argc = 0`, which both helpers short-circuit to the
///   empty slice before touching the pointer) — so ANY value, including a dangling/reserved one,
///   is harmless there (the pre-W-2 test suites pass an arbitrary reserved address for exactly
///   this reason).
///
/// `otter::validate::check_func` is called first, **always** (§8d — its `Ok` is the static proof
/// the emitted code's local indices, br_table targets and, from W-2, `Call`/`TailCall` marshaling
/// slot reads `l(base) .. l(base+1+argc)` lean on); then the §4.6 structural precheck (complete
/// as of W-1b: §4.6 1-5). Only then is anything emitted, so a `Refusal` never leaves a half-built
/// module behind.
///
/// **Implementation-limit precheck (closes the W-1a gap)**: wasm engines cap a function at
/// [`MAX_WASM_FUNCTION_LOCALS`] locals, params included — far below `check_func`'s
/// `MAX_SAFE_NLOCALS` (~67M). W-1a documented this as a known gap (an over-the-cap function came
/// back `Ok` with bytes no engine would validate, surfacing as the host's `-1` misattributed to a
/// host limit); the §4.6-5 precheck now refuses those functions up front
/// ([`Refusal::WasmLocalsLimitExceeded`]), so `Ok` again implies a validating module (W-1a panel
/// #2a, lens A#2).
///
/// **Body lowering (W-1b, design §3)**: a function containing `JumpTo`/`JumpIf` lowers through
/// the dispatcher loop — basic-block leaders (pc 0 + every jump target + every terminator
/// successor; the same derivation `otter-cranelift` uses) become nested `block`s inside a `loop`,
/// entered through a `br_table` on an i32 `$state` local (declared after the otter locals;
/// wasm-zero-initialized, so the first iteration dispatches to leader 0 = pc 0). Every control
/// transfer — `JumpTo`, both `JumpIf` edges, and a non-terminator run falling into the next
/// leader (§3's B#3 boundary rule) — sets `$state` to the target leader's ordinal and re-enters
/// the loop, so ANY otter CFG, irreducible ones included, lowers without a relooper. See the
/// dispatcher emitter's doc (`emit_dispatcher_body`) for the exact wat skeleton and the
/// `br_table`-default coverage note. Jump-free functions keep the W-1a straight-line emission
/// instead: pc order IS execution
/// order there, so the dispatcher would add bytes (`loop` + N `block`s + `br_table` + `$state`)
/// and no behavior — and specializing keeps the W-1a output byte-for-byte (the structural tests
/// pin it).
///
/// The emitted prologue (mirroring `otter-cranelift`'s, :864-904):
/// 1. every otter local ← `fill` (the embedder's "undefined"),
/// 2. parameter `i` ← `i64.load args+8i` **iff `i < argc`** (argc-guard: an absent argument stays
///    `fill` and its memory is never read),
/// 3. `consts[j]` ← the slot right after the `nparams` parameter slots.
// 8 parameters is the signature design §9 fixes (index externalization made helper indices and the
// resolver explicit parameters — the whole point of §2.3/§8a — and W-2 added the announced
// `scratch_addr`); a bundling struct would only rename the coupling, so the lint is waived here.
// O-17 deliberately did NOT grow this to 9: the new module-shape switch rides in [`EmitOpts`]
// through `compile_dispatch_fn_with`, and this function stays the unchanged non-shared default
// (see `EmitOpts`'s doc for the full signature-stability rationale).
#[allow(clippy::too_many_arguments)]
pub fn compile_dispatch_fn(
    isa: &Isa,
    func_idx: usize,
    f: &FuncDef,
    fill: Value,
    host_ctx: Value,
    helper_idx: HelperIdx,
    resolve: &dyn Fn(usize) -> Option<u32>,
    scratch_addr: u32,
) -> Result<Vec<u8>, Refusal> {
    // O-19 C1 (panel tech-N1): this legacy entry point KEEPS its u32 `scratch_addr` — the O-17
    // byte-pin's meaning is preserved and no pre-O-19 caller churns — and upcasts internally;
    // the u64 width lives on the opts route (`compile_dispatch_fn_with`) and the internal `Ctx`.
    compile_dispatch_fn_with(
        isa,
        func_idx,
        f,
        fill,
        host_ctx,
        helper_idx,
        resolve,
        u64::from(scratch_addr),
        EmitOpts::new(),
    )
}

/// [`compile_dispatch_fn`] plus the O-17 module-shape options ([`EmitOpts`]) — the only
/// difference `opts` can currently make is the `otter.memory` import's declared type
/// (non-shared `(memory 0)` vs shared `(memory 0 65536 shared)`, design §5.2 v1.1); the
/// function body's lowering is bit-identical either way. With `EmitOpts::new()` the output is
/// byte-for-byte `compile_dispatch_fn`'s (pinned by the structure tests).
///
/// `scratch_addr` is `u64` since O-19 C1 (a recorded pre-1.0 breaking change, panel tech-N1:
/// the u64 width rides the opts route + internal `Ctx`; the legacy [`compile_dispatch_fn`]
/// keeps u32 + internal upcast). This surface stays A32-only regardless (§2.1: `EmitOpts` is
/// deliberately NOT memory64-extended — a JIT deployment's memory is the engine's own), so an
/// address whose scratch span would cross `2^32` is refused exactly as before
/// ([`Refusal::ScratchOutOfAddressSpace`]).
#[allow(clippy::too_many_arguments)] // the §9 8-param shape + the O-17 opts bundle (its doc has the rationale)
pub fn compile_dispatch_fn_with(
    isa: &Isa,
    func_idx: usize,
    f: &FuncDef,
    fill: Value,
    host_ctx: Value,
    helper_idx: HelperIdx,
    resolve: &dyn Fn(usize) -> Option<u32>,
    scratch_addr: u64,
    opts: EmitOpts,
) -> Result<Vec<u8>, Refusal> {
    // `func_idx` names the function on the native backends (JIT symbol names); a wasm module has no
    // symbol to name (and §5.2 forbids extra sections, so no name section either). Kept for API
    // parity with `otter-cranelift::compile_dispatch_fn` and for future diagnostics.
    let _ = func_idx;

    // A32 unconditionally — the JIT surface is not addrtype-parameterized (O-19 §2.1 [결정]).
    let mut types = Types::new(AddrType::A32);
    let compiled_ty = types.compiled();
    // `CallMode::Dispatch`, unconditionally: this is the JIT-shaped single-function module's ONLY
    // call-lowering mode (design §5.1/§9 A-3 row — A-3 only inverts `compile_module`'s AOT
    // surface, this entry point is untouched byte-for-byte).
    let body = emit_function_body(
        isa,
        f,
        fill,
        host_ctx,
        helper_idx,
        resolve,
        scratch_addr,
        &mut types,
        CallMode::Dispatch,
    )?;

    // --- module assembly: type, import, function, export, code — and NOTHING else (§5.2: no start
    // section, no custom sections; instantiation runs zero guest code) ---
    let imports = memory_and_table_imports(opts.shared_memory);

    let mut functions = FunctionSection::new();
    functions.function(compiled_ty);

    let mut exports = ExportSection::new();
    exports.export("f", ExportKind::Func, 0); // zero fn imports ⇒ our function is index 0

    let mut code = CodeSection::new();
    code.function(&body);

    let mut module = Module::new();
    module.section(&types.section);
    module.section(&imports);
    module.section(&functions);
    module.section(&exports);
    module.section(&code);
    Ok(module.finish())
}

/// The ONE body emitter both public entry points share (O-14 design §5.1 — the "no second
/// emission path" review rule in code form): everything [`compile_dispatch_fn_with`] does per
/// function up to and including the body's final `end`, parameterized by the type-interner scope
/// — a fresh per-function [`Types`] for the JIT-shaped single-function module, the shared
/// module-scope one for [`compile_module`]'s N-in-1 product (design §2.1: the `Types` interning
/// was already module-scope structured, so the promotion is literally "pass the same interner N
/// times") — and, since A-3, by [`CallMode`] (design §5.1): [`compile_dispatch_fn_with`] always
/// passes `CallMode::Dispatch`, [`compile_module`] always `CallMode::Table`. The CALLER interns
/// [`COMPILED_TY`] first, so type index 0 is the function's own type in every module shape.
///
/// Runs the §4.2 per-function acceptance predicate in its normative order — `check_func` → the
/// §4.6 structural precheck (host resolution included) → the scratch address-space check — then
/// emits prologue + body. A `Refusal` never leaves a half-built body behind (the returned
/// [`Function`] only exists on `Ok`).
#[allow(clippy::too_many_arguments)] // the §9 8-param shape minus module assembly, plus the interner + A-3's call_mode
fn emit_function_body(
    isa: &Isa,
    f: &FuncDef,
    fill: Value,
    host_ctx: Value,
    helper_idx: HelperIdx,
    resolve: &dyn Fn(usize) -> Option<u32>,
    scratch_addr: u64,
    types: &mut Types,
    call_mode: CallMode,
) -> Result<Function, Refusal> {
    validate::check_func(isa, f).map_err(Refusal::ValidationFailed)?;
    precheck(isa, f, resolve)?;

    // Statically-detectable always-trap shape (panel #3 lens A finding P2): if the scratch's last
    // byte would sit past the linear-memory address space, EVERY execution of a marshaling store
    // must trap — a real allocation can never legitimately produce such an address (a 4 GiB
    // memory's final byte is `2^32 - 1`), so per this crate's own norm (§3/§4.6: never a runtime
    // trap for a statically detectable shape) it is refused, not emitted. A scratch ending
    // exactly AT the boundary is legal (`>` comparison), and a call-free function (`words == 0`)
    // never bakes the address at all. `checked_add` (O-19 C1): `scratch_addr` is a full u64 now,
    // so the sum itself can wrap — an overflowing span is the same always-trap shape, refused.
    // The bound is addrtype-keyed (O-19 §2.2 #7): A32 keeps the wasm32 `2^32`; A64 is CAPPED at
    // `2^63`, deliberately short of the full `2^64` [§2.4 결정] — the "span may legally end
    // exactly at the address-space top" edge would need an exclusive end of `2^64`, which a u64
    // cannot represent; capping at `2^63` keeps u64 arithmetic total everywhere (no u128, no
    // dual notation) at zero practical cost (no execution environment provides a 2^63-byte
    // linear memory).
    let words = max_scratch_words(isa, f);
    let limit: u64 = match types.addr {
        AddrType::A32 => 1u64 << 32,
        AddrType::A64 => 1u64 << 63,
    };
    if words > 0
        && scratch_addr
            .checked_add(8 * (words as u64))
            .is_none_or(|end| end > limit)
    {
        return Err(Refusal::ScratchOutOfAddressSpace { scratch_addr, words });
    }

    // Leader derivation + path selection (design §3). `use_dispatcher` keys on the presence of a
    // jump, NOT on the leader count: mid-function `Halt`/`Return` split leaders too, but without
    // jumps pc order IS execution order and the straight-line emission stays correct (the split-off
    // tail is dead on T0 and lowers to validating dead wasm code). A new jump-like Effect variant
    // cannot slip past this `matches!`: adding one breaks the exhaustive matches in `precheck`/
    // `effect_is_control`/both emitters first.
    let leaders = leaders(isa, f);
    let use_dispatcher = f.code.iter().any(|inst| {
        isa.op(inst.op)
            .effects
            .iter()
            .any(|e| matches!(e, Effect::JumpTo { .. } | Effect::JumpIf { .. }))
    });
    if use_dispatcher && i32::try_from(leaders.len()).is_err() {
        return Err(Refusal::TooManyLeaders {
            nleaders: leaders.len(),
        });
    }

    // All otter locals are i64 (design §3: register allocation is delegated to the host JIT).
    // `check_func` bounds nlocals <= MAX_SAFE_NLOCALS (~2^26) — and the §4.6-5 precheck tightened
    // that to MAX_WASM_FUNCTION_LOCALS — so the u32 casts below cannot wrap. The dispatcher path
    // appends its `$state` i32 AFTER the otter locals (wasm-zero-initialized ⇒ the first loop
    // iteration dispatches to leader 0, i.e. pc 0 — no explicit init needed).
    let mut body = if use_dispatcher {
        Function::new(vec![(f.nlocals as u32, I64), (1, I32)])
    } else {
        Function::new(vec![(f.nlocals as u32, I64)])
    };
    let state_local = FIRST_LOCAL + f.nlocals as u32;

    // --- prologue ---
    for j in 0..f.nlocals {
        body.instruction(&Instruction::I64Const(fill));
        body.instruction(&Instruction::LocalSet(local(j as i32)));
    }
    for i in 0..f.nparams {
        // if argc > i { local[i] = load(args + 8*i) }  — cranelift :886-900 argc-guard mirror.
        body.instruction(&Instruction::LocalGet(ARGC_PARAM));
        body.instruction(&Instruction::I64Const(i as i64));
        body.instruction(&Instruction::I64GtS);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(ARGS_PARAM));
        body.instruction(&Instruction::I64Load(MemArg {
            offset: (i as u64) * 8,
            align: 3,
            memory_index: 0,
        }));
        body.instruction(&Instruction::LocalSet(local(i as i32)));
        body.instruction(&Instruction::End);
    }
    for (j, c) in f.consts.iter().enumerate() {
        body.instruction(&Instruction::I64Const(*c));
        body.instruction(&Instruction::LocalSet(local((f.nparams + j) as i32)));
    }

    // --- body (design §3: dispatcher loop for jump-containing functions, W-1a straight-line
    // emission otherwise — see the path-selection comment above and the emitters' own docs) ---
    let mut cx = Ctx {
        types,
        helper: helper_idx,
        resolve,
        host_ctx,
        scratch_addr,
        call_mode,
        pc: 0,
    };
    if use_dispatcher {
        emit_dispatcher_body(&mut body, &mut cx, isa, f, &leaders, fill, state_local)?;
    } else {
        emit_straightline_body(&mut body, &mut cx, isa, f, fill)?;
    }
    body.instruction(&Instruction::End);
    Ok(body)
}

/// Pushes the shared `otter.memory` import (O-17 §5.2 v1.1 shape) onto `imports` — the ONE
/// memory-import-building logic every module shape uses (§2.2: memory stays an import in EVERY
/// AOT shape, A-1 through A-3 alike — M2a's memory owner is the engine-library build, decided at
/// link time, so the emitter must not pre-empt that decision by declaring its own).
fn push_memory_import(imports: &mut ImportSection, shared_memory: bool, addr: AddrType) {
    // O-17 (§5.2 v1.1): the memory import's declared type follows the caller's declaration of
    // the deployment's actual linear memory. Both A32 shapes are universal supertypes within
    // their sharedness class (limits subtyping: declared.min <= actual.min ∧ declared.max >=
    // actual.max, with `shared` matched exactly — SHARED_MEMORY_IMPORT_MAX_PAGES's doc has the
    // M5 argument); sharedness itself never coerces, which is exactly why it must be a parameter
    // and not a single always-works shape. O-19 adds the A64 arm in the same spot O-17 added
    // shared (§2.2 #5): `(memory i64 0)` — min 0, no max, memory64.
    let mem = match (addr, shared_memory) {
        (AddrType::A32, true) => MemoryType {
            minimum: 0, // min 0 ⇒ links against any current size (§5.2)
            // shared memories MUST declare a max; 65536 pages ≥ every wasm32 actual max (M5)
            maximum: Some(SHARED_MEMORY_IMPORT_MAX_PAGES),
            memory64: false,
            shared: true,
            page_size_log2: None,
        },
        (AddrType::A32, false) => MemoryType {
            minimum: 0, // min 0 ⇒ links against any current size (§5.2)
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        },
        // shared × A64 cannot reach here: `compile_module` refuses the combination fail-closed
        // BEFORE any emission ([`AotError::SharedMemory64Unsupported`], O-19 §2.1 [결정] — the
        // shared max-pages figure is a wasm32-only measurement), and the JIT surface has no A64
        // at all. The arm ignores `shared` so this match stays total without a panic path (the
        // crate's no-panic contract).
        (AddrType::A64, _) => MemoryType {
            minimum: 0, // min 0 ⇒ links against any current-size 64-bit memory
            maximum: None,
            memory64: true,
            shared: false,
            page_size_log2: None,
        },
    };
    imports.import("otter", "memory", EntityType::Memory(mem));
}

/// The `otter.memory` + `otter.table` import pair for the JIT-shaped single-function module
/// (design §5.2) — unchanged since O-13. [`compile_module`]'s AOT product no longer imports a
/// table AT ALL as of A-2 (§2.3: "otter.table" import → module-DECLARED table + active element —
/// see `compile_module`'s own import/table/element assembly).
fn memory_and_table_imports(shared_memory: bool) -> ImportSection {
    let mut imports = ImportSection::new();
    // A32 unconditionally — the JIT surface is not addrtype-parameterized (O-19 §2.1).
    push_memory_import(&mut imports, shared_memory, AddrType::A32);
    imports.import(
        "otter",
        "table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 0, // min 0, no max ⇒ links against any shared table, growable or not (§5.2)
            maximum: None,
            shared: false,
        }),
    );
    imports
}

// ========================== O-14 A-1/A-2/A-3: AOT whole-module entry point ==========================
// Design: `.claude-research-library/otter-o14-aot-core-design.md` — §4.1 (signature), §4.2 (error
// totalization + single-pass determinism), §2.5 (scratch self-assignment), §2.1 (module-scope type
// interning), §2.2 (function imports), §2.3 (declared table + element), §2.4 (exports), §3.1/§3.2
// (A-3 call-lowering inversion), §9 A-1/A-2/A-3 (increment scope history below).
//
// A-1→A-2 retirement note: A-1's `AOT_DISPATCH_SENTINEL`/`AOT_TAIL_SENTINEL` constants (baked OOB
// placeholder table slots that made an A-1 product byte-valid but call-UNEXECUTABLE, §5.2) were
// REMOVED as of A-2 — every `Call`/`TailCall` site baked a REAL, table-backed slot instead, because
// A-2's whole point was standing up the internal link the sentinels were a placeholder FOR.
//
// A-2→A-3 retirement note: A-2's two provisional `otter_helpers.{dispatch,tail}` trampoline table
// slots — and the dispatch-form `call_indirect` every `Call`/`TailCall` site routed through them —
// are REMOVED as of A-3 ([`AotImportKind`]'s "A-2→A-3 retirement note" has the enum-level detail).
// `Call`/`TailCall` now guard (`i64.ge_u` against `nfuncs`) then `call_indirect`/
// `return_call_indirect` the callee word DIRECTLY (§3.1/§3.2, [`CallMode::Table`]) — no dispatch
// helper, no engine trampoline loop, no A-2-provisional table oversizing. The record==live gate's
// canonical `H*` injection (§5.2) no longer needs dispatch/tail sentinel values for this reason: the
// R2 window comparison (§5.2, gate test suite) compares STRUCTURE, not raw bytes, across the two
// call-lowering modes at every `Call`/`TailCall` site — see the test suite's own module doc.

/// Options for [`compile_module`] (design §4.1). Construct via [`AotOpts::new`] + the `with_*`
/// builders (`#[non_exhaustive]`, the same forward-compatibility posture as [`EmitOpts`]).
///
/// `scratch_base` has **no default** on purpose (§4.1): address 0's neighborhood is plausibly
/// engine-library data, so no "safe default" exists — forcing the caller to pick a base is what
/// keeps the §2.5 reservation duty (the manifest's `scratch_region` must be reserved by the
/// linker/embedder) in the caller's face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AotOpts {
    /// First byte of the emitter-owned scratch region (§2.5). MUST be 8-aligned — violation is
    /// [`AotError::ScratchBaseMisaligned`], the machine-enforced form of the §2.5 MUST.
    ///
    /// **Caller norm — scratch floor (O-16c′ design §2-2, normative)**: when the artifact is
    /// statically linked against an engine-library b′ (the M2a chain), `scratch_base` MUST be
    /// ≥ the link partner's `heap_floor` — the value recorded in the b′ sidecar
    /// (`otter_helpers.manifest.json`), which is the floor's TRUTH (it moves with the library
    /// generation; at the current generation: non-shared 0x100000, +atomics 0x110000). The
    /// floor formula includes the `memory_min × 65536` term (passive-segment blind-spot
    /// absorber), and the link gate `check --scratch` (iii) enforces this norm fail-closed;
    /// recovery for a violation is RE-EMIT with a new base (§2.5-iv). See also O-14 §12.2.
    ///
    /// `u64` since O-19 C1 (§2.2 #8, a recorded pre-1.0 breaking change): the A64 address space
    /// needs the width, and the u64 representation is upward-compatible on A32 (a base whose
    /// span crosses the A32 boundary still refuses through the §4.6 bound, exactly as before).
    pub scratch_base: u64,
    /// Same meaning (and same emitted import shape) as [`EmitOpts::shared_memory`] (§2.7).
    pub shared_memory: bool,
    /// Emit the module in the O-19 **A64 (memory64) shape** — MIXED on purpose (§2.1 [결정]):
    /// a 64-bit `(memory i64 0)` `otter.memory` import while the declared funcref table (and
    /// the whole `call_indirect` machinery, `i32.wrap_i64` callee narrowing included) stays
    /// 32-bit — S2-M1-3 measured the mixed shape valid + executable, addrtype being
    /// per-memory/per-table in wasm, and it keeps the table lowering byte-identical across
    /// addrtypes (the modulo-addrtype differential gate's ground). What changes under `true`
    /// (the §2.2 inventory, exhaustively): the `CompiledFn` type's two pointer params widen to
    /// i64 ([`Types::compiled`]), every baked scratch-address constant becomes `i64.const`
    /// ([`emit_scratch_addr_const`] — validator-forced, S2-M1-2), the memory import gains
    /// `memory64`, and the scratch span bound moves to the §2.4-capped `2^63`. `false` (the
    /// default) is the pre-O-19 wasm32 shape, byte-for-byte (golden-vector pinned).
    ///
    /// v1 limits: `shared_memory` × `memory64` is refused fail-closed
    /// ([`AotError::SharedMemory64Unsupported`]), and the O-16c′ link/sidecar chain stays
    /// wasm32-only (§2.5 — an A64 manifest cannot become a v1 sidecar).
    pub memory64: bool,
}

impl AotOpts {
    /// `scratch_base` + non-shared, wasm32 (A32) memory import.
    pub const fn new(scratch_base: u64) -> Self {
        AotOpts {
            scratch_base,
            shared_memory: false,
            memory64: false,
        }
    }

    /// Builder toggle for [`AotOpts::shared_memory`].
    #[must_use]
    pub const fn with_shared_memory(mut self, shared: bool) -> Self {
        self.shared_memory = shared;
        self
    }

    /// Builder toggle for [`AotOpts::memory64`] (O-19 — the addrtype parameter's ONLY public
    /// exposure; the JIT surface's [`EmitOpts`] is deliberately not extended, §2.1).
    #[must_use]
    pub const fn with_memory64(mut self, memory64: bool) -> Self {
        self.memory64 = memory64;
        self
    }
}

/// A host-fn import name, as supplied by [`compile_module`]'s `resolve_host` (design §4.1 — the
/// no_std+alloc newtype). At A-1 the name is consumed for [`AotError::DuplicateHostImportName`]
/// detection and for the §4.1 resolver-determinism norm (first-resolution order = slot order);
/// A-2 additionally emits it as the `otter_host.<name>` function import.
///
/// Deliberately a plain `pub` tuple field, no `#[non_exhaustive]` (A-5 semver closeout):
/// EMBEDDERS construct this value inside their resolver — restricting construction would break
/// the `resolve_host` contract itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostImportName(pub String);

/// Which canonical arithmetic helper an [`AotImportKind::Helper`] entry names. (The design §4.1
/// sketch writes `Helper(HelperIdx)`, but [`HelperIdx`] already names the JIT-surface 5-slot
/// struct — a per-import entry names exactly ONE helper, so the payload is this discriminator;
/// recorded as a naming adaptation, not a semantic change.)
///
/// **Deliberately NOT `#[non_exhaustive]`** (A-5 disposition of the A-2 review carry-over): the
/// helper set is frozen by the JIT-surface [`HelperIdx`] shape (design §4.3) and the manifest's
/// `helper_slots: [u32; 3]` — a 4th helper would be a coordinated breaking change across the
/// whole AOT surface regardless of this enum's attribute. Keeping it exhaustive means downstream
/// import-wiring code (extern-list builders keyed on [`AotImportKind::Helper`]) breaks LOUDLY at
/// compile time on such a change, instead of silently falling through a forced wildcard arm into
/// a misaligned extern list — the fail-closed direction for exactly this consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperKind {
    /// `otter_binop` — `(i64, i64, i32, i32) -> i64`.
    Binop,
    /// `otter_convert` — `(i64, i32, i32) -> i64`.
    Convert,
    /// `otter_unop` — `(i64, i32, i32) -> i64`.
    Unop,
}

/// What one [`AotImport`] entry is (design §4.1). The wasm-level function TYPE is deliberately
/// not duplicated here: its **canonical source is the artifact's type section** (wasm bytes) —
/// this kind is a convenience copy for the b′ export spec, and on any disagreement the bytes win
/// (§4.1, panel-reinforced).
///
/// **A-2→A-3 retirement note**: A-2 temporarily added a `Trampoline(TrampolineKind)` variant for
/// the `otter_helpers.{dispatch,tail}` provisional stubs (§9 A-2 intermediate-shape norm) — A-3
/// deletes that variant AND the `TrampolineKind` enum with it OUTRIGHT (not merely unused: those
/// two imports no longer exist in ANY A-3 artifact, since `Call`/`TailCall` now guard +
/// `call_indirect`/`return_call_indirect` the callee word directly, §3.1/§3.2). Breaking on a
/// `#[non_exhaustive]` enum, but this crate is path-dep pre-1.0 (`otter-reusability-plan`) —
/// A-2's retirement of the equally-provisional `AotImportKind::Table` variant is the precedent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AotImportKind {
    /// `otter.memory` — the linear-memory import (permanent: M2a's memory owner is the
    /// engine-library build, decided at link time — §2.2).
    Memory,
    /// `otter_helpers.otter_{binop,convert,unop}` — emitted since A-2 (§2.2).
    Helper(HelperKind),
    /// `otter_host.<name>` — emitted since A-2 (§2.2); `arity`/`ctx` are a best-effort DESCRIPTIVE
    /// label, not a second source of truth (the type section is — struct doc). Specifically: a
    /// 2-argument host fn pointer is wasm-indistinguishable between `Host2` (arity 2, no ctx) and
    /// `HostCtx1` (arity 1, ctx baked — 1 explicit arg + the prepended `host_ctx` word) — BOTH
    /// lower to the identical 2×i64→i64 import type (§2.3's `HOST2_TY`), so whichever `Expr`
    /// variant resolves the pointer FIRST picks the label here; on disagreement the bytes win.
    Host {
        /// Number of i64 value arguments (1 or 2), NOT counting the prepended `host_ctx` word.
        arity: u8,
        /// `true` for `HostCtx1`/`HostCtx2` (the baked `host_ctx` word is prepended).
        ctx: bool,
    },
}

/// One import of the emitted module (design §4.1) — the machine-readable channel the §2.2 naming
/// contract travels through (manifest-carried, so a b″-driven namespace rename stays mechanical —
/// §11-1).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AotImport {
    /// Import module namespace (e.g. `"otter"`, from A-2 also `"otter_helpers"`/`"otter_host"`).
    pub module: String,
    /// Import name within the namespace.
    pub name: String,
    /// What the import is — see [`AotImportKind`] (type authority = the artifact's type section).
    pub kind: AotImportKind,
}

/// One function's self-assigned marshaling scratch (design §2.5): `manifest.scratch[func_idx]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AotScratchEntry {
    /// First byte of this function's scratch run. For a `words == 0` function the address is
    /// never STORED through (there is no marshaling store to emit) but may still be BAKED as the
    /// dead `args` operand of an `argc == 0` call site — where any value is harmless (the callee
    /// short-circuits before touching the pointer; see [`compile_dispatch_fn`]'s `scratch_addr`
    /// doc), so the entry records the cursor position for determinism, not for reservation (its
    /// span is empty). `u64` since O-19 C1 (§2.2 #9): the entry now records the EXACT §2.5
    /// cursor value — the pre-O-19 `u32`-saturated stand-in for a cursor sitting at `2^32` is
    /// gone (that edge only ever arose for a `words == 0` entry or as a refusal diagnostic).
    pub addr: u64,
    /// The function's [`scratch_words`] — its span is `8 * words` bytes at `addr`.
    pub words: usize,
}

/// The machine-readable half of an [`AotArtifact`] (design §4.1). A **Rust value** in v1 — the
/// serialization format is the consumer's (b″'s) decision, deferred to O-16c′ (§11-2).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AotManifest {
    /// `= m.entry` (§2.3 identity mapping: otter function index = table slot) — the table SLOT
    /// the `otter_entry` export names. Since A-2 the export's actual wasm FUNCTION INDEX is
    /// `nimports_func + entry_slot` (function imports precede every module-defined function in
    /// the function index space — §2.3); `entry_slot` itself stays the slot-space value, the one
    /// meaningful across every increment (A-1's imports-free function index space made the two
    /// numbers coincide, which is why this field's meaning did not need to change at A-2).
    pub entry_slot: u32,
    /// Number of emitted functions — since A-3 this is ALSO the [`CallMode::Table`] guard
    /// boundary baked into every `Call`/`TailCall` site's `i64.ge_u` (design §3.1/§3.2): a callee
    /// word `>= nfuncs` (unsigned, full 64-bit) never reaches `call_indirect`/
    /// `return_call_indirect` — it takes the `fill` arm instead.
    pub nfuncs: u32,
    /// Table slots of binop/convert/unop: `[nfuncs, nfuncs+1, nfuncs+2]` (§2.3 — the CANONICAL
    /// formula, unchanged since A-1). These are also the 0th/1st/2nd FUNCTION IMPORT indices
    /// (`otter_helpers.otter_binop` etc. are the first 3 function imports — §2.2/§2.3 construction
    /// order; since A-3 they are the ONLY fixed-prefix imports, `otter.memory` aside). Also the
    /// §5.2 record==live gate's `H*` injection values for binop/convert/unop.
    pub helper_slots: [u32; 3],
    /// The k-th distinct host fn's table slot, in **first-resolution order** (§2.3 CANONICAL
    /// formula `nfuncs + 3 + k`, restored at A-3 now that the A-2-provisional trampoline slots —
    /// which had shifted this range 2 slots later, to `nfuncs + 5 + k` — are deleted outright, not
    /// merely relocated: `resolve_host`'s first-call order = import order = slot order; the §4.1
    /// resolver-determinism rule — called exactly once per distinct pointer — is what makes this
    /// list reproducible). The §5.2 `R*` injection's canonical channel.
    pub host_slots: Vec<u32>,
    /// Every import of the artifact, in import-section order: `otter.memory`, the 3
    /// `otter_helpers.otter_{binop,convert,unop}` helpers, then one `otter_host.<name>` per
    /// distinct resolved host fn (first-resolution order) — the complete, FINAL b′
    /// export-requirement spec (§1.2). The A-2-provisional `otter_helpers.{dispatch,tail}`
    /// trampoline imports this list used to carry are gone outright since A-3 (§3.1/§3.2 —
    /// `Call`/`TailCall` no longer need a dispatch-helper indirection at all).
    pub imports: Vec<AotImport>,
    /// `func_idx → (addr, words)` — the §2.5 self-assignment map (ascending, disjoint spans).
    pub scratch: Vec<AotScratchEntry>,
    /// `[scratch_base, end)` — the region the linker/embedder MUST reserve (§2.5-i) and cover
    /// (§2.5-ii). `end` was `u64` from the start (O-14, recorded deviation from the design
    /// sketch's `(u32, u32)`): a region legally ending exactly AT `2^32` (the §4.6 boundary is
    /// `>`, so `base + span == 2^32` is accepted) is not representable in a `u32`, and
    /// truncating it would under-state the reservation duty by one byte — the silent-corruption
    /// class §2.5 exists to prevent. `base` widened to `u64` at O-19 C1 (§2.2 #9), making the
    /// pair uniformly u64; the exclusive-u64 `end` REPRESENTATION stays sufficient for A64 too
    /// [O-19 §2.4 결정]: the A64 span bound is capped at `2^63`, so the "end == 2^64" edge that
    /// would overflow an exclusive u64 end is refused before it can be recorded.
    pub scratch_region: (u64, u64),
    /// `nfuncs + 3 + host_slots.len()` — the §2.3 CANONICAL layout `T` (unchanged formula/meaning
    /// since A-1). Since A-3 this is ALSO the artifact's ACTUAL declared `(table T T funcref)`
    /// size: A-2's `+2` provisional-trampoline oversizing is gone (those 2 slots no longer exist
    /// in the table at all), so `table_size` finally means exactly what its name says on every
    /// axis at once — no recovery arithmetic needed against a since-deleted field.
    pub table_size: u32,
    /// Mirror of [`AotOpts::shared_memory`] (the emitted memory-import shape).
    pub shared_memory: bool,
    /// Mirror of [`AotOpts::memory64`] (O-19): whether the artifact was emitted in the A64
    /// mixed (memory64 memory + 32-bit table) shape. Carried so downstream layers can
    /// cross-check the flag against the BYTES' own memory64 bit (§2.5 — the record==live
    /// invariance check's manifest half, C5) instead of trusting either side alone.
    pub memory64: bool,
    /// `env!("CARGO_PKG_VERSION")` at emit time — the minimal anchor of the artifact↔engine-library
    /// generation check (the full build-hash fail-closed rule is deferred to O-16c′ with the rest
    /// of the packaging decisions — §4.1).
    pub emitter_version: &'static str,
}

/// [`compile_module`]'s product: the module bytes plus the manifest that, together, are the
/// **entire input** of the O-16c′ link step (design §1.2).
///
/// Deliberately NOT `#[non_exhaustive]` (A-5 disposition, unlike [`AotManifest`] whose field set
/// is expected to grow): this pair IS the total product — anything new belongs inside the
/// manifest — and the §8.3 mutation harness must construct doctored artifacts (`AotArtifact {
/// bytes: mutated, manifest }`) from downstream to prove the gates bite.
#[derive(Clone, Debug)]
pub struct AotArtifact {
    /// The core wasm module (A-1 sections: type, import, function, export, code — §2.1's AOT
    /// whitelist additions table/element only appear from A-2).
    pub bytes: Vec<u8>,
    /// The machine-readable contract data — see [`AotManifest`].
    pub manifest: AotManifest,
}

/// Why [`compile_module`] failed — **whole-module** (design §4.2, owner-ratified §5b C-②):
/// any per-function [`Refusal`] is promoted value-intact into [`AotError::Func`] and fails the
/// entire module. AOT has no T0 fallback slot, so the JIT's sticky-`Refused` degradation does not
/// exist here; the only honest reaction to an unloadable function is a loud, attributed error.
/// Never a panic (the [`Refusal`] no-panic contract, inherited), and an `Err` never leaves a
/// half-built artifact behind (bytes are only assembled after every check passes).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AotError {
    /// Input-contract violation, checked before anything else (§4.2): `scratch_base % 8 != 0`
    /// (the §2.5 8-alignment MUST, machine-enforced).
    ScratchBaseMisaligned {
        /// The misaligned [`AotOpts::scratch_base`] (`u64` since O-19 C1, with the field it echoes).
        base: u64,
    },
    /// Input-contract violation (O-19 §2.1 [결정]): [`AotOpts::shared_memory`] ×
    /// [`AotOpts::memory64`] is refused **fail-closed in v1** — the universal shared-import max
    /// ([`SHARED_MEMORY_IMPORT_MAX_PAGES`]) is a wasm32-only figure, and no measurement of the
    /// combined shared+memory64 shape exists yet (S2-M2 #11). Checked at the same input stage as
    /// [`AotError::ScratchBaseMisaligned`] (alignment first, then this). Unblock: a dedicated
    /// spike measuring the shared×A64 import-matching shape.
    SharedMemory64Unsupported,
    /// Function `func_idx` was refused — the existing [`Refusal`] value, unchanged (§4.3: no
    /// second enumeration — a new `Refusal` variant propagates to this surface automatically).
    /// Deterministic order: the FIRST failure in func_idx-ascending, single-pass order (§4.2 —
    /// a size-pre-scan two-pass implementation is forbidden precisely because it would reorder
    /// failures). `ScratchOutOfAddressSpace` (base + Σspan > 2^32) surfaces here at the first
    /// violating function — there is no separate module-level variant (§2.5, panel-corrected).
    Func {
        /// Index of the refused function in `m.funcs`.
        func_idx: usize,
        /// The per-function refusal, value-intact.
        refusal: Refusal,
    },
    /// Module-level (checked after every function passed): `m.entry` names no function.
    EntryOutOfRange {
        /// The out-of-range `m.entry`.
        entry: usize,
        /// `m.funcs.len()`.
        nfuncs: usize,
    },
    /// Module-level, detected at the dup's own resolution site during the single pass (§4.2):
    /// two DIFFERENT host-fn pointers resolved to the SAME import name — the A-2 import section
    /// (one import per name) could not represent both, so the ambiguity is refused now, not
    /// discovered at link time.
    DuplicateHostImportName {
        /// The colliding name.
        name: HostImportName,
        /// Function whose walk first resolved the name.
        first_func_idx: usize,
        /// Function whose walk resolved a different pointer to the same name.
        dup_func_idx: usize,
    },
    /// Anti-wrap totality guard, the module-level analogue of [`Refusal::TooManyLeaders`] (an
    /// ADDITION to the design §4.2 sketch, recorded as such): the §2.3 canonical slot layout
    /// `[funcs][3 helpers][hosts]` must fit the u32 slot space that `call_indirect`/
    /// `return_call_indirect` immediates and the manifest carry — `nfuncs + 3 + nhosts >
    /// u32::MAX` (the `3` = the 3 fixed helper slots; A-2's extra `+2` for the now-retired
    /// provisional dispatch/tail trampolines no longer applies since A-3) cannot be encoded
    /// without a silently wrapping cast (a miscompile, the worst class). Reaching it needs ~2^32
    /// functions or distinct host pointers, so it is untestable in practice; the variant exists to
    /// keep "`Ok` ⇒ well-formed artifact" total rather than probabilistic.
    TooManyTableSlots {
        /// `m.funcs.len()` at the point of overflow.
        nfuncs: usize,
        /// Distinct host fns resolved so far (0 when the fixed helper/trampoline slots themselves
        /// overflow).
        nhosts: usize,
    },
}

// --- A-2 host import-type derivation (§2.2/§2.3) ---------------------------------------------------

/// Wasm argument-count family of a `Host1`/`Host2`/`HostCtx1`/`HostCtx2` call — determines which
/// canonical helper TYPE (`HOST1_TY`/`HOST2_TY`/`HOSTCTX2_TY`) an A-2 host import must declare so
/// it matches what every `call_indirect` site through its slot expects (§2.2/§2.3). A **pure
/// bookkeeping walk for import-type derivation**, kept independent of `lower_expr`/`precheck_expr`
/// on purpose (§5.1: this is not a second semantics pass — it never lowers or validates anything,
/// only classifies an already-resolved pointer for the import section).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostShape {
    /// `Host1` — `(i64) -> i64` ([`HOST1_TY`]).
    One,
    /// `Host2` OR `HostCtx1` — BOTH lower to `(i64, i64) -> i64` ([`HOST2_TY`]): Rust's
    /// `HostFn2`/`HostFnCtx1` typedefs share this exact 2-argument signature, so a single fn
    /// pointer's WASM TYPE is unambiguous even though which `Expr` variant resolved it first is
    /// not (see [`AotImportKind::Host`]'s doc — the `arity`/`ctx` label is a best-effort
    /// convenience for exactly this bucket, not a second source of truth).
    Two,
    /// `HostCtx2` — `(i64, i64, i64) -> i64` ([`HOSTCTX2_TY`]).
    Three,
}

impl HostShape {
    /// The wasm function type an import of this shape must declare.
    const fn wasm_ty(self) -> (&'static [ValType], &'static [ValType]) {
        match self {
            HostShape::One => HOST1_TY,
            HostShape::Two => HOST2_TY,
            HostShape::Three => HOSTCTX2_TY,
        }
    }

    /// The best-effort `(arity, ctx)` DESCRIPTIVE label for [`AotImportKind::Host`] — canonical
    /// for `One`/`Three` (only one `Expr` shape maps to each), a convention for `Two` (see the
    /// variant's own doc: `Host2`'s reading is picked, since it needs no baked `host_ctx` slot to
    /// explain).
    const fn arity_ctx(self) -> (u8, bool) {
        match self {
            HostShape::One => (1, false),
            HostShape::Two => (2, false),
            HostShape::Three => (2, true),
        }
    }
}

/// Find the [`HostShape`] of the FIRST `Host1`/`Host2`/`HostCtx1`/`HostCtx2` node naming fn
/// pointer `ptr`, anywhere in `f`'s code (walking `isa.op(inst.op).effects`' `Expr` trees exactly
/// where [`precheck`]/[`precheck_expr`] do — the Expr trees live on the OPCODE, not on `Inst`).
/// Used ONLY to pick an A-2 host import's wasm type (§2.2) — called exclusively from inside
/// `compile_module`'s resolver closure at the FIRST resolution of `ptr`, which only happens
/// because some node in the function currently being processed just supplied that exact pointer
/// to the resolver, so the walk is structurally guaranteed to find it; the fallback is defensive
/// bookkeeping only (never a real path), kept per this crate's no-panic contract instead of
/// `unwrap`/`expect`.
fn host_shape_in(isa: &Isa, f: &FuncDef, ptr: usize) -> HostShape {
    fn walk(e: &Expr, ptr: usize) -> Option<HostShape> {
        match e {
            Expr::Imm(_) | Expr::ImmF64(_) | Expr::OperandImm(_) | Expr::LocalAt(_) => None,
            Expr::Bin(_, a, b) => walk(a, ptr).or_else(|| walk(b, ptr)),
            Expr::Un(_, a) => walk(a, ptr),
            Expr::Host1(g, a) => {
                if *g as usize == ptr {
                    Some(HostShape::One)
                } else {
                    walk(a, ptr)
                }
            }
            Expr::Host2(g, a, b) => {
                if *g as usize == ptr {
                    Some(HostShape::Two)
                } else {
                    walk(a, ptr).or_else(|| walk(b, ptr))
                }
            }
            Expr::HostCtx1(g, a) => {
                if *g as usize == ptr {
                    Some(HostShape::Two)
                } else {
                    walk(a, ptr)
                }
            }
            Expr::HostCtx2(g, a, b) => {
                if *g as usize == ptr {
                    Some(HostShape::Three)
                } else {
                    walk(a, ptr).or_else(|| walk(b, ptr))
                }
            }
        }
    }
    // Exhaustive on Effect, no wildcard (this crate's completeness convention) — mirrors
    // `precheck`'s reach (SetLocalAt/Guard/Halt/Return/JumpIf's Expr fields).
    for inst in &f.code {
        for eff in &isa.op(inst.op).effects {
            let found = match eff {
                Effect::SetLocalAt { value, .. } => walk(value, ptr),
                Effect::Guard { cond } => walk(cond, ptr),
                Effect::Halt { value } | Effect::Return { value } => walk(value, ptr),
                Effect::JumpIf { cond, .. } => walk(cond, ptr),
                Effect::JumpTo { .. } | Effect::Call { .. } | Effect::TailCall { .. } => None,
            };
            if let Some(shape) = found {
                return shape;
            }
        }
    }
    // Defensive-only fallback (fn doc) — `Two` is as good a guess as any of the three.
    HostShape::Two
}

/// Whole-module build-time emission (O-14 design §4.1) — the wasm twin of
/// `otter-cranelift::compile_module(isa, m)` (same `(isa, module)` input shape; mode ≠ tier,
/// MODE §3). Lowers EVERY function of `m` into ONE standalone wasm module and returns its bytes
/// plus the [`AotManifest`] describing them, or the first failure as a whole-module [`AotError`]
/// (§4.2 totalization — no partial artifact, ever).
///
/// # A-3 shape (design §9 A-3 — call-lowering inversion; the FINAL-CONTRACT AOT shape)
///
/// * **N functions, one module, module-scope type interning** (§2.1, unchanged since A-1): bodies
///   are emitted by the SAME single body emitter the JIT surface uses (§5.1), against one shared
///   [`Types`] — which re-arranges `call_indirect` type-index immediates relative to the
///   per-function modules (the §5.2 T-normalization's defined divergence).
/// * **Call lowering INVERTED since A-3** ([`CallMode::Table`], design §3.1/§3.2): `Bin`/`Un`
///   sites still `call_indirect` the §2.3 canonical helper slots (`helper_slots`, UNCHANGED) and
///   host sites their `host_slots` (UNCHANGED), but `Call`/`TailCall` sites now guard
///   (`i64.ge_u` against [`AotManifest::nfuncs`]) then `call_indirect`/`return_call_indirect`
///   the callee word DIRECTLY — no dispatch/tail helper indirection at all. The A-2 provisional
///   `otter_helpers.{dispatch,tail}` trampoline imports/slots this increment retires WHOLE (they
///   no longer appear anywhere in the artifact, the manifest, or the table size) — the engine
///   trampoline loop they fed is not needed by this artifact's `Call`/`TailCall` sites either;
///   wasm's own tail-call mechanism keeps `TailCall` chains native-stack-bounded instead
///   ([스파이크] M2).
/// * **Internal link** (§2.3, unchanged structure since A-2): a module-**declared** funcref table
///   (`(table T T funcref)`, min=max, `T = nfuncs + 3 + nhosts` — the §2.3 CANONICAL formula,
///   restored verbatim now that the A-2-provisional +2 trampoline slots are gone) plus ONE active
///   element segment (MVP encoding, offset 0, every slot populated). Table layout,
///   func-index-ascending: own functions `[0, nfuncs)`, then binop/convert/unop (`helper_slots`
///   at `[nfuncs, nfuncs+3)`), then one slot per distinct resolved host fn (`host_slots` at
///   `[nfuncs+3, T)` — the canonical §2.3 range, no longer shifted by 2).
/// * **Function imports** (§2.2, provisional trampolines removed): `otter.memory` (unchanged),
///   `otter_helpers.otter_{binop,convert,unop}`, and `otter_host.<name>` per distinct resolved
///   host fn — `manifest.imports` carries the complete list (the b′ export-requirement spec, now
///   with nothing provisional left in it).
/// * **Scratch self-assignment** (§2.5, unchanged since A-1): func-ascending, disjoint
///   `8 * scratch_words(f)` runs from [`AotOpts::scratch_base`]; the per-function `scratch_addr`
///   PARAMETER does not exist on this surface — the manifest carries the map instead, and
///   re-basing is a re-emission (§2.5-iv: emission is deterministic, so the linker re-invokes
///   with a new base; no runtime relocation).
/// * **Exports** (§2.4, unchanged since A-2): `otter_entry` (the entry function — its wasm
///   FUNCTION INDEX is `nimports_func + m.entry`, since function imports precede every
///   module-defined function in the function index space; `manifest.entry_slot` stays the
///   slot-space value) and `otter_table` (table index 0 — the b″ link/compose/individual-call
///   surface, §2.4). `ctx` stays the opaque dead parameter: callers pass 0 (the harness may pass
///   junk — emitted code forwards it at most).
/// * **Pre-link AOT whitelist + defensive element-target check**: [`validate_aot_artifact`] (§2.1)
///   enforces the AOT-widened section whitelist and rejects an active element segment that
///   targets an IMPORTED table ([스파이크] M5's demonstrated hazard) — `compile_module`'s own
///   output cannot violate this by construction (it never imports a table since A-2), so the
///   checker exists to defend arbitrary purported pre-link artifacts, this one included.
///
/// # Determinism & failure order (§4.2, normative)
///
/// Single pass, func_idx ascending, each function running the full acceptance predicate
/// (`check_func` → §4.6 precheck, host resolution included → scratch bound check) with the first
/// failure returned immediately as `AotError::Func { func_idx, refusal }`. Before the pass:
/// `ScratchBaseMisaligned`, then the [`AotError::TooManyTableSlots`] helper-slot guard (both
/// input-shape checks). After the pass: `EntryOutOfRange`. `DuplicateHostImportName` (and the
/// host-slot half of `TooManyTableSlots`) fire AT their resolution site inside the pass.
/// `resolve_host` is called **exactly once per distinct pointer**, at its first use in walk
/// order, and cached (§4.1 resolver-determinism norm) — an impure resolver still yields a
/// deterministic import list, slot assignment and manifest. This single-pass shape is why `Call`/
/// `TailCall` sites bake their [`CallMode::Table`] guard boundary as `nfuncs` (known BEFORE the
/// pass starts — `m.funcs.len()`, not the host-dependent `table_size`) rather than the final
/// table size: `nfuncs` is exactly [`AotManifest::nfuncs`], the one guard-boundary quantity every
/// `Call`/`TailCall` site needs and the only one available while function bodies are still being
/// emitted (the same single-pass constraint that forced A-2's now-retired provisional trampoline
/// placement no longer applies to anything, since A-3 has nothing left that needs the FINAL host
/// count to bake a call site).
///
/// # No-panic
///
/// Same contract as [`compile_dispatch_fn`] (well-formed [`Isa`] trust boundary included).
pub fn compile_module(
    isa: &Isa,
    m: &otter::Module,
    resolve_host: &dyn Fn(usize) -> Option<HostImportName>,
    opts: &AotOpts,
) -> Result<AotArtifact, AotError> {
    // §4.2: input-contract checks first — 8-alignment (the §2.5 MUST), then the fixed-slot
    // anti-wrap guard. `+ 3`, the §2.3 CANONICAL fixed prefix (the 3 helpers only, since A-3
    // retired the 2 A-2-provisional trampoline slots outright — fn doc's "A-3 shape"): the
    // baseline that must fit u32 before even one host is resolved.
    if !opts.scratch_base.is_multiple_of(8) {
        return Err(AotError::ScratchBaseMisaligned {
            base: opts.scratch_base,
        });
    }
    // O-19 §2.1 [결정]: shared × memory64 is refused fail-closed in v1, at the same input stage
    // (alignment first — its "checked before anything else" doc stays true, this is second).
    // `SHARED_MEMORY_IMPORT_MAX_PAGES` is a wasm32-only figure and the combined shape has no
    // measurement behind it yet (S2-M2 #11).
    if opts.shared_memory && opts.memory64 {
        return Err(AotError::SharedMemory64Unsupported);
    }
    let nfuncs = m.funcs.len();
    if nfuncs as u64 + 3 > u64::from(u32::MAX) {
        return Err(AotError::TooManyTableSlots { nfuncs, nhosts: 0 });
    }
    let helper_slots = [nfuncs as u32, nfuncs as u32 + 1, nfuncs as u32 + 2];
    // A-3 (§3.1/§3.2): `Call`/`TailCall` sites no longer `call_indirect` a dispatch/tail HELPER
    // slot at all — they guard against `nfuncs` and `call_indirect`/`return_call_indirect` the
    // callee word directly (`CallMode::Table`, below). `dispatch`/`tail` are therefore inert
    // placeholders here: the JIT-surface `HelperIdx` shape stays frozen (§4.3), so a full struct
    // is still required to share `emit_function_body`, but `CallMode::Table` never reads these
    // two fields — see `HelperIdx`'s own doc.
    let canonical_helpers = HelperIdx {
        dispatch: 0,
        tail: 0,
        binop: helper_slots[0],
        convert: helper_slots[1],
        unop: helper_slots[2],
    };
    // §3.1/§3.2's guard boundary: `nfuncs` is known immediately (`m.funcs.len()`), unlike the
    // A-2-retired trampoline slots which needed the FINAL host count — see the fn doc's
    // "Determinism & failure order" section.
    let call_mode = CallMode::Table {
        nfuncs: nfuncs as u32,
    };

    /// One resolved host fn: pointer → (name, slot, wasm-type shape), plus where it was first
    /// seen (for the [`AotError::DuplicateHostImportName`] attribution).
    struct HostBinding {
        ptr: usize,
        name: String,
        slot: u32,
        first_func_idx: usize,
        /// A-2 addition: which `HOST*_TY` this pointer's import must declare (§2.2/§2.3) —
        /// derived once, at first resolution, from the `Expr` node that supplied `ptr`.
        shape: HostShape,
    }
    let hosts: RefCell<Vec<HostBinding>> = RefCell::new(Vec::new());
    // Module-level failures detected INSIDE the resolver closure (dup name / slot-space overflow)
    // can only escape the body emitter as a `HostFnUnresolved` refusal (the closure's channel is
    // `Option<u32>`); this side-channel carries the real error past that, and the loop below
    // checks it FIRST on any refusal.
    let module_err: RefCell<Option<AotError>> = RefCell::new(None);
    let cur_func = Cell::new(0usize);
    let wrapped_resolve = |ptr: usize| -> Option<u32> {
        // Cache hit: the §4.1 exactly-once rule — the user resolver is never re-consulted for a
        // pointer it already answered (impure resolvers stay deterministic this way).
        let cached = hosts.borrow().iter().find(|b| b.ptr == ptr).map(|b| b.slot);
        if let Some(slot) = cached {
            return Some(slot);
        }
        let name = resolve_host(ptr)?;
        let mut hs = hosts.borrow_mut();
        if let Some(prev) = hs.iter().find(|b| b.name == name.0) {
            *module_err.borrow_mut() = Some(AotError::DuplicateHostImportName {
                name,
                first_func_idx: prev.first_func_idx,
                dup_func_idx: cur_func.get(),
            });
            return None;
        }
        // §2.3 CANONICAL formula (restored since A-3 — the A-2-provisional trampoline shift is
        // gone): k-th distinct host fn ↦ slot nfuncs + 3 + k. `slot + 1 <= u32::MAX` keeps
        // `table_size` (now also the ACTUAL declared table size — no more A-2 +2) representable.
        let slot_wide = nfuncs as u64 + 3 + hs.len() as u64;
        if slot_wide + 1 > u64::from(u32::MAX) {
            *module_err.borrow_mut() = Some(AotError::TooManyTableSlots {
                nfuncs,
                nhosts: hs.len() + 1,
            });
            return None;
        }
        let slot = slot_wide as u32;
        // §2.2/§2.3: this pointer's import TYPE, derived from whichever `Expr` node in the
        // CURRENT function (`cur_func` — resolution only ever happens while that function's
        // `precheck`/lowering walk is running) supplied it (`host_shape_in`'s doc).
        let shape = host_shape_in(isa, &m.funcs[cur_func.get()], ptr);
        hs.push(HostBinding {
            ptr,
            name: name.0,
            slot,
            first_func_idx: cur_func.get(),
            shape,
        });
        Some(slot)
    };

    // Module-scope interning (§2.1): ONE `Types` for all N bodies, carrying the module's
    // addrtype (O-19 — [`AotOpts::memory64`] is the addrtype parameter's only public exposure).
    // The compiled type first, as on the per-function surface — type index 0 is the functions'
    // own type in both module shapes (and in both addrtypes).
    let addrtype = if opts.memory64 {
        AddrType::A64
    } else {
        AddrType::A32
    };
    let mut types = Types::new(addrtype);
    let compiled_ty = types.compiled();
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    let mut scratch: Vec<AotScratchEntry> = Vec::with_capacity(nfuncs);
    // The §2.5 assignment cursor — u64 end-to-end since O-19 C1 (it always was u64 here: a run
    // may legally END exactly at 2^32, the §4.6 boundary being `>`; C1 merely made the base and
    // the recorded entries share the width instead of narrowing).
    let mut cursor: u64 = opts.scratch_base;

    // §4.2 single pass: func_idx ascending, full acceptance predicate per function, first failure
    // wins. Disjoint sequential assignment needs no pre-scan (f's base = the running cursor), and
    // a pre-scan is FORBIDDEN — it would reorder failures across functions. Since A-3, `call_mode`
    // (`CallMode::Table { nfuncs }`) needs nothing from this pass — `nfuncs` is `m.funcs.len()`,
    // fixed before the pass even starts (fn doc's "Determinism & failure order").
    for (func_idx, f) in m.funcs.iter().enumerate() {
        cur_func.set(func_idx);
        // The cursor IS the address since O-19 C1 (u64 end-to-end — §2.2 #6/#9/#10): the old
        // u32-saturating narrow for the "cursor == 2^32 exactly" edge is gone. A words>0
        // function past the addrtype bound still refuses via the emitter's own span check —
        // now with the EXACT address as the diagnostic — and a words==0 function bakes the
        // value only as a dead argc==0 `args` operand, where any value is harmless
        // (`AotScratchEntry::addr`'s doc).
        let addr = cursor;
        match emit_function_body(
            isa,
            f,
            m.fill,
            m.host_ctx,
            canonical_helpers,
            &wrapped_resolve,
            addr,
            &mut types,
            call_mode,
        ) {
            Ok(body) => {
                functions.function(compiled_ty);
                code.function(&body);
            }
            Err(refusal) => {
                if let Some(e) = module_err.borrow_mut().take() {
                    return Err(e); // the resolver side-channel's real error (fn doc)
                }
                return Err(AotError::Func { func_idx, refusal });
            }
        }
        // check_func passed inside the emitter, so this walk is panic-free; same single source
        // (`site_argc` via `max_scratch_words`) as the stores the body just baked (§2.5-1).
        let words = max_scratch_words(isa, f);
        scratch.push(AotScratchEntry { addr, words });
        cursor += 8 * words as u64;
    }
    // Defensive totality: a side-channel error can only be observed through a refusal above, but
    // draining it here keeps "Ok ⇒ no module-level failure recorded" unconditional.
    if let Some(e) = module_err.into_inner() {
        return Err(e);
    }

    // §4.2: module-level checks AFTER every function passed.
    if m.entry >= nfuncs {
        return Err(AotError::EntryOutOfRange {
            entry: m.entry,
            nfuncs,
        });
    }

    // --- module assembly (A-3 sections: type, import, function, table, export, element, code —
    // §2.1 canonical order; fn doc) ---
    let host_bindings = hosts.into_inner();
    let nhosts = host_bindings.len();
    // In-loop slot checks guaranteed `nfuncs + 3 + nhosts <= u32::MAX`; the fallback can't fire
    // (kept over `as`/`expect` for the no-panic contract and cast-lint hygiene, matching this
    // function's existing style). `table_size` is the §2.3 CANONICAL `T` — since A-3 this is ALSO
    // the artifact's ACTUAL declared table size (the A-2 `+2` provisional-trampoline oversizing is
    // gone, `AotManifest::table_size`'s doc).
    let table_size = u32::try_from(nfuncs as u64 + 3 + nhosts as u64).unwrap_or(u32::MAX);
    // Own functions start right after every FUNCTION import: 3 helpers + nhosts hosts (§2.2/§2.3
    // — NOT `table_size`, which additionally counts the `nfuncs` OWN functions themselves; the
    // function-index-space count is `table_size - nfuncs`, computed directly here instead to avoid
    // a subtraction. Guaranteed to fit u32 by the same in-loop checks as `table_size`.
    let nimports_func = u32::try_from(3u64 + nhosts as u64).unwrap_or(u32::MAX);

    // Import section: memory (unchanged shape, §2.2), then function imports in slot-ascending
    // order — binop/convert/unop (func indices 0-2), then one `otter_host.<name>` per resolved
    // host (3..3+nhosts) — so import func index `i` and table slot `nfuncs + i` name the SAME
    // entity uniformly for every non-own-function slot (the §2.3 canonical identity, restored).
    let binop_ty = types.index_of(BINOP_TY.0, BINOP_TY.1);
    let unary_ty = types.index_of(UNARY_HELPER_TY.0, UNARY_HELPER_TY.1); // shared: convert + unop
    let mut imports = ImportSection::new();
    push_memory_import(&mut imports, opts.shared_memory, addrtype);
    imports.import("otter_helpers", "otter_binop", EntityType::Function(binop_ty));
    imports.import("otter_helpers", "otter_convert", EntityType::Function(unary_ty));
    imports.import("otter_helpers", "otter_unop", EntityType::Function(unary_ty));
    let mut manifest_imports = Vec::with_capacity(4 + nhosts);
    manifest_imports.push(AotImport {
        module: String::from("otter"),
        name: String::from("memory"),
        kind: AotImportKind::Memory,
    });
    manifest_imports.push(AotImport {
        module: String::from("otter_helpers"),
        name: String::from("otter_binop"),
        kind: AotImportKind::Helper(HelperKind::Binop),
    });
    manifest_imports.push(AotImport {
        module: String::from("otter_helpers"),
        name: String::from("otter_convert"),
        kind: AotImportKind::Helper(HelperKind::Convert),
    });
    manifest_imports.push(AotImport {
        module: String::from("otter_helpers"),
        name: String::from("otter_unop"),
        kind: AotImportKind::Helper(HelperKind::Unop),
    });
    for b in &host_bindings {
        let (ty0, ty1) = b.shape.wasm_ty();
        let host_ty = types.index_of(ty0, ty1);
        imports.import("otter_host", b.name.as_str(), EntityType::Function(host_ty));
        let (arity, ctx) = b.shape.arity_ctx();
        manifest_imports.push(AotImport {
            module: String::from("otter_host"),
            name: b.name.clone(),
            kind: AotImportKind::Host { arity, ctx },
        });
    }

    // Table (§2.3): declared, min=max=`table_size` — no more `otter.table` import, and since A-3
    // no more +2 provisional oversizing either (the dispatch/tail slots are gone outright).
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: u64::from(table_size),
        maximum: Some(u64::from(table_size)),
        shared: false,
    });

    // Element (§2.3): ONE active segment, MVP encoding (table 0 implicit, offset 0), every slot
    // populated — func-index-ascending exactly as the table-slot layout (fn doc's "A-3 shape").
    let mut elems: Vec<u32> = Vec::with_capacity(table_size as usize);
    for j in 0..nfuncs as u32 {
        elems.push(nimports_func + j); // own functions: identity mapping, slot j ↦ func nimports+j
    }
    elems.push(0); // binop
    elems.push(1); // convert
    elems.push(2); // unop
    for k in 0..nhosts as u32 {
        elems.push(3 + k);
    }
    let mut element_section = ElementSection::new();
    let zero_offset = ConstExpr::i32_const(0);
    element_section.active(None, &zero_offset, Elements::Functions(Cow::Owned(elems)));

    // Export (§2.4): `otter_entry` (its wasm FUNCTION INDEX is `nimports_func + m.entry` — function
    // imports precede every module-defined function in the function index space) + `otter_table`
    // (table index 0, §2.4).
    let mut exports = ExportSection::new();
    exports.export("otter_entry", ExportKind::Func, nimports_func + m.entry as u32);
    exports.export("otter_table", ExportKind::Table, 0);

    let mut module = Module::new();
    module.section(&types.section);
    module.section(&imports);
    module.section(&functions);
    module.section(&tables);
    module.section(&exports);
    module.section(&element_section);
    module.section(&code);

    let manifest = AotManifest {
        entry_slot: m.entry as u32,
        nfuncs: nfuncs as u32,
        helper_slots,
        host_slots: host_bindings.iter().map(|b| b.slot).collect(),
        imports: manifest_imports,
        scratch,
        scratch_region: (opts.scratch_base, cursor),
        table_size,
        shared_memory: opts.shared_memory,
        memory64: opts.memory64,
        emitter_version: env!("CARGO_PKG_VERSION"),
    };
    Ok(AotArtifact {
        bytes: module.finish(),
        manifest,
    })
}

// ============================ O-14 A-2: pre-link artifact validator ================================
// Design §2.1 — the AOT-widened whitelist (adds table/element/data/datacount to O-13's JIT-only
// set) PLUS the §2.3 defensive check the [스파이크] M5 probe demanded: an active element segment
// targeting an IMPORTED table is legal core wasm, but the M5 probe measured it silently
// overwriting host table slot 0 on instantiation — a contract violation the wasm spec itself does
// not forbid, so it must be enforced here.

/// Why [`validate_aot_artifact`] rejected `bytes` — never a panic (the function fully owns its own
/// parse errors, wrapping every `wasmparser` failure into [`AotValidationError::Malformed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AotValidationError {
    /// A section outside the §2.1 AOT pre-link whitelist
    /// `{custom, type, import, function, table, export, element, code, data, datacount}` — the
    /// section id is the raw wasm binary section byte (`start` = 8, `memory` = 5, `global` = 6,
    /// `tag` = 13, or an outright unknown id).
    DisallowedSection {
        /// The offending section's binary id.
        id: u8,
    },
    /// An active element segment's target table index resolves to an IMPORTED table, not a
    /// module-DECLARED one (§2.3's defensive check — [스파이크] M5's demonstrated hazard: legal
    /// core wasm, but a silent host-table-slot overwrite on instantiation). `compile_module`'s own
    /// A-2 output can never trigger this (it imports no table at all, by construction — §2.2/§2.3)
    /// — this variant exists to reject arbitrary purported pre-link artifacts, `compile_module`'s
    /// own output mutated for [`#[cfg(test)]`] adversarial coverage included (§8.3 mutation ⑥).
    ElementTargetsImportedTable {
        /// The active segment's resolved table index (MVP encoding's implicit `None` reads as 0).
        table_index: u32,
    },
    /// `bytes` did not parse as a well-formed wasm binary at all (truncated, corrupt section
    /// framing, etc.) — a parse failure is itself a whitelist/shape violation, not a distinct
    /// success path this validator needs to report more precisely than "reject".
    Malformed,
}

/// Enforce the O-14 §2.1 AOT pre-link section whitelist and the §2.3 element-target defense on an
/// arbitrary purported pre-link AOT artifact's raw `bytes`.
///
/// **Pre-link ONLY** (§2.1 scope note): after O-16c′ M2a linking, a real product necessarily gains
/// entities this whitelist forbids (a `global` shadow-stack pointer, an owned `memory`) — this
/// validator is not meant to run on link OUTPUT, only on `compile_module`'s pre-link artifact (or
/// anything claiming to be one).
///
/// `compile_module`'s own product satisfies this **by construction** (§2.2/§2.3: it never imports
/// a table since A-2, so the element-target check is automatically vacuous for it) — this function
/// is the DEFENSIVE, standalone form (§2.1: "검증기는 방어적으로 검사한다"), exercised directly by
/// the §8.3 mutation ⑥ adversarial test rather than wired into `compile_module` itself (there is
/// nothing for it to catch there).
///
/// Reads with `wasmparser`'s raw section/payload parser only (`Parser`/`Payload`, unconditionally
/// available — no `validate`/`std`/`component-model` feature needed, keeping this crate no_std-
/// clean): this is a WHITELIST + one targeted structural check, not full wasm validation (the
/// differential test harnesses run `wasmparser::Validator` separately, as a dev-dependency, for
/// that).
pub fn validate_aot_artifact(bytes: &[u8]) -> Result<(), AotValidationError> {
    // Exhaustive on `Payload`, no wildcard ([[check-completeness-periodically]]): a new
    // `wasmparser` payload variant must be classified here (allow or reject) before this crate
    // compiles against a newer `wasmparser`, rather than silently falling through a wildcard arm.
    let mut n_imported_tables: u32 = 0;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|_| AotValidationError::Malformed)?;
        match payload {
            wasmparser::Payload::Version { .. } | wasmparser::Payload::End(_) => {}
            wasmparser::Payload::CustomSection(_) => {} // id 0 — always allowed, any position
            wasmparser::Payload::TypeSection(_) => {}   // id 1
            wasmparser::Payload::ImportSection(reader) => {
                // id 2 — count table imports so the element check below can tell an import-space
                // table index from a module-declared one (import section always precedes element
                // section in canonical wasm binary order, so this count is final by the time any
                // `ElementSection` payload arrives). `into_imports()` flattens the compact-import
                // encoding (wasmparser 0.258) back into individual `Import`s.
                for imp in reader.into_imports() {
                    let imp = imp.map_err(|_| AotValidationError::Malformed)?;
                    if matches!(imp.ty, wasmparser::TypeRef::Table(_)) {
                        n_imported_tables += 1;
                    }
                }
            }
            wasmparser::Payload::FunctionSection(_) => {} // id 3
            wasmparser::Payload::TableSection(_) => {}    // id 4 — module-declared tables, allowed
            wasmparser::Payload::MemorySection(_) => {
                // id 5 — the AOT contract keeps memory an IMPORT in every shape (§2.2); a
                // module-declared memory is outside the pre-link contract entirely.
                return Err(AotValidationError::DisallowedSection { id: 5 });
            }
            wasmparser::Payload::GlobalSection(_) => {
                return Err(AotValidationError::DisallowedSection { id: 6 }); // id 6 — link-time only
            }
            wasmparser::Payload::ExportSection(_) => {} // id 7
            wasmparser::Payload::StartSection { .. } => {
                // id 8 — O-13 §5.2 norm inherited: instantiation must never run guest code.
                return Err(AotValidationError::DisallowedSection { id: 8 });
            }
            wasmparser::Payload::ElementSection(reader) => {
                // id 9 — the §2.3 defensive check.
                for elem in reader {
                    let elem = elem.map_err(|_| AotValidationError::Malformed)?;
                    if let wasmparser::ElementKind::Active { table_index, .. } = elem.kind {
                        let ti = table_index.unwrap_or(0); // MVP encoding's implicit table 0
                        if ti < n_imported_tables {
                            return Err(AotValidationError::ElementTargetsImportedTable { table_index: ti });
                        }
                    }
                }
            }
            wasmparser::Payload::CodeSectionStart { .. } | wasmparser::Payload::CodeSectionEntry(_) => {} // id 10
            wasmparser::Payload::DataSection(_) => {} // id 11 — AOT-contract-level allowance, §2.1
            wasmparser::Payload::DataCountSection { .. } => {} // id 12
            wasmparser::Payload::TagSection(_) => {
                return Err(AotValidationError::DisallowedSection { id: 13 }); // id 13 — EH, out of scope
            }
            wasmparser::Payload::UnknownSection { id, .. } => {
                return Err(AotValidationError::DisallowedSection { id });
            }
            // `wasmparser::Payload` is itself `#[non_exhaustive]` upstream (component-model
            // payloads guarded by a cargo feature this crate keeps off, plus room for future
            // core-wasm proposals) — this crate's own [[check-completeness-periodically]]
            // convention is "no wildcard", but that convention covers OUR types; a wildcard
            // forced by an external non_exhaustive enum is unavoidable, so it fails CLOSED
            // (reject, not silently allow) rather than silently matching a future variant this
            // whitelist was never reviewed against.
            _ => return Err(AotValidationError::Malformed),
        }
    }
    Ok(())
}
