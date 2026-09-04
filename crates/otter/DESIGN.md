# L0 meta-VM framework — design & increment log

The meta-hybrid JIT toolchain core. Goal (decision doc §2): a **single annotated source**
`{bytecode semantics + JIT hints}` generates a multi-tier, multi-strategy VM, unifying Deegen
(meta-VM-generation) + Truffle (partial evaluation) + RPython (developer-marks-only) virtues behind
**one strategy-agnostic hint vocabulary**. Methodology: PyPy — interpreter-first, correctness-then-JIT,
the interpreter is the semantic oracle ([[runtime-dev-methodology-pypy]]).

## Architecture

| layer | crate | toolchain | role |
|-------|-------|-----------|------|
| IR + T0 interpreter + engine | `otter` | `no_std`+`alloc`, `#![forbid(unsafe_code)]` | the single source (`Expr`/`Effect`/`OpDef`/`Isa`), the reference interpreter, the hint vocabulary, the optimizer, and the multi-tier `engine` policy |
| T1 baseline (platform shim) | `otter-baseline` | `std`, x86-64, `#![allow(unsafe_code)]` | copy-and-patch stencil JIT — emits an x86-64 template per IR node, microsecond compile |
| T2 optimizing (platform shim) | `otter-cranelift` | `std`, `#![allow(unsafe_code)]` | generic lowering of any `Isa` to host machine code via Cranelift |

The split is deliberate ([[runtime-rust-toolchain-config]]): the portable, safe core has no machine-code
emission; each unsafe code-emit backend lives behind the `engine::Backend` seam, so the toolchain alone
is usable `no_std` and tiers can be added/swapped. Both backends are **driven by the same reifiable IR**
— neither has hand-written per-op semantics; only their *primitive* lowering differs (Cranelift IR vs
hand-encoded x86-64).

### Tiers & the tier-up engine (`engine`)

Execution is **adaptive multi-tier**. `engine::Backend` is the seam: `compile(isa, prog, nlocals, nargs)
-> Code` + `run(code, args)`. `engine::Engine<B>` (in the safe core — pure policy) runs a registered
function on the **T0 interpreter**, counts calls, and once it is hot (`>= threshold`) promotes it through
the backend and caches the `Code`, reusing it thereafter. The Cranelift JIT implements `Backend`
(`otter-cranelift::CraneliftBackend`); a future T1 copy-and-patch tier is just another `Backend`.

Functions take **arguments**: `interpret_with(.., args)` seeds the first locals; the compiled function
has signature `extern "C" fn(*const i64) -> i64` and loads the first `nargs` locals from the pointer.
This is what makes tier-up meaningful — one compile, many calls with different args.

### OSR & deopt (frame transfer)

OSR (interpreter→compiled, enter mid-function) and deopt (compiled→interpreter, exit mid-function) are
the two directions of one **frame-transfer** machinery, over a shared frame (`locals[] + pc`):

- `Effect::Guard { cond }` — a speculation marker. The interpreter **ignores it** (semantically a
  no-op), so the result is identical whether or not a deopt happens — which is what makes deopt sound.
- `otter-cranelift::compile_resumable(.., entry_pc)` emits a **resumable** function with ABI
  `extern "C" fn(frame: *mut i64, deopt_out: *mut i64) -> i64`: it loads the whole frame at entry, jumps
  to `entry_pc`'s block, and on a failing guard spills live locals back to `frame`, writes the pc to
  `deopt_out`, and returns. `run_resumable`/`osr_continue` resolve a deopt by resuming
  `interp::interpret_from(frame, pc)`.
- `interp::interpret_until_osr` counts loop back-edges and returns `Step::Osr(pc)` once one is hot — the
  trigger for promoting a long-running, called-once loop mid-flight.
- **Auto-wired into the engine**: `Engine::enable_osr(backend, backedge_threshold)` registers an
  `OsrBackend` (produces `Resumable` code with `run_frame(&mut frame) -> Run`). At the interpreter level
  `Engine::call` runs `interpret_until_osr`; on `Step::Osr` it `compile_resumable`s at that pc and
  continues on the same frame, resolving any deopt by resuming the interpreter — all transparent to the
  caller. (`otter-cranelift::ResumableCraneliftBackend` is the `OsrBackend`.)

### The "single source" property

An op's semantics are written ONCE as a reifiable IR and consumed by every tier:

- `Expr` (pure): `Imm`, `OperandImm(slot)`, `LocalAt(slot)`, `Bin(BinOp, …)`.
- `Effect` (side effects, run in order): `SetLocalAt{reg,value}`, `JumpTo{target}`, `JumpIf{cond,target}`,
  `Halt{value}`. Operands are referenced by **slot index** so semantics are generic over the concrete
  operands an instruction carries.
- `OpDef { name, effects, hints }`; an `Isa` is a `Vec<OpDef>` indexed by opcode.

Adding an opcode = a data entry in the `Isa`. It is interpreted AND compiled with **no new match arm in
any backend** — that is the "meta" property. The only closed, framework-level set is the primitive ALU
(`BinOp`), like a CPU's instruction set; bytecode ops *compose* primitives. (`BinOp::eval` is the
reference; the backend must lower each primitive to match it exactly.)

### Abstract machine

**Register-based, typed-operation 64-bit word machine** (the WASM/CPU model). State = a flat `[i64]`
locals array (raw 64-bit words) + pc; 3-address instructions (`NOPERANDS = 3`). The machine is
**language-agnostic**: a `Value` is just a 64-bit word, and *operations are typed* — integer ops
(`Add/Sub/Mul/Lt/Eq`) read the word as `i64`; float ops (`FAdd/FSub/FMul/FDiv/FLt/FEq`) reinterpret it
as `f64` (via `f64::{from,to}_bits`); `UnOp` covers conversions (`I64ToF64`, `F64ToI64` saturating) and
negation. Booleans are `0`/`1` words. Any source language lowers its typed scalar ops to these. Float
values share the integer slot (no boxing); the backends `bitcast`/`movq` between GPR and XMM as needed.
(Register-based — not stack — because it lowers to SSA cleanly.)

**Width-parametric primitives (B1.2-wide+):** the op set is now typed by [`NumTy`] — `i8/i16/i32/i64`,
`u8/u16/u32/u64`, `f16/f32/f64`. `BinOp` is
`{ ty: NumTy, op: ArithOp }`; `UnOp` is `Neg(NumTy)` or `Convert { from, to }`. The slot is normalized
per type: signed ints sign-extended, unsigned ints zero-extended, floats as low-bit IEEE patterns.
Narrow integer arithmetic computes in 64-bit then re-normalizes to width; signed/unsigned compares pick
the right condition; float→int conversion saturates per width (matching Rust `as`). See
[[otter-primitive-types]] for the type-set rationale and the **bytecode-compat invariant** for `f16`.

### Strategy-agnostic hints (`OpHints`)

Starts minimal — `pure` (foldable when inputs are constant) and `terminator` (ends a basic block). ONE
set drives all consumers: the JIT derives basic-block leaders from `terminator` + the control `Effect`s
(not from specific opcodes), and the optimizer (`opt::constant_fold`) uses `pure` to decide what is
foldable. The vocabulary grows but stays neutral across interpreter / baseline / optimizing strategies.

### Optimizer (`opt`) — the third consumer of the single source

`opt::constant_fold` is generic constant propagation + folding driven by the hints + IR, with nothing
hardcoded to a specific opcode:
- a `pure` op with a single `SetLocalAt` whose value evaluates to a constant (block-locally) is folded
  at compile time with `BinOp::eval` — the **same wrapping arithmetic** the interpreter and JIT use, so
  the folded constant is bit-identical to the runtime result;
- **algebraic simplification** driven by the primitive hint vocabulary (`BinOp::absorbing_element`,
  `BinOp::right_identity`, `BinOp::is_commutative`): `x*0 → 0` (absorbing — holds even when `x` is
  unknown, so it's strictly stronger than const-fold) and `x+0 / x*1 / x-0 → x` (identity, rewritten to
  a copy);
- the "set register := immediate" (materializer) and "set register := register" (copy) ops are
  **discovered from the ISA** by IR signature (`SetLocalAt { value: OperandImm(_) }` /
  `SetLocalAt { value: LocalAt(_) }`), so the rewrites work on any ISA that provides them.

Soundness: the constant lattice resets to all-unknown at every basic-block leader (joins are
conservative); a folded constant is only *materialized* if it fits the immediate width (`i32`),
otherwise the original instruction is kept (still tracked). The rewrite is **length-preserving** (one
instruction → one instruction) so pc indices and branch targets stay valid — no renumbering.

## Verification (codegen = dangerous subsystem → verify-before-implement)

The differential tests assert **`jit(prog) == interpret(prog)`** with the interpreter as oracle:
- hand-written control-flow programs (`sum_program`, `factorial_program`);
- 2000 randomized straight-line arithmetic programs (deterministic LCG; reproducible).

For the optimizer, **`interpret(fold(p)) == interpret(p)` AND `jit(fold(p)) == interpret(p)`** over the
same control-flow samples + 2000 fuzz programs (which fold heavily — every register becomes constant
after the MovImm prelude — exercising the mixed materialized/kept path and the i32 overflow guard),
plus targeted unit tests (chained fold fires; overflow constant is *not* materialized; folding is
length-preserving). `cargo test -p otter -p otter-cranelift` is green. The terminator hint is also
cross-checked against the effects (`debug_assert`).

## Increment log

- **B0.1 — DONE (2026-06-16).** Reifiable IR + generic T0 interpreter (`otter`) + generic
  Cranelift JIT backend (`otter-cranelift`) + sample register ISA defined purely as data +
  differential verification (sum/factorial + 2000-program fuzz). Generalizes spikes S3/S3-2b (which
  hand-wrote two parallel backends for a hardcoded stack VM) into the real L0 crates with the single
  source actually shared. Sanity perf (not authoritative): sum 1..=50M interp ≈ 2.0 s vs JIT ≈ 14 ms.
  NB the generic `Expr`-tree interpreter is a naive reference oracle (slower than S3's closure-threaded
  one); T0 *specialization* (Deegen/RPython-style) is later work — correctness and the meta-property
  come first.
- **B0.2 — DONE (2026-06-16).** `opt::constant_fold` — hint-driven (`pure`/`terminator`) generic
  constant propagation + folding, the THIRD consumer of the single source; materializer op discovered
  from the ISA by IR signature; block-local sound lattice; i32-fit guard; length-preserving. Verified
  `interpret(fold)==interpret` AND `jit(fold)==interpret` on samples + 2000 fuzz, + targeted unit tests.
- **B0.3 — DONE (2026-06-16).** Adaptive **tier-up engine**: `engine::Backend` seam + `engine::Engine`
  (interpret → compile+cache on hotness threshold) in the safe core; functions take **arguments**
  (`interpret_with`; compiled `extern "C" fn(*const i64)->i64` loads args at entry). Verified: identical
  results across the interp→JIT boundary; arg-loading codegen differential; engine state machine.
- **B0.4 — DONE (2026-06-16).** **Multi-tier T0→T1→T2** + **T1 copy-and-patch baseline** (`otter-baseline`,
  x86-64). The engine was generalized to an ordered tier stack (trait-object `Backend`/`Code`, ascending
  thresholds, monotonic promotion to the highest eligible tier). `otter-baseline` hand-emits an x86-64
  template per IR node (W^X), driven by the same IR (so it too is single-source-derived); microsecond
  compile. Verified: `baseline == interpret` (samples + args + 2000 fuzz), `baseline == cranelift ==
  interpret`, and full T0→T1→T2 promotion with identical results at every tier. Measured trade-off:
  baseline compiles ≈7× faster than Cranelift; baseline run ≈26× over the interpreter, Cranelift ≈6×
  over baseline.

- **B0.5-1 — DONE (2026-06-16).** **OSR + deopt** via shared frame transfer. `Effect::Guard` (interpreter
  no-op); `otter-cranelift::compile_resumable`/`ResumableCode` (frame ABI, baked entry pc, guard→deopt
  spilling locals + pc); `interp::interpret_from` (deopt resume) and `interp::interpret_until_osr`
  (back-edge counter → OSR trigger); `run_resumable`/`osr_continue` wrappers. Verified: deopt preserves
  semantics (guarded program, n above/below the limit) and the deopt path is genuinely taken; OSR
  preserves semantics and fires only when hot; OSR-then-deopt composes. Baseline + non-resumable
  Cranelift treat `Guard` as a transparent no-op.

- **B0.5-2 — DONE (2026-06-16).** **Hint vocabulary expansion**: primitive algebraic-law hints on
  `BinOp` (`is_commutative`, `right_identity`, `absorbing_element`) + a sample `copy` op, consumed by a
  new **algebraic-simplification** path in `opt::constant_fold` (`x*0 → 0` even with `x` unknown;
  `x+0 / x*1 / x-0 → x` via a discovered copy). Verified by targeted unit tests + the existing 2000-fuzz
  differential (both backends now also execute the `copy` op, handled generically).

- **B1.2a — DONE (2026-06-16).** otter upgraded to a **language-agnostic typed word machine** with
  first-class **i64 + f64**: float `BinOp`s (`FAdd/FSub/FMul/FDiv/FLt/FEq`) + `UnOp` conversions
  (`I64ToF64`, saturating `F64ToI64`, `INeg`/`FNeg`) + `Expr::ImmF64`/`Expr::Un`. Implemented across ALL
  three tiers: interpreter, **Cranelift** (bitcast + fadd/fcmp/fcvt_to_sint_sat), and **baseline**
  (hand-written SSE: movq/addsd/ucomisd/cvtsi2sd/cvttsd2si). Verified bit-for-bit `interp == cranelift
  == baseline` on `(x+y)*x` over tricky inputs (±0, Inf, NaN) + conversion round-trips. Algebraic
  identities stay integer-only (unsound for `f64`). NB baseline `F64ToI64` uses raw `cvttsd2si`
  (truncation); NaN/overflow *saturation* in baseline is a follow-up (interp + Cranelift saturate).

- **B1.2-wide+ — DONE (2026-06-16).** **Width-parametric, language-agnostic** op set: `BinOp{ty,op}`
  over `NumTy{i8,i16,i32,i64, u8,u16,u32,u64, f32,f64}` + `UnOp{Neg(ty), Convert{from,to}}` (incl.
  unsigned per user request). Slot normalization (sign/zero-extend per type), narrow integer wrap,
  signed-vs-unsigned compare, saturating per-width float→int. Implemented across ALL three tiers:
  interpreter; **Cranelift** (`ireduce`/`sextend`/`uextend` normalize, `icmp` slt/ult, `fadd`/`fcmp` at
  F32/F64, `fcvt_*_sat` to I64 + width-clamp, `fpromote`/`fdemote`, `fcvt_from_{s,u}int`); **baseline**
  (hand SSE: `movsx`/`movzx` normalize, `setb`/`setl` compare, `addss`/`ucomiss`, `cvtsi2ss`/`cvtss2sd`/
  `cvttss2si` + `cmov` clamp). Differential-verified `interp == cranelift == baseline` on the width
  family. **f16 deferred** to B1.2-wide-f16 (needs the `half`/`f16-intrinsic` codec + bytecode-compat
  invariant test — see [[otter-primitive-types]]). Baseline caveats (documented on `emit_convert`):
  unsigned-int→float and float→int NaN/over-i64 edges (interp + Cranelift are fully correct).

- **B1.2-wide-f16 — DONE (2026-06-16).** Added `f16` (ES2025 `Float16Array`/`Math.f16round`). f16 is
  computed by a single config-dependent codec — `half` crate (default `half-codec`) or the native `f16`
  primitive (nightly `f16-intrinsic`, opting out of `half`); both are IEEE binary16 round-to-nearest, so
  bytecode results are identical regardless of build (gated by `f16_ieee_reference_table`, a hardcoded
  IEEE table both configs must pass). f16 arithmetic = `f64`-intermediate + single round. The JIT tiers
  don't lower f16 natively; they **`call` the canonical interpreter helpers** (`otter::otter_binop` /
  `otter::otter_convert`) via `call_indirect` (Cranelift) / an aligned `call` (baseline) — guaranteeing
  bit-identity across tiers and configs.
- **Baseline exceptions RESOLVED (2026-06-16).** The same helper-call path routes baseline's previously
  hand-encoded edge cases — **float→int saturation** (NaN/over-i64) and **unsigned-int→float** — through
  the canonical interpreter logic, so `baseline == interp == cranelift` on **all inputs** now (no
  documented exceptions). Verified by `baseline_f16_and_resolved_edges`.
- **Function calls (interpreter oracle) — DONE (2026-06-16).** otter grew a multi-function execution
  model: `FuncDef { nparams, consts, nlocals, code }` + `Module { funcs, fill, entry }`, two new effects
  `Effect::Call { dest, base, argc }` (operand-index fields; calling convention = a contiguous local
  block `[callee, arg0, …]`, the callee word a **raw `funcs` index**) and `Effect::Return { value }`,
  and a call-stack interpreter `interp::run_module`. Frame layout `[params][consts][temps]`, leading
  slots initialized at call time (`fill` = embedder's "uninitialized/undefined" word — missing args
  default to it). Per the PyPy methodology the JIT tiers do **not** lower calls yet: the single-frame
  interpreter / both backends carry exhaustive `Call`/`Return` arms (`unreachable!`/`unimplemented!`) and
  are only ever invoked on call-free functions. Verified by `run_module_recursion` (factorial via
  recursion) and `run_module_missing_args_use_fill`.
- **JIT call lowering (Cranelift) — DONE (2026-06-16).** `otter-cranelift::compile_module` compiles a
  whole `Module` to machine code: each function gets the uniform ABI
  `extern "C" fn(table, args, argc) -> i64`; inter-function calls lower to `call_indirect` through a
  **read-only function-pointer table** (callee index is a runtime value → first-class/dynamic callees and
  recursion work without baking addresses; a read-only table avoids reentrant-`&mut` aliasing). Constants
  and `fill` are baked per function; args are marshalled into a per-call stack buffer; the prologue copies
  parameters guarded by `argc` (absent args stay `fill`, never loaded OOB); `Halt`/`Return` both lower to
  a native `return`. No adaptive per-function tiering / interpreter fallback yet (whole module at the
  optimizing tier). Verified bit-identical to `interp::run_module` at the otter layer
  (`cranelift_module_matches_run_module`, hand-built recursive module) and on 10 real JS programs —
  recursion, fib, mutual recursion, higher-order, loops-in-functions, arg mismatch, returned function
  values, tagged values through calls (`jit_call_lowering_matches_oracle`).
- **Baseline (T1) call lowering — DONE (2026-06-16).** `otter-baseline::compile_module` is the hand
  x86-64 mirror of the Cranelift one (same read-only fn-ptr-table ABI, so the two backends' table entries
  are interchangeable). Per the design panel: `(table,args,argc)` spilled to fixed frame slots (the
  rax/rcx scratch + push/pop model untouched); the per-call argument buffer lives INSIDE the rbp-relative
  frame (a pre-scanned max-argc region) so the rsp-alignment dance can't corrupt it; the indirect call is
  rsp-aligned via the rbx idiom. **Bug caught by the differential tests** (the higher-order isolation
  case): frame slots grow downward but the callee reads an *ascending* arg array, so args must be stored
  in reversed slot order — invisible at argc≤1 (recursion passed), surfaced at a 2-arg higher-order call.
  Verified `interp == baseline == cranelift` on hand modules (recursion, higher-order) + 12 real JS
  programs incl. ≥3-arg calls.
- **Adaptive module engine — DONE (2026-06-16).** New crate `otter-engine` (std, concentrated unsafe):
  per-function tier-up (T0 `interp::run_one` → T1 baseline → T2 Cranelift) + cross-tier dispatch. Calls
  route through one Rust dispatcher `module_dispatch(ctx, callee_idx, args, argc)`; each backend gains a
  `compile_dispatch_fn` (ABI `extern "C" fn(ctx, args, argc)`, `Call` → that dispatcher) — the per-fn
  lowering is shared via `CallMode{Table,Dispatch}`. otter gains `interp::run_one` (single-function
  interpreter delegating `Call` to a closure). **Reentrancy-safe by the design-panel argument**: compiled
  code holds only an opaque `ctx` it forwards (never derefs engine state); `module_dispatch` reconstitutes
  a *shared* `&ModuleEngine` (no `&mut` ever); all mutation via `Cell`/`RefCell` with **no borrow held
  across a nested call**; compiled code is **add-only** (an in-flight older-tier frame stays valid through
  a tier-up). **Hardened after an adversarial review**: an out-of-range callee (an erroneous JS call of a
  non-function) and a corrupt/negative `argc` now yield a defined `fill` instead of an index panic, and
  `module_dispatch` wraps dispatch in `catch_unwind` so **no panic ever unwinds across the `extern "C"`
  boundary → abort** (a DoS the review reproduced). Verified `engine == run_module` across every tier mix
  incl. **mid-recursion tier-up**, monotonic tier-up, OOB-callee-traps-not-aborts, and 7 real JS programs
  on the adaptive engine. (Per-call dispatch overhead is the v1 cost; a read-only-table + per-fn-thunk
  refinement is a later perf step.)
- **Host-call op — DONE (2026-06-16).** New IR nodes `Expr::Host1(HostFn1, …)` / `Host2(HostFn2, …)`
  where `HostFn1/2 = extern "C" fn(Value[, Value]) -> Value`. The fn pointer is **baked into the IR**
  itself — no host table is threaded and no global is consulted, so otter keeps `forbid(unsafe_code)`
  (the interpreter just calls it; only the backends, already `allow(unsafe)`, bake its address). The
  interpreter calls it directly; cranelift `call_indirect`s a SystemV `(i64[,i64])->i64` signature to the
  baked address; baseline emits an rsp-aligned `call` to it (args marshalled into rdi/rsi exactly like
  the `otter_binop` helper). The optimizer treats Host exprs as opaque (`eval_const → None`), so a write
  through one correctly drops to "unknown" in the constant lattice; `pure` is safe to set since they are
  deterministic side-effect-free functions. This is the canonical extension point an embedder/runtime
  uses to expose builtins to the bytecode — the L1 JS runtime's `js_add`/`js_lt`/`js_truthy`/… ride it
  (see `crates/runtime/DESIGN.md`). Verified bit-identical across T0/T1/T2 by
  `host_call_op` (interp) and `host_call_op_agrees_across_tiers` (three-tier differential).

## Tier availability × architecture (final as of O-13, 2026-09-03)

Which execution tiers exist on which targets. Two sources feed this table: `otter-engine`'s cfg
gates (T1 = `x86_64 ∧ hosted`, T2 = `¬wasm ∧ hosted`, Twasm = `wasm32 ∧ feature "wasm-emit"`) and,
for T2's arch column, **Cranelift's backend support list** — the T2 cfg itself is arch-agnostic, so
on a hosted arch Cranelift does NOT support (e.g. ppc64le) the T2 slot still compiles and the first
tier-up panics in `JITBuilder::new()`'s host-ISA expect; such arches are outside this table and
effectively T0-with-a-caveat, not a supported configuration. Availability is NOT a parity gap
([[otter-tier-parity-policy]]): on any target, every AVAILABLE tier supports the full IR feature
set (TailCall included); a missing tier just means T0 fallback there.

| target | tiers | notes |
|---|---|---|
| x86_64 (hosted) | T0 + T1 + T2 | full ladder: interp → baseline (hand-emitted x86-64) → Cranelift |
| aarch64 / riscv64 / s390x (hosted) | T0 + T2 | Cranelift covers them; T1 is x86-64-only by construction |
| wasm32, feature `wasm-emit`, §7 build | T0 + **Twasm** | the O-13 wasm-emit tier: IR → fresh wasm module bytes → `otter_jit.compile` host import → shared-funcref-table `call_indirect`. Requires the O-13 §7 build configuration (nightly build-std, `-Cpanic=unwind`, wasm EH) and an EH-capable host with the `otter_jit` contract wired (`hosts/otter-host-wasmtime` native, `hosts/otter-glue-js` browser) |
| wasm32 (anything else) | T0 | feature-off builds import nothing extra — any wasm-EH-capable WASI runtime runs them as pure T0 (O-7 preserved) |
| bare metal (`target_os = "none"`) | T0 | no_std interp only; both compiled tiers are gated out |

Twasm trap contract: a genuine wasm trap (as opposed to a caught Rust panic) makes the
engine/instance discard-only — `try_dispatch`'s rustdoc carries the normative wording (O-13 §2.4).

## Next

- ongoing L1: strings + objects (needs `neosurf-gc` heap), functions/calls → `test262`.
- mid-term: target-arch validation ([[arch-support-tiers]]) — aarch64/riscv64 exist in the table
  above via Cranelift but still want dedicated cross-compile/CI validation; T3 POWER/SPARC/wasm64
  (T0-only). The "emit WASM instead of native" idea from the original note is DONE — it's the
  Twasm tier above (O-13).
