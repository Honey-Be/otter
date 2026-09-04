//! L0 **adaptive module engine** — per-function tier-up + cross-tier dispatch for `otter` function
//! [`Module`]s.
//!
//! The single-function [`otter::engine::Engine`] tiers up one function; this engine runs a whole
//! multi-function module where **each function tiers up independently** (T0 interpreter → T1 baseline →
//! T2 Cranelift) and a function at any tier may call a function at any *other* tier. Calls route through
//! one Rust dispatcher, [`module_dispatch`], which counts calls, lazily compiles hot functions, and
//! invokes the callee's current-best tier (or interprets it via [`otter::interp::run_one`]).
//!
//! O-13 (`wasm32` + feature `wasm-emit`): on that build configuration the two native compiled tiers are
//! compiled out and the ladder is **T0 → Twasm** — the wasm-emit tier (`otter-wasmemit` lowers the
//! function to a fresh wasm module's bytes, the `otter_jit.compile` host import turns them into a shared
//! funcref-table index, and `call_compiled` invokes it through the very same transmuted-fn-pointer path,
//! a wasm32 fn pointer being its table index). It reuses the `t2` threshold (design §6) and the same
//! sticky-refusal discipline as every other tier.
//!
//! ## Reentrancy safety (this is the `unsafe`/FFI-adjacent core — verify-before-implement)
//!
//! Compiled code is given ONLY an opaque `ctx` pointer (the engine) which it forwards, untouched, to
//! `module_dispatch`; it never dereferences engine state. `module_dispatch` reconstitutes a **shared**
//! `&ModuleEngine` from `ctx` and all mutation goes through `Cell`/`RefCell` (sound interior mutability
//! behind `&T`) — so **no `&mut ModuleEngine` ever exists**, and arbitrarily deep reentrant calls only
//! ever hold *shared* references (always sound to alias). No `RefCell` borrow is held across a nested
//! call (the raw code pointer is copied out and the borrow dropped first), so reentry cannot
//! double-borrow. Compiled code is **add-only** (a tier-up installs new code; old code is never freed
//! while the engine lives), so an in-flight call to an older tier stays valid. `ctx` points at the
//! `ModuleEngine` owned by the [`ModuleEngine::run`] caller's frame, which outlives the whole call tree.
//!
//! O-8c extends this same invariant to compiled-tier proper tail calls, at BOTH compiled tiers — T2
//! Cranelift (O-8c-1) and T1 baseline (O-8c-2): `tail_pending` / `tail_callee` / `tail_args` are three
//! more `Cell`/`RefCell` fields behind `&self`, written by the baked [`tail_signal`] helper and
//! read+cleared by [`ModuleEngine::dispatch`]'s trampoline loop immediately after the compiled call
//! returns. `tail_pending` is `true` only inside the narrow window between `tail_signal` setting it and
//! `dispatch` reading it — a window containing no Rust code on the compiled side (pure machine epilogue,
//! for either backend), so no other otter execution can observe or interleave with it. As of O-8c-2 a
//! `TailCall`-containing function has NO tier restriction of any kind — it promotes through T0 → T1 → T2
//! exactly like any other function (feature parity across tiers; only speed differs).
//! See `.claude-research-library/otter-o8-tailcall-design.md` §8.3/§8.7 for the full argument.
//!
//! O-9 ([`ModuleEngine::push_func`]) extends the SAME add-only invariant to the function table itself: a
//! live engine can be handed a brand-new [`otter::FuncDef`] and keep running. `otter::Module` is not
//! touched by this at all — the *original* index space (`idx < module.funcs.len()`) is exactly what it
//! was before O-9, byte-for-byte, so every reentrancy argument above still applies unchanged there. The
//! new *pushed* index space (`idx >= module.funcs.len()`) lives in parallel `RefCell<Vec<..>>` storage
//! (`pushed`/`pushed_counts`/`pushed_baseline`/`pushed_cranelift`); each pushed `FuncDef` is individually
//! heap-boxed (`Box<FuncDef>`) so growing/reallocating the *outer* `Vec` (e.g. a later `push_func` call)
//! never moves or frees a `FuncDef` an in-flight call already holds a reference to — see `push_func`'s and
//! `pushed`'s own docs for the full argument (O-9 design doc §1.2/§1.4). `push_func` takes only `&self`
//! and is explicitly reentrant, exactly like every other method here (O-9 design doc §2): it may be called
//! from arbitrarily deep inside an in-flight `dispatch` call tree.
//!
//! O-10 ([`ModuleEngine::try_dispatch`]/[`ModuleEngine::try_run`]/[`ModuleEngine::clear_trap`]) makes a
//! caught trap OBSERVABLE (`Result<Value, Trap>` instead of a poisoned engine silently returning `fill`
//! forever) and RECOVERABLE (`clear_trap` un-poisons the engine so it can keep running). `try_dispatch`
//! wraps its OWN call to the private `dispatch` in `catch_unwind` too, extending the exact same "never
//! abort, always contain" guarantee `module_dispatch`/`tail_signal` already give nested calls to the
//! OUTERMOST call as well (a gap `dispatch`/[`ModuleEngine::run`] still have, by design — see their own
//! doc comments). `clear_trap` only resets this engine's own `trapped`/`trapped_idx`/`tail_pending` state
//! — it is a poison-flag reset, not a fix for whatever caused the trap: a deterministic cause (malformed
//! bytecode, or a compiler bug a specific function's shape triggers) retraps identically on retry. See
//! `try_dispatch`/`clear_trap`'s own docs for the full contract.
//!
//! **Hard invariant this file relies on (pre-dates O-9, but O-9's reentrancy argument leans on it
//! directly):** every "a stray panic here is contained, not a process abort" claim in this file — for
//! `RefCell` double-borrows, malformed bytecode, anything — depends on this crate being built with
//! `panic = "unwind"`. Under `panic = "abort"` `catch_unwind` cannot catch anything and every
//! trap-instead-of-abort guarantee below degrades to a real process abort. This is not new to O-9, but is
//! worth restating here because it is the ONLY thing standing behind O-9's "reentrancy is safe even if the
//! borrow discipline below is violated" fallback argument. **O-12 enforces this at compile time** (see the
//! `#[cfg(panic = "abort")] compile_error!` below `use` at the top of this file) rather than leaving it a
//! documented-only precondition — this paragraph now documents WHY the requirement exists; the
//! `compile_error!` is what actually stops a `panic = "abort"` build of this crate (and, transitively,
//! every crate that depends on it — Rust cannot mix panic strategies within one link unit, so gating this
//! one crate is sufficient; see the O-12 design doc §2.1 for the measured proof).
//!
//! ## Known gaps this crate does NOT close (read before trusting "fail-closed" without qualification)
//!
//! O-12's `panic = "unwind"` build-time gate (above) and the three `catch_unwind` sites below close ONE
//! specific failure mode: a panic escaping this crate's `extern "C"` FFI boundary under `panic = "abort"`
//! aborting the whole process. It does NOT make every path through this crate fail-closed against
//! malformed bytecode. In particular, as of O-12:
//! - **[`ModuleEngine::run`]/the private `dispatch` (non-`try_*`) have an unprotected outermost call.**
//!   [`ModuleEngine::dispatch`]'s every iteration calls `maybe_tier_up`, which runs whether or not a
//!   compiled frame is anywhere on the call stack — so the FIRST-EVER call to `run`/`dispatch` for a given
//!   function (nothing compiled yet) is not wrapped by [`module_dispatch`]/[`tail_signal`]'s
//!   `catch_unwind` (those only wrap NESTED calls FROM already-compiled code). A panic there — a
//!   first-time tier-up compile hitting `otter-cranelift`'s `block_at[&target]` `HashMap` panic, or a
//!   purely-T0-interpreted call tree with no compiled frame anywhere hitting `otter::interp`'s indexing —
//!   propagates as an ordinary Rust unwind straight out of `run`/`dispatch` to THEIR caller, not a
//!   contained `Trap`. **Use [`ModuleEngine::try_run`]/[`ModuleEngine::try_dispatch`], never `run`/the
//!   private `dispatch`, for any bytecode you did not fully control the construction of.** This is not a
//!   new gap O-12 introduced — O-10's own doc already names it — but O-12 is the reason it is worth
//!   restating: under `panic = "abort"` this and every other gap below would ALSO have been a silent
//!   process abort; under the now-enforced `panic = "unwind"`, a panic that escapes `run`/`dispatch` is at
//!   least an ordinary Rust unwind an embedder's OWN `catch_unwind` can still contain, even though this
//!   crate's own containment does not reach it.
//! - **`otter::interp::run_module`/`run_one`, called directly (WITHOUT a `ModuleEngine`), have no
//!   `catch_unwind` at all** — this crate's containment lives entirely in [`module_dispatch`]/
//!   [`tail_signal`]/[`ModuleEngine::try_dispatch`], none of which exist on that path. Use those two
//!   functions only as the reference oracle (differential tests, offline evaluation of already-trusted
//!   bytecode) — never execute untrusted bytecode through them directly and expect containment.
//! - **`otter-cranelift`'s internal use of the Cranelift libraries can panic in ways this crate cannot
//!   enumerate or prevent** (third-party code) — `panic = "unwind"` + `catch_unwind` is the only mechanism
//!   that contains that residual risk, and it is a PERMANENT one, not something a future otter release is
//!   expected to close to zero.
//! - **None of the above indexing/panic sites are validated away before this crate ever sees the
//!   bytecode.** [`ModuleEngine::push_func`]/[`ModuleEngine::new`] accept a [`FuncDef`]/[`Module`] as-is
//!   (see their own docs) — `otter::validate::check_func`/`check_module` (a separate, OPT-IN static
//!   checker in the `otter` crate) can rule out the entire indexing-panic class (and, for
//!   `otter-baseline`, a panic-LESS memory-corruption class no amount of `catch_unwind` can touch) ahead
//!   of time, but only if the caller invokes it — this crate does not do so automatically.
//!
//! **Hard invariant O-9 adds:** [`ModuleEngine::push_func`] (and any future `&self` mutator on this
//! type) must NEVER become a call target baked directly into JIT'd code — only [`module_dispatch`] and
//! [`tail_signal`] may be, because only those two wrap the call in [`catch_unwind`] at the `extern "C"`
//! boundary. A future hostcall mechanism (tracked separately, O-11) that lets compiled code invoke an
//! engine mutator directly MUST route through a new `catch_unwind`-wrapped `extern "C"` trampoline of its
//! own — reusing `push_func`'s plain `&self` signature as a bare call target would silently reopen the
//! "panic unwinds across a non-unwind `extern "C"` boundary → abort" hole this crate exists to close.
#![allow(unsafe_code)] // FFI to JIT'd code (transmute + call); concentrated + argued above

use core::cell::{Cell, RefCell};
use core::ptr;
use std::panic::{AssertUnwindSafe, catch_unwind};

use otter::interp::TailReq;
use otter::{FuncDef, Isa, Module, Value};

// O-12: this crate's ENTIRE fail-closed story (module doc above, "Hard invariant this file relies on")
// depends on `panic = "unwind"` — `catch_unwind` in `module_dispatch`/`tail_signal`/`try_dispatch` cannot
// catch anything under `panic = "abort"` (a panic there is a bare process abort instead of a caught
// trap). This was previously a documented-only precondition; enforce it at compile time instead, so a
// release-profile size/perf tweak (or a target whose DEFAULT is abort — e.g. `wasm32-unknown-unknown`
// with no explicit `-C panic` override) fails loudly at `cargo check`/`cargo build` rather than silently
// reopening the DoS this crate exists to close.
//
// What this gate does NOT catch (see the module doc's "Known gaps" section for the full list): if you
// react to this error by explicitly passing `-C panic=unwind` on a target whose std was pre-built WITHOUT
// unwind-runtime support (e.g. `wasm32-unknown-unknown`'s shipped std, as of this toolchain) — this crate
// itself, and `cargo check -p otter-engine`, will then compile/typecheck FINE (the gate correctly does
// not fire, since panic really is "unwind" for THIS crate's `cfg` purposes), and the failure is deferred
// to whenever your OWN final binary/cdylib links: `error: the crate 'panic_unwind' does not have the
// panic strategy 'unwind'` — an ordinary rustc panic-runtime-selection error, not this message, and not
// specific to `otter-engine`. Do not read a clean `cargo check -p otter-engine` on such a target as proof
// `catch_unwind` will actually work there; verify by building your own final artifact.
#[cfg(panic = "abort")]
compile_error!(
    "otter-engine requires panic = \"unwind\": catch_unwind in module_dispatch/tail_signal/ \
     try_dispatch is this crate's ONLY containment for a panic caused by malformed bytecode, and it \
     does nothing under panic = \"abort\" (the process aborts immediately instead). If you need \
     panic = \"abort\" (binary size, a target whose std lacks unwind-runtime support, etc.), do not \
     feed untrusted/malformed bytecode to a ModuleEngine — see this crate's module docs (O-12), \
     including the \"Known gaps\" section on what panic = \"unwind\" alone does NOT fix on every target."
);

/// ABI of a compiled function (both tiers): `(ctx, args, argc) -> result`. `ctx` is threaded through to
/// [`module_dispatch`] for nested calls.
type CompiledFn = extern "C" fn(*const ModuleEngine, *const Value, i64) -> Value;

/// Type of [`module_dispatch`]; used to take its address (via a fn pointer, not a fn-item cast).
type DispatchPtr = extern "C" fn(*const ModuleEngine, i64, *const Value, i64) -> Value;

/// The address of [`module_dispatch`], baked into compiled code's `Call`s.
fn dispatch_addr() -> usize {
    module_dispatch as DispatchPtr as usize
}

/// The address of [`tail_signal`], baked into compiled code's `TailCall`s (O-8c, both compiled tiers —
/// T2 Cranelift since O-8c-1, T1 baseline since O-8c-2).
fn tail_signal_addr() -> usize {
    tail_signal as DispatchPtr as usize
}

/// O-13 W-3: the wasm-emit (Twasm) tier's per-function slot — a TRI-state, deliberately not an
/// `Option`, because "tried and refused" must be remembered (sticky-refusal, the O-8 fix T8
/// pattern): a backend [`otter_wasmemit::Refusal`] or a host failure code is permanent for that
/// function (no retry loop — design §5.1), so the only transition a slot ever makes is
/// `Untried` → (`Refused` | `Compiled`), exactly once. `Compiled` is add-only like every other
/// tier's slot: once installed it is never replaced or removed for the life of the engine (the
/// baked scratch address depends on this — see [`CompiledWasmFn::scratch`]).
#[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
enum WasmSlot {
    /// No compile attempt yet — the ONLY state `maybe_tier_up` acts on.
    Untried,
    /// The backend refused, the host returned an error code, or the host's return violated the
    /// §5.1 contract (`idx` outside `1..=u32::MAX`) — sticky: this function stays on T0 forever.
    Refused,
    /// Compiled, host-instantiated, and registered in the shared funcref table.
    Compiled(CompiledWasmFn),
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
impl WasmSlot {
    /// The `Compiled` "code pointer" ([`CompiledWasmFn::code_ptr`]), `None` otherwise — shaped so
    /// [`ModuleEngine::wasm_ptr`] keeps the exact single-expression borrow discipline of
    /// [`ModuleEngine::cranelift_ptr`] despite the third state.
    fn code_ptr(&self) -> Option<*const u8> {
        match self {
            WasmSlot::Compiled(c) => Some(c.code_ptr()),
            WasmSlot::Untried | WasmSlot::Refused => None,
        }
    }
}

/// A function compiled to the wasm-emit tier (O-13 design §6): the funcref-table index the host
/// registered the emitted module's export under (§5.1/§5.3), plus the function's own marshaling
/// scratch buffer (§2.5-1) whose address is baked into the emitted code.
#[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
struct CompiledWasmFn {
    /// The shared-funcref-table index `otter_jit.compile` returned (`> 0`, range-checked at
    /// install time — §5.1 engine-side defense). On wasm32 a function "pointer" IS a table index
    /// (§2, measured; W-0 M3 re-verified on wasip1), so this is the exact analogue of the native
    /// tiers' machine-code address.
    table_idx: u32,
    /// This function's per-function, exact-size marshaling scratch (§2.5-1): every `Call`/
    /// `TailCall` site in the emitted code `i64.store`s its argument words here and passes this
    /// buffer's address to `module_dispatch`/`tail_signal`. The address was mined ONCE, via
    /// `as_mut_ptr()` (write provenance), immediately after allocation, and baked into the
    /// emitted code (§13-5); no Rust reference into the buffer is ever formed again except the
    /// TWO momentary copy windows — `module_dispatch`'s `to_vec` slice and `tail_signal`'s
    /// slice→`extend_from_slice` window (an emitted `TailCall` passes this same scratch address)
    /// — both guest-execution-free, so neither borrow can be live while any code writes the
    /// buffer (W-3 borrow-window review, finding #1: the earlier "single carve-out" wording
    /// under-stated the aliasing surface).
    ///
    /// **Address stability comes from HEAP INDIRECTION, not from add-only (§13-2)** — the same
    /// argument as [`ModuleEngine::pushed_baseline`]'s field doc, which this deliberately
    /// mirrors: add-only only rules out *freeing*; it does not stop the outer
    /// [`ModuleEngine::pushed_wasm`] `Vec` from reallocating on a later `push_func` and moving
    /// this very `CompiledWasmFn` value (inside its `RefCell<WasmSlot>`) to a new address. That
    /// move copies only the `Box`'s (ptr, len) fat pointer — the separately-allocated `[Value]`
    /// heap block it addresses never moves, so the baked address stays valid for the life of the
    /// engine (and the block is only freed with the engine, because the slot IS add-only —
    /// [`WasmSlot`]). For exactly this reason the scratch MUST stay a separately heap-allocating
    /// container (`Box<[Value]>`/`Vec<Value>`): an inline-storage container (SmallVec and
    /// friends) would keep the words inside this struct's own bytes, and the outer-`Vec`
    /// reallocation WOULD then move them — silently invalidating every baked store/args address
    /// in already-registered wasm code (§13-2(b): inline storage is forbidden).
    // dead_code: correct as far as rustc can see — no Rust code ever reads this field again (the
    // §13-5 provenance rule FORBIDS re-deriving pointers from it). Its whole job is ownership:
    // keeping the heap block alive and address-stable for the emitted code that reads/writes it
    // through the baked address. Removing it (what the lint suggests) would free the scratch and
    // dangle every registered wasm function's marshaling stores.
    #[allow(dead_code)]
    scratch: Box<[Value]>,
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
impl CompiledWasmFn {
    /// The value [`ModuleEngine::best_code`] hands to `call_compiled`: the table index threaded
    /// through the existing `*const u8` plumbing. Not a pun this crate invented: on wasm32,
    /// `call_compiled`'s `transmute` + call compiles to `i32.wrap_i64` + `call_indirect` (§2,
    /// measured — design §2.3 #13's "source-unchanged" row), so an integer table index IS what a
    /// compiled-code "pointer" already is there.
    fn code_ptr(&self) -> *const u8 {
        self.table_idx as usize as *const u8
    }
}

/// §5.1: the host-contract import surface (wasm import namespace `otter_jit`) — present ONLY in
/// wasm-emit builds. Core wasm has no optional imports, so a feature-off build must not declare
/// this at all or it could no longer instantiate on a plain WASI runtime (O-7 preserved; the
/// whole reason the tier is feature-gated on top of the target gate — design §5.1).
#[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
mod otter_jit {
    #[link(wasm_import_module = "otter_jit")]
    unsafe extern "C" {
        /// Compile + instantiate the wasm module bytes at `[ptr, ptr+len)` of this instance's
        /// linear memory, register its export `"f"` in the shared funcref table, and return the
        /// new table index (`> 0`). Negative = §5.1 error code; `0` is contract-forbidden (the
        /// null funcref slot). The engine treats EVERY return outside `1..=u32::MAX` as a sticky
        /// per-function refusal (§5.1 engine-side defense) — never a panic, never a retry.
        pub(super) fn compile(ptr: u32, len: u32) -> i64;
    }
}

/// The adaptive engine for one `otter` function [`Module`] (one per JS agent, like the heap).
pub struct ModuleEngine {
    isa: Isa,
    module: Module,
    counts: Box<[Cell<u32>]>,
    #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
    baseline: Box<[RefCell<Option<otter_baseline::CompiledDispatchFn>>]>,
    #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
    cranelift: Box<[RefCell<Option<otter_cranelift::CompiledDispatchFn>>]>,
    #[cfg_attr(not(all(target_arch = "x86_64", not(target_os = "none"))), allow(dead_code))]
    t1_threshold: u32,
    // O-13 (design §6, B#8b): the pre-W-3 predicate here was `not(<cranelift cfg>)` — written when
    // "wasm ⇒ no compiled tier" held. The wasm-emit tier reuses `t2_threshold` (its compile cost is
    // T2-class; `t1_threshold` stays unused on wasm), so the allow must NOT fire on
    // wasm32+wasm-emit builds, where the field is genuinely read again.
    #[cfg_attr(
        not(any(
            all(not(target_family = "wasm"), not(target_os = "none")),
            all(target_arch = "wasm32", feature = "wasm-emit")
        )),
        allow(dead_code)
    )]
    t2_threshold: u32,
    /// Set if a panic was caught crossing the FFI boundary (malformed bytecode) — the engine is then
    /// poisoned and every further dispatch returns `fill`, so a trap can never abort the process.
    trapped: Cell<bool>,
    /// O-10: the function index in flight at whichever panic-catching boundary ([`module_dispatch`],
    /// [`tail_signal`], or [`ModuleEngine::try_dispatch`]'s own boundary) most recently caught a panic —
    /// always `Some` whenever `trapped` is `true` (all three call sites set this together, in the same
    /// breath as `trapped.set(true)`), and reset to `None` by [`ModuleEngine::clear_trap`] for hygiene
    /// (never read while [`Self::trapped`] is `false`). See [`Trap::idx`]'s doc for what this value means
    /// (a diagnostic hint — the nearest available site, not a full backtrace).
    trapped_idx: Cell<Option<usize>>,
    /// O-8c compiled-tier tail-call trampoline signal (shared by BOTH compiled tiers — T2 Cranelift,
    /// T1 baseline): the baked [`tail_signal`] helper sets these three, then the compiled function
    /// returns immediately (pure machine epilogue, no Rust in between, on either backend) so
    /// [`ModuleEngine::dispatch`]'s loop can read+clear them and re-enter the callee without recursing —
    /// see the module doc's reentrancy-safety section.
    tail_pending: Cell<bool>,
    tail_callee: Cell<usize>,
    tail_args: RefCell<Vec<Value>>,
    /// O-9: functions injected at runtime by [`ModuleEngine::push_func`], addressed as indices
    /// `>= module.funcs.len()`. Each element is individually heap-boxed so that growing the OUTER `Vec`
    /// (which may reallocate and memcpy the `Box` pointers themselves) NEVER moves or frees the `FuncDef`
    /// a `Box` points at — this is what gives an already-extracted `&FuncDef` (held live across a nested
    /// `dispatch` call, e.g. while a pushed function recurses, or while the host injects more functions
    /// mid-call — O-9 design doc §1.2/§1.4) address stability across concurrent/reentrant `push_func`
    /// calls, the same way `baseline`/`cranelift` above get it from being fixed-size.
    ///
    /// **SAFETY-CRITICAL INVARIANT (add-only, all the way down):** once an index has a `Box<FuncDef>`
    /// here, that slot must NEVER be replaced or removed for the life of the engine. Violating this is
    /// NOT caught by this file's usual `RefCell`-double-borrow fail-closed safety net (module doc): the
    /// `&FuncDef` handed out by [`ModuleEngine::func_ref`] has already released its `Ref` guard by design
    /// (that's the whole point — see `func_ref`'s doc), so replacing the boxed value out from under a
    /// still-live reference is silent, undetected Undefined Behavior (a dangling read), not a panic. A
    /// future "redefine an already-pushed function" feature (e.g. REPL top-level re-declaration) MUST NOT
    /// reuse this field for that purpose — it needs its own design (generation counters / epoch-based
    /// invalidation), never a slot replacement here.
    // `clippy::vec_box` doesn't know why: the `Box` isn't for de-duplicated heap allocation (clippy's
    // usual concern), it's SPECIFICALLY so growing/reallocating this `Vec` moves only 8-byte `Box`
    // pointers, never the `FuncDef` bytes those pointers address — the field doc above is the actual
    // justification (O-9 design doc §1.2, Alternative C vs the rejected `RefCell<Vec<FuncDef>>`
    // Alternative B, which this lint would otherwise steer us straight back into).
    #[allow(clippy::vec_box)]
    pushed: RefCell<Vec<Box<FuncDef>>>,
    /// Call counters for `pushed`, index-aligned to it. Plain `Cell<u32>` per slot — no address-stability
    /// requirement (nothing ever holds a `&Cell` across a nested call, only `.get()`/`.set()` snapshots,
    /// mirroring `counts` above) — but the OUTER `Vec` itself needs a `RefCell` (unlike `counts`'s fixed
    /// `Box<[..]>`) because `push_func` grows it.
    pushed_counts: RefCell<Vec<Cell<u32>>>,
    /// Compiled baseline (T1) code for `pushed`, same target-gate as `baseline`. The OUTER `RefCell<Vec<..>>`
    /// is growable (unlike `baseline`'s fixed `Box<[..]>`); each slot is its own `RefCell<Option<..>>` so
    /// [`ModuleEngine::maybe_tier_up`] can still install code with a single per-slot `borrow_mut()`, exactly
    /// like the original segment. Address stability does NOT come from the `Box`/`Vec` layout here (unlike
    /// `pushed` above) — it comes from `CompiledDispatchFn::code_ptr()` returning a *value-copied* raw
    /// pointer into a SEPARATELY allocated executable buffer (an `ExecBuf`/mmap owned by the
    /// `CompiledDispatchFn`, not the `CompiledDispatchFn` Rust value's own bytes), so relocating the
    /// `CompiledDispatchFn` value itself (e.g. via an outer-`Vec` reallocation) never moves or invalidates
    /// the machine code the pointer refers to (verified against `otter_baseline::CompiledDispatchFn`'s
    /// actual layout — O-9 design doc §6 risk #2 / panel Lens A finding #2).
    #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
    pushed_baseline: RefCell<Vec<RefCell<Option<otter_baseline::CompiledDispatchFn>>>>,
    /// Compiled Cranelift (T2) code for `pushed`, same target-gate + address-stability argument as
    /// `pushed_baseline` (see its doc — `JITModule`-owned code buffer, not `CompiledDispatchFn`'s own
    /// bytes).
    #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
    pushed_cranelift: RefCell<Vec<RefCell<Option<otter_cranelift::CompiledDispatchFn>>>>,
    /// O-13 W-3: the wasm-emit (Twasm) tier for the ORIGINAL index space — tri-state slots
    /// ([`WasmSlot`]) instead of the native tiers' `Option`, because a refusal is sticky (§5.1;
    /// O-8 fix T8). Fixed-size like `baseline`/`cranelift`, same borrow discipline (every access
    /// in `wasm_ptr`/`wasm_untried`/`install_wasm` is a single expression, no borrow held across
    /// any other call).
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    wasm: Box<[RefCell<WasmSlot>]>,
    /// O-9 mirror of [`Self::wasm`] for the pushed segment, growable like `pushed_cranelift`
    /// (outer `RefCell<Vec<..>>`, per-slot `RefCell`), appended by `push_func` as the FIFTH
    /// participant in its acquire-everything-then-append discipline (design §6 B#8c). Address
    /// stability of the scratch a `Compiled` slot owns does NOT come from this `Vec` (which
    /// reallocates and moves the slot values wholesale) — it comes from
    /// [`CompiledWasmFn::scratch`]'s heap indirection; see that field's doc (§13-2).
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    pushed_wasm: RefCell<Vec<RefCell<WasmSlot>>>,
}

impl ModuleEngine {
    /// Build an engine. `t1`/`t2` are the call counts at which a function is promoted to the baseline /
    /// Cranelift tiers (`t1 <= t2`; `0` would compile on the first call).
    ///
    /// `module`'s code is not validated against `isa` here — malformed bytecode is only ever caught
    /// lazily, the first time the offending function is actually dispatched, via the trap mechanism
    /// ([`Self::trapped`]/[`Self::try_run`]). **If `module` comes from a source you did not fully control
    /// the construction of** (an untrusted compiler, a fuzzer, deserialized input), call
    /// `otter::validate::check_module(&isa, &module)` first and reject on `Err` — see this crate's module
    /// doc's "Known gaps" section (O-12) for why lazy trapping alone is not equivalent to pre-validation
    /// (in particular, it does not close `otter-baseline`'s panic-less local-index memory corruption).
    pub fn new(isa: Isa, module: Module, t1: u32, t2: u32) -> Self {
        assert!(t1 <= t2, "t1 threshold must be <= t2");
        let n = module.funcs.len();
        let counts = (0..n).map(|_| Cell::new(0)).collect();
        // O-8c-2: no per-function tier-eligibility pre-scan is needed anymore. O-8b/O-8c-1 restricted
        // `TailCall`-containing functions from T1 (and, briefly during O-8c-1, tracked T2 separately) via
        // a `TierCaps` table; now that BOTH T1 (baseline) and T2 (cranelift) lower `TailCall` (via the
        // `tail_signal` trampoline signal), a `TailCall`-containing function tiers up exactly like any
        // other function — no restriction of any kind remains (design §8.7 O-8c-2: "TierCaps 완전 해제").
        ModuleEngine {
            isa,
            module,
            counts,
            #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
            baseline: (0..n).map(|_| RefCell::new(None)).collect(),
            #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
            cranelift: (0..n).map(|_| RefCell::new(None)).collect(),
            #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
            wasm: (0..n).map(|_| RefCell::new(WasmSlot::Untried)).collect(),
            t1_threshold: t1,
            t2_threshold: t2,
            trapped: Cell::new(false),
            trapped_idx: Cell::new(None),
            tail_pending: Cell::new(false),
            tail_callee: Cell::new(0),
            tail_args: RefCell::new(Vec::new()),
            pushed: RefCell::new(Vec::new()),
            pushed_counts: RefCell::new(Vec::new()),
            #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
            pushed_baseline: RefCell::new(Vec::new()),
            #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
            pushed_cranelift: RefCell::new(Vec::new()),
            #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
            pushed_wasm: RefCell::new(Vec::new()),
        }
    }

    /// Whether a trap (caught FFI-boundary panic on malformed bytecode) has poisoned this engine.
    pub fn trapped(&self) -> bool {
        self.trapped.get()
    }

    /// Run the module's entry function with `args`. Equal to [`otter::interp::run_module`] for the same
    /// module + args, at every mix of tiers (verified by the differential tests).
    pub fn run(&self, args: &[Value]) -> Value {
        self.dispatch(self.module.entry, args)
    }

    /// Current tier of function `idx` (`"interpreter"` / `"baseline"` / `"cranelift"` /
    /// `"wasm-emit"`) — for tests/intro.
    /// On targets where a tier is compiled out, that tier is simply never reported. Works identically for
    /// an `idx` in the original module or the O-9 pushed segment (routed via [`Self::cranelift_ptr`] /
    /// [`Self::baseline_ptr`] / [`Self::wasm_ptr`]).
    pub fn tier_of(&self, idx: usize) -> &'static str {
        // O-13: on wasm32 both native tiers below are compiled out, so this is the only compiled-
        // tier arm there (its cfg is disjoint from both of theirs — source order is cosmetic).
        #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
        if self.wasm_ptr(idx).is_some() {
            return "wasm-emit";
        }
        #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
        if self.cranelift_ptr(idx).is_some() {
            return "cranelift";
        }
        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        if self.baseline_ptr(idx).is_some() {
            return "baseline";
        }
        let _ = idx;
        "interpreter"
    }
    /// How many times function `idx` has been dispatched (original or O-9 pushed). `0` for an out-of-range
    /// pushed `idx` (never negative/panicking — mirrors `dispatch`'s own fail-closed OOB handling).
    pub fn calls(&self, idx: usize) -> u32 {
        let base = self.base();
        if idx < base {
            return self.counts[idx].get();
        }
        self.pushed_counts.borrow().get(idx - base).map_or(0, Cell::get)
    }

    /// The dispatcher: count the call, lazily compile `idx` to its due tier, then run it at the
    /// current-best tier (or interpret) — looping in place on a proper tail call instead of recursing, so
    /// unbounded tail recursion runs in bounded native stack (design §8.2). Reentrant; holds only shared
    /// `&self` (see the module docs).
    ///
    /// Two tail-signal mechanisms feed this ONE loop (O-8c unifies them behind a single discriminant
    /// check per iteration): a compiled (T2) callee signals via the engine's `tail_pending`/`tail_callee`/
    /// `tail_args` fields (the [`tail_signal`] baked helper — see the module doc's reentrancy argument);
    /// an interpreted (T0) callee signals via `run_one`'s local out-param [`TailReq`] (O-8b — kept exactly
    /// as-is, no engine-field involvement needed there since `run_one` already returns to this same
    /// frame). Either way, `idx`/`cur` are updated and the loop re-enters WITHOUT a new Rust stack frame.
    fn dispatch(&self, mut idx: usize, args: &[Value]) -> Value {
        let mut owned: Vec<Value> = Vec::new();
        let mut cur: &[Value] = args;
        let mut tail = TailReq::default();
        loop {
            // Fail-closed, never abort: a poisoned engine or an out-of-range callee (e.g. an erroneous JS
            // call of a non-function → `js_func_index` returns a sentinel) yields a defined `fill` value
            // consistently across all tiers, instead of an index panic that would abort across
            // `extern "C"`. `in_range` covers BOTH the original module (`idx < base`, no borrow — the hot
            // path is unchanged from pre-O-9) and the O-9 pushed segment (`idx >= base`, one momentary
            // `pushed` borrow, dropped before this statement ends).
            if self.trapped.get() || !self.in_range(idx) {
                return self.module.fill;
            }
            self.bump_count(idx);
            self.maybe_tier_up(idx);

            if let Some(p) = self.best_code(idx) {
                let v = self.call_compiled(p, cur);
                if !self.tail_pending.get() {
                    return v;
                }
                // Consume the signal IMMEDIATELY (before doing anything else) — the reentrancy invariant
                // (module doc + design §8.3) requires `tail_pending` to be read-and-cleared in the same
                // breath, so a nested `dispatch()` never sees a stale flag belonging to an outer frame.
                self.tail_pending.set(false);
                idx = self.tail_callee.get();
                core::mem::swap(&mut owned, &mut self.tail_args.borrow_mut());
                cur = &owned;
                continue;
            }
            // Interpret with a tail-call out-param. `run_one` fills `tail` + returns (without recursing)
            // on a `TailCall`; `tail` is a per-dispatch LOCAL (not thread-global), so a trap/panic
            // unwinding past here leaves no stale tail request to leak into a later run.
            tail.pending = false;
            // `func_ref` returns `Some` here because `in_range` (checked at the top of this iteration)
            // and `func_ref` consult the SAME underlying storage (`module.funcs` / `pushed`) for
            // existence, and neither this engine nor otter's bytecode ever removes a pushed function
            // (add-only — see `pushed`'s field doc) — so the range check above cannot go stale by the
            // time we get here (no other code runs in between that could shrink either store).
            let f = self
                .func_ref(idx)
                .expect("idx passed the in_range check above; module.funcs/pushed are add-only");
            let v = otter::interp::run_one(
                &self.isa,
                f,
                cur,
                self.module.fill,
                self.module.host_ctx,
                &mut |callee, cargs| self.dispatch(callee, cargs),
                &mut tail,
            );
            if !tail.pending {
                return v;
            }
            idx = tail.callee;
            core::mem::swap(&mut owned, &mut tail.args);
            cur = &owned;
        }
    }

    /// The boundary between the "original" index space (`self.module.funcs`, which never grows — see the
    /// module doc) and the O-9 "pushed" index space opened by [`Self::push_func`].
    fn base(&self) -> usize {
        self.module.funcs.len()
    }

    /// Whether `idx` currently addresses a function, in either index space. For `idx >= base` this takes
    /// one momentary shared borrow of `pushed` (dropped at the end of this expression, per this file's
    /// reentrancy discipline — module doc); for `idx < base` (the overwhelmingly common case) it's a
    /// plain integer comparison, no borrow at all, so the original segment's hot path is unaffected by
    /// O-9's existence.
    fn in_range(&self, idx: usize) -> bool {
        let base = self.base();
        idx < base || idx - base < self.pushed.borrow().len()
    }

    /// Bump the call counter for `idx`, in either index space, with the SAME saturating (never-panicking)
    /// arithmetic on both sides (O-9 panel finding: the pushed segment must not silently drop the
    /// overflow guard `counts` already has). `idx` is assumed already range-checked by the caller
    /// (`dispatch`'s loop head, which runs before this on every iteration); an out-of-range pushed `idx`
    /// is a silent no-op here rather than a panic, matching this file's fail-closed style.
    fn bump_count(&self, idx: usize) {
        let base = self.base();
        if idx < base {
            self.counts[idx].set(self.counts[idx].get().saturating_add(1));
            return;
        }
        if let Some(c) = self.pushed_counts.borrow().get(idx - base) {
            c.set(c.get().saturating_add(1));
        }
    }

    /// The `FuncDef` for `idx`, in either index space — `None` if out of range.
    ///
    /// For the pushed segment this borrows `pushed`, extracts a raw pointer via `Box::as_ref()`, and
    /// drops the `Ref` guard *before* returning — the `&FuncDef` handed back is NOT tied to a live `Ref`,
    /// so holding it across a nested `dispatch()` call (the whole point of `push_func` being reentrant,
    /// and exactly what `dispatch`'s interpreter path does across `run_one`) cannot conflict with a
    /// `push_func` call made from inside that nested call. Sound ONLY because `pushed` is add-only (see
    /// its field doc) — the `Box`'s heap block is never freed or replaced while `self` lives; see the
    /// module doc's reentrancy-safety section and the O-9 design doc §1.2/§1.4.
    fn func_ref(&self, idx: usize) -> Option<&FuncDef> {
        let base = self.base();
        if idx < base {
            return self.module.funcs.get(idx);
        }
        let pushed = self.pushed.borrow();
        let ptr: *const FuncDef = pushed.get(idx - base)?.as_ref();
        drop(pushed); // release the guard BEFORE handing out a reference derived from it (see fn doc)
        // SAFETY: see this fn's doc — `pushed` is add-only, so the pointee outlives `self`.
        Some(unsafe { &*ptr })
    }

    /// The current-best compiled code pointer for `idx` (Cranelift T2 preferred over baseline T1; on a
    /// wasm-emit build the wasm tier is the ONLY compiled tier, its "pointer" being the funcref table
    /// index), or `None` to interpret. The `RefCell` borrow(s) are dropped before returning so a
    /// reentrant call can't double-borrow; every tier is add-only so the pointer stays valid. Tiers
    /// compiled out on this target contribute nothing. Routes through
    /// [`Self::cranelift_ptr`]/[`Self::baseline_ptr`]/`wasm_ptr`, which handle the original/pushed split
    /// identically to every other per-function accessor here.
    fn best_code(&self, idx: usize) -> Option<*const u8> {
        let code: Option<*const u8> = None;
        #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
        let code = code.or_else(|| self.cranelift_ptr(idx));
        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        let code = code.or_else(|| self.baseline_ptr(idx));
        // O-13: on wasm32 the two native tiers above are compiled out, so wasm-emit is the sole
        // (and therefore best) compiled tier there — same `or_else` chaining discipline.
        #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
        let code = code.or_else(|| self.wasm_ptr(idx));
        code
    }

    /// The Cranelift (T2) code pointer for `idx`, in either index space — `None` if not compiled to this
    /// tier yet (or `idx` is out of range in the pushed segment). Each branch is a SINGLE expression (no
    /// named `Ref` binding that could outlive it) — for the pushed segment this is a doubly-nested
    /// `RefCell` read (outer `pushed_cranelift` Vec-level borrow, inner per-slot `Option` borrow), and
    /// both borrows end when this expression does, same discipline as the original segment's single
    /// borrow (O-9 panel finding: this doubly-nested read path needed its own explicit walk-through, not
    /// just an appeal to the single-borrow original-segment argument).
    #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
    fn cranelift_ptr(&self, idx: usize) -> Option<*const u8> {
        let base = self.base();
        if idx < base {
            return self.cranelift[idx].borrow().as_ref().map(|c| c.code_ptr());
        }
        self.pushed_cranelift
            .borrow()
            .get(idx - base)
            .and_then(|slot| slot.borrow().as_ref().map(|c| c.code_ptr()))
    }

    /// The baseline (T1) code pointer for `idx`, in either index space. See [`Self::cranelift_ptr`] (same
    /// shape, same single-expression discipline).
    #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
    fn baseline_ptr(&self, idx: usize) -> Option<*const u8> {
        let base = self.base();
        if idx < base {
            return self.baseline[idx].borrow().as_ref().map(|c| c.code_ptr());
        }
        self.pushed_baseline
            .borrow()
            .get(idx - base)
            .and_then(|slot| slot.borrow().as_ref().map(|c| c.code_ptr()))
    }

    /// The wasm-emit (Twasm) "code pointer" (= funcref-table index — [`CompiledWasmFn::code_ptr`])
    /// for `idx`, in either index space — `None` unless the slot is [`WasmSlot::Compiled`]. Same
    /// single-expression borrow discipline as [`Self::cranelift_ptr`] (same shape: each branch is
    /// one expression; the outer `pushed_wasm` Vec-level borrow and the inner per-slot borrow both
    /// end when it does).
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    fn wasm_ptr(&self, idx: usize) -> Option<*const u8> {
        let base = self.base();
        if idx < base {
            return self.wasm[idx].borrow().code_ptr();
        }
        self.pushed_wasm
            .borrow()
            .get(idx - base)
            .and_then(|slot| slot.borrow().code_ptr())
    }

    /// Whether `idx`'s wasm slot is still [`WasmSlot::Untried`] — the ONLY state `maybe_tier_up`
    /// acts on (`Refused` is sticky, never retried — §5.1; `Compiled` is done). An out-of-range
    /// pushed `idx` reports `false` (defensive no-op, `bump_count`'s style). Same
    /// single-expression borrow discipline as [`Self::cranelift_ptr`].
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    fn wasm_untried(&self, idx: usize) -> bool {
        let base = self.base();
        if idx < base {
            return matches!(*self.wasm[idx].borrow(), WasmSlot::Untried);
        }
        self.pushed_wasm
            .borrow()
            .get(idx - base)
            .is_some_and(|slot| matches!(*slot.borrow(), WasmSlot::Untried))
    }

    /// Install the outcome of the one-and-only wasm compile attempt for `idx` ([`WasmSlot::Compiled`]
    /// or the sticky [`WasmSlot::Refused`] — never `Untried`), in either index space. Single
    /// `borrow_mut()` per branch, no borrow held across any other call —
    /// [`Self::install_cranelift`]'s discipline. The caller (`maybe_tier_up`) only reaches this
    /// when the slot is currently `Untried`, so a `Compiled` value is never replaced — the
    /// add-only invariant the baked scratch address relies on ([`CompiledWasmFn::scratch`]).
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    fn install_wasm(&self, idx: usize, slot: WasmSlot) {
        let base = self.base();
        if idx < base {
            *self.wasm[idx].borrow_mut() = slot;
            return;
        }
        if let Some(s) = self.pushed_wasm.borrow().get(idx - base) {
            *s.borrow_mut() = slot;
        }
    }

    /// The §2.3 helper-index bundle for the wasm backend. On wasm32 a fn pointer IS its
    /// funcref-table index (§2, measured; W-0 M3 re-verified on wasip1), so the very expressions
    /// the native engine already bakes as call targets — `module_dispatch as DispatchPtr as usize`
    /// etc. — ARE the `call_indirect` indices the emitted module needs: the resolver-side identity
    /// mapping of design §2.3's index-externalization principle, no registry, no indirection.
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    fn wasm_helper_idx() -> otter_wasmemit::HelperIdx {
        // usize is 32 bits here (the cfg pins target_arch = "wasm32" — §13-3), so every
        // `as u32` below is lossless. The helper casts go through explicit fn-pointer types
        // first (never a bare fn-item cast), the same discipline as `DispatchPtr`.
        otter_wasmemit::HelperIdx {
            dispatch: dispatch_addr() as u32,
            tail: tail_signal_addr() as u32,
            binop: otter::otter_binop as extern "C" fn(Value, Value, u32, u32) -> Value as usize as u32,
            convert: otter::otter_convert as extern "C" fn(Value, u32, u32) -> Value as usize as u32,
            unop: otter::otter_unop as extern "C" fn(Value, u32, u32) -> Value as usize as u32,
        }
    }

    /// The one-shot wasm-emit compile for `idx` (design §6's `maybe_tier_up` branch, O-13 W-3):
    /// returns the slot to install — `Compiled` on full success, `Refused` on ANY failure
    /// (backend [`otter_wasmemit::Refusal`], host error code, or a contract-violating host
    /// return). Never panics (O-8 fix T9): every failure is a value, and the installed `Refused`
    /// is sticky, so `otter_jit.compile` is called at most once per function, ever (§8c-5).
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    fn compile_wasm(&self, idx: usize, f: &FuncDef) -> WasmSlot {
        // 1. Scratch size first (§2.5-1). `scratch_words` runs `check_func` + the §4.6-5 locals
        //    cap itself, so a certain-to-refuse function refuses HERE — before any allocation
        //    (the backend mirrors the cap precisely so allocate-then-compile callers like this
        //    one can't be lured into a large dead allocation).
        let words = match otter_wasmemit::scratch_words(&self.isa, f) {
            Ok(w) => w,
            Err(_) => return WasmSlot::Refused,
        };
        // 2. The per-function scratch: a SEPARATE heap allocation (`Box<[Value]>` — §13-2; see
        //    `CompiledWasmFn::scratch` for the full address-stability argument and why inline
        //    storage is forbidden). `words == 0` (call-free function) makes an empty Box: the
        //    dangling-but-aligned address mined below is then never baked into any store and
        //    never read by any callee (the backend's documented zero-words contract).
        let mut scratch: Box<[Value]> = vec![0; words].into_boxed_slice();
        // 3. Mine the address ONCE, immediately after allocation, via `as_mut_ptr()` — WRITE
        //    provenance (the emitted code stores through it), exposed here and never re-derived
        //    (§13-5). No Rust reference into this buffer is formed from here on, except the two
        //    momentary copy windows (`module_dispatch`'s `to_vec` slice and `tail_signal`'s
        //    slice→`extend_from_slice` — both guest-execution-free; see `CompiledWasmFn::scratch`).
        let scratch_addr = scratch.as_mut_ptr() as usize as u32;
        // 4. Lower. Helper indices are the baked fn pointers themselves and the host-fn resolver
        //    is the checked identity (`ptr as u32` — §2.3 #3: on the wasm deployment an IR-carried
        //    fn pointer already is its table index; `try_from` only rejects what can't exist here).
        //
        //    O-17 L0 (passive shared-memory compatibility, design §5.2 v1.1 + the O-17 note): the
        //    emitted module's `otter.memory` import must match THIS build's own linear memory
        //    exactly on sharedness (limits subtyping never coerces shared↔unshared — O-17 M5).
        //    `cfg!(target_feature = "atomics")` is the COMPILE-TIME self-knowledge signal: the §7
        //    atomics recipe builds the whole guest (this very crate included) with `+atomics` and
        //    a `--shared-memory` link, so the flag and the memory's sharedness travel together in
        //    every supported §7 configuration. Known blind spots (recorded honestly in the O-17
        //    design note, with their unblock conditions): a shared-memory guest built WITHOUT
        //    +atomics (the V3 recipe) under-detects, an +atomics guest linked non-shared (V1)
        //    over-detects — BOTH degrade through the host's exact-sharedness import check into
        //    `-2` → sticky `Refused` → permanent T0 (§5.1): a tier-up loss, never a miscompile.
        let opts = otter_wasmemit::EmitOpts::new().with_shared_memory(cfg!(target_feature = "atomics"));
        let bytes = match otter_wasmemit::compile_dispatch_fn_with(
            &self.isa,
            idx,
            f,
            self.module.fill,
            self.module.host_ctx,
            Self::wasm_helper_idx(),
            &|p| u32::try_from(p).ok(),
            // O-19 C1: the opts-route `scratch_addr` is u64 now (the backend's addrtype
            // widening); this JIT surface stays A32 (§2.1), so the upcast is width-only.
            u64::from(scratch_addr),
            opts,
        ) {
            Ok(b) => b,
            Err(_) => return WasmSlot::Refused,
        };
        // 5. Hand the bytes to the host (§5.1). SAFETY: plain scalar-argument FFI describing a
        //    live buffer the host only READS (§5.3-7: `compile` must not write guest memory) and
        //    during which no guest code re-enters (§5.2 start-section ban + §13-4 export-call
        //    ban), so `bytes` is stable and unaliased for the whole call.
        let raw = unsafe { otter_jit::compile(bytes.as_ptr() as usize as u32, bytes.len() as u32) };
        // 6. Engine-side defense (§5.1): install ONLY `0 < idx <= u32::MAX`. Every error code
        //    (negative), the contract-forbidden 0 (the null funcref slot — a buggy glue returning
        //    it must not become a null-funcref trap on first call), and anything past u32::MAX
        //    all collapse into the same sticky `Refused` — never a panic (O-8 fix T9).
        match u32::try_from(raw) {
            Ok(table_idx) if table_idx > 0 => WasmSlot::Compiled(CompiledWasmFn { table_idx, scratch }),
            _ => WasmSlot::Refused,
        }
    }

    fn call_compiled(&self, p: *const u8, args: &[Value]) -> Value {
        // SAFETY: `p` was compiled with the `CompiledFn` ABI by a backend; its owning buffer is kept
        // alive in `self` (add-only); `self` outlives this call.
        let f: CompiledFn = unsafe { core::mem::transmute(p) };
        let argp = if args.is_empty() {
            ptr::null()
        } else {
            args.as_ptr()
        };
        f(self as *const ModuleEngine, argp, args.len() as i64)
    }

    // O-13 (design §6, B#8b): like `t2_threshold`'s attribute above, this predicate predates the
    // wasm-emit tier ("no tier compiled in ⇒ `idx` is unused"); the wasm-emit arm keeps the allow
    // OFF on wasm32+wasm-emit builds, where `idx` is used again.
    #[cfg_attr(
        not(any(
            all(not(target_family = "wasm"), not(target_os = "none")),
            all(target_arch = "x86_64", not(target_os = "none")),
            all(target_arch = "wasm32", feature = "wasm-emit")
        )),
        allow(unused_variables)
    )]
    fn maybe_tier_up(&self, idx: usize) {
        let base = self.base();
        let c = if idx < base {
            self.counts[idx].get()
        } else {
            match self.pushed_counts.borrow().get(idx - base) {
                Some(cell) => cell.get(),
                // Out-of-range pushed idx: `dispatch`'s loop-head `in_range` check already returns `fill`
                // for this idx before `maybe_tier_up` is ever reached, so this is unreachable in normal
                // operation — kept as a defensive no-op (never a panic/unwrap) rather than relying on that.
                None => return,
            }
        };
        // O-13 (design §6): the wasm-emit tier. Its cfg is disjoint from BOTH native branches below
        // (their predicates exclude wasm), so on wasm32+wasm-emit this is the only compile branch —
        // wasm-emit is the sole compiled tier there. It reuses `t2_threshold` (the compile+host
        // round-trip is T2-class work; `t1_threshold` stays unused on wasm) and acts only on an
        // `Untried` slot: the attempt is one-shot, `Untried` → (`Compiled` | `Refused`), and
        // `Refused` is sticky (§5.1 — no retry loop). Mirrors the cranelift branch's shape (outer
        // condition, inner `func_ref`, unconditional `return` once the outer condition holds) so a
        // should-be-unreachable `func_ref == None` can never fall through into another branch —
        // pure discipline here (nothing below survives this cfg), not reachability. The `return`
        // is therefore a needless_return on exactly this build — kept anyway, with the lint
        // waived, because it mirrors the cranelift branch's shape and closes the fall-through
        // surface the moment a later increment adds another branch below (W-3 review lens B #1:
        // W-4's wasm clippy lane would otherwise trip -D warnings on it).
        #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
        #[allow(clippy::needless_return)]
        if c >= self.t2_threshold && self.wasm_untried(idx) {
            if let Some(f) = self.func_ref(idx) {
                let slot = self.compile_wasm(idx, f);
                self.install_wasm(idx, slot);
            }
            return;
        }
        // Cranelift (T2) takes priority: if promoted this call, don't also compile the baseline (T1). Each
        // branch is present only where its tier is compiled in for this target. O-8c-2: both tiers lower
        // `TailCall` (via the `tail_signal` trampoline signal), so no per-function eligibility gate remains
        // — every function, `TailCall`-containing or not, tiers up identically. `compile_dispatch_fn` (both
        // backends, since the O-9 signature refactor) takes the `FuncDef`/`fill` directly rather than a
        // `&Module`, so the SAME call shape compiles an original OR a pushed function — only which storage
        // `func_ref`/`install_cranelift`/`install_baseline` touch differs (O-9 design doc §1.1).
        #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
        #[allow(clippy::collapsible_if)]
        // NOT equivalent to an `&& let` chain: the `return` below must
        // fire unconditionally once the outer condition holds, even in the (should-be-unreachable) case
        // `func_ref` returns `None` — collapsing would instead fall through to try the baseline branch in
        // that case, silently changing the "cranelift attempted this call ⇒ never also try baseline this
        // same call" priority the two branches are written to preserve.
        if c >= self.t2_threshold && self.cranelift_ptr(idx).is_none() {
            if let Some(f) = self.func_ref(idx) {
                let code = otter_cranelift::compile_dispatch_fn(
                    &self.isa,
                    idx,
                    f,
                    self.module.fill,
                    self.module.host_ctx,
                    dispatch_addr(),
                    tail_signal_addr(),
                );
                self.install_cranelift(idx, code);
            }
            return;
        }
        // (No trailing statement after this `if` — it's the tail of the function — so, unlike the
        // cranelift branch above, collapsing into a single `&&`-chained condition changes nothing: whether
        // `func_ref` succeeds or not, falling out of this `if` either way just ends the function.)
        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        if c >= self.t1_threshold
            && self.baseline_ptr(idx).is_none()
            && let Some(f) = self.func_ref(idx)
        {
            let code = otter_baseline::compile_dispatch_fn(
                &self.isa,
                idx,
                f,
                self.module.fill,
                self.module.host_ctx,
                dispatch_addr(),
                tail_signal_addr(),
            );
            self.install_baseline(idx, code);
        }
    }

    /// Install newly-compiled Cranelift code for `idx`, in either index space. Single `borrow_mut()` per
    /// branch (no borrow held across any other call), same discipline as the pre-O-9 original-only code.
    #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
    fn install_cranelift(&self, idx: usize, code: otter_cranelift::CompiledDispatchFn) {
        let base = self.base();
        if idx < base {
            *self.cranelift[idx].borrow_mut() = Some(code);
            return;
        }
        if let Some(slot) = self.pushed_cranelift.borrow().get(idx - base) {
            *slot.borrow_mut() = Some(code);
        }
    }

    /// Install newly-compiled baseline code for `idx`, in either index space. See [`Self::install_cranelift`].
    #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
    fn install_baseline(&self, idx: usize, code: otter_baseline::CompiledDispatchFn) {
        let base = self.base();
        if idx < base {
            *self.baseline[idx].borrow_mut() = Some(code);
            return;
        }
        if let Some(slot) = self.pushed_baseline.borrow().get(idx - base) {
            *slot.borrow_mut() = Some(code);
        }
    }

    /// Inject a new function into this LIVE engine and return its index (REPL/eval use case: e.g. a JS
    /// `eval()` builtin or a Scheme REPL compiles a fresh top-level definition and hands it to an engine
    /// that may already be mid-`run`). Extends this crate's add-only invariant (module doc) to the
    /// function table itself: `f` is appended, never replacing or invalidating any existing index — a
    /// previously returned index (original or previously pushed) keeps addressing the exact same
    /// function, at whatever tier it has reached, for the rest of the engine's life. **This is a hard
    /// invariant, not just documentation** — see [`Self::pushed`]'s field doc for what breaks (silent UB,
    /// not a panic) if it is ever violated.
    ///
    /// Returns the new function's index; pass it as the `callee` operand of a `Call`/`TailCall` (the
    /// operand is a plain runtime value, so existing code that computes its callee dynamically can invoke
    /// a function pushed after that code was compiled), or to introspection ([`Self::tier_of`],
    /// [`Self::calls`]). Indices are dense and strictly increasing: the k-th (0-based, counting from this
    /// engine's first `push_func` call) call returns `original_module.funcs.len() + k`.
    ///
    /// **Reentrant.** Safe to call from anywhere, including from deep inside an in-flight `run`/`dispatch`
    /// call chain (e.g. a future host callback that runs while otter code is executing and injects a new
    /// definition) — like every other method here it takes only `&self` and cannot conflict with a nested
    /// call into the same engine (see the module doc's reentrancy-safety section, which this extends).
    /// Internally this acquires EVERY backing `RefCell` (up to five: `pushed`/`pushed_counts`/
    /// (target-gated) `pushed_baseline`/`pushed_cranelift`/`pushed_wasm` — the last is the O-13
    /// wasm-emit tier's fifth participant in this same discipline, design §6 B#8c) BEFORE mutating
    /// any of them, so a hypothetical future
    /// double-borrow panic on one of them (which would require the borrow discipline documented throughout
    /// this file to already be broken elsewhere) can only happen before any append — never leaving the
    /// stores misaligned with each other (O-9 panel finding: a naive "acquire, mutate, acquire, mutate, …"
    /// sequence could otherwise leave e.g. `pushed` one longer than `pushed_counts`, surfacing as a
    /// confusing OOB panic much later, on an unrelated dispatch of a subsequent index). This function calls
    /// nothing that could reenter the engine while any of these guards are held (no callback, no compiled
    /// code), so holding them all at once introduces no new reentrancy risk of its own.
    ///
    /// `f`'s code is not validated against `self`'s [`Isa`] here — same as every function `ModuleEngine::new`
    /// accepts, malformed code is only ever caught lazily, the first time `f` is actually dispatched, via the
    /// existing trap mechanism ([`Self::trapped`]). **If `f` comes from a source you did not fully control
    /// the construction of**, call `otter::validate::check_func(&isa, &f)` before pushing it and reject on
    /// `Err` — see this crate's module doc's "Known gaps" section (O-12) for what lazy trapping does and
    /// does not cover; it is opt-in and this function does not call it for you. `push_func` itself never
    /// panics under normal operation
    /// (short of the process-wide allocator aborting on OOM, same risk class as any other `Vec::push` in
    /// this codebase — not a new failure mode) **provided the `RefCell` borrow discipline documented
    /// throughout this file continues to hold** — that is an invariant this crate maintains by construction
    /// and code review, not something the type system enforces, so it is not an unconditional guarantee.
    /// This function is a plain `&self` method with no `catch_unwind` of its own (only the JIT-boundary
    /// entry points, [`module_dispatch`]/[`tail_signal`], catch panics) — a panic that DID escape here would
    /// propagate as an ordinary Rust unwind (not a process abort, since this crate requires `panic =
    /// "unwind"` — see the module doc) directly to `push_func`'s own caller. An embedder that must survive a
    /// single failed call without losing the whole engine should wrap its own call site in
    /// [`std::panic::catch_unwind`].
    ///
    /// It succeeds even on an already-[`trapped`](Self::trapped) engine (it is a pure data append); the
    /// pushed function simply won't run until the engine is un-poisoned via [`Self::clear_trap`] (O-10).
    /// Combined with [`Self::try_dispatch`] (the first public API able to invoke an arbitrary — including
    /// freshly pushed — index directly), this is what lets a REPL-style embedder push a new top-level
    /// definition and call it immediately, even from inside the error-handling path of a previous trap.
    pub fn push_func(&self, f: FuncDef) -> usize {
        // Acquire every RefCell this touches BEFORE mutating any of them — see this fn's doc for why.
        let mut pushed = self.pushed.borrow_mut();
        let mut counts = self.pushed_counts.borrow_mut();
        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        let mut baseline = self.pushed_baseline.borrow_mut();
        #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
        let mut cranelift = self.pushed_cranelift.borrow_mut();
        #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
        let mut wasm = self.pushed_wasm.borrow_mut();

        let idx = self.base() + pushed.len();
        pushed.push(Box::new(f));
        counts.push(Cell::new(0));
        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        baseline.push(RefCell::new(None));
        #[cfg(all(not(target_family = "wasm"), not(target_os = "none")))]
        cranelift.push(RefCell::new(None));
        #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
        wasm.push(RefCell::new(WasmSlot::Untried));
        idx
    }

    /// Like the internal `dispatch`/[`Self::run`], but makes a trap OBSERVABLE and DISTINGUISHABLE from a
    /// normal return value — including a normal value that happens to equal [`Module::fill`] (an
    /// out-of-range callee index is a pre-existing, DEFINED fallback that returns `fill` as data, not a
    /// trap; see the three cases below) or a value produced by a compiled frame that did further arithmetic
    /// on a nested trap's `fill` result before returning it (this can happen on the [`module_dispatch`]
    /// path just as much as the [`tail_signal`] path — either way the outer return value is not a reliable
    /// discriminator, which is exactly why this method never looks at it).
    ///
    /// Distinguishes exactly three cases, based on [`Self::trapped`]'s state before/after the call — NEVER
    /// on the returned value itself:
    /// - **Already trapped** when called: `dispatch`'s own loop-head check short-circuits to `fill`
    ///   immediately, without doing anything else. Returns `Err(Trap { idx })` where `idx` is the site of
    ///   the ORIGINAL trap (not this call's `idx`), recorded when that original trap was caught.
    /// - **Newly trapped during this call**: either a nested compiled call panicked and was caught at the
    ///   [`module_dispatch`]/[`tail_signal`] FFI boundary, or the panic never crossed any such boundary at
    ///   all (e.g. it happened directly in `dispatch`'s own loop-head bookkeeping, in a first-ever tier-up
    ///   compile, or in a pure-T0-interpreted recursive call tree with no compiled frame anywhere on the
    ///   stack) and is instead caught by THIS method's own boundary around the whole call (see below).
    ///   Either way, returns `Err(Trap { idx })` where `idx` is the callee in flight at whichever boundary
    ///   actually caught it.
    /// - **Out-of-range callee** (existing, defined behavior, unchanged): `trapped` is never touched.
    ///   Returns `Ok(fill)`, exactly like [`Self::run`]/`dispatch` already do for this case — including when
    ///   the out-of-range index is in the O-9 pushed segment.
    ///
    /// Unlike `dispatch`/[`Self::run`], this method wraps its OWN call to `dispatch` in
    /// [`std::panic::catch_unwind`] — the SAME containment [`module_dispatch`]/[`tail_signal`] already give
    /// nested calls, extended to the outermost call too, so a `try_dispatch`/[`Self::try_run`] call can
    /// never itself escape as a raw Rust panic for a reason internal to this engine. This closes a gap
    /// `dispatch`/`run` still have: a panic that never crosses the `module_dispatch`/`tail_signal` FFI
    /// boundary — because there was no compiled frame on the stack yet, e.g. dispatching a function for the
    /// very first time, which is exactly [`Self::push_func`]'s REPL use case ("push, then immediately call")
    /// — would otherwise propagate straight out as an ordinary panic instead of `Err(Trap)`. The only way a
    /// Rust panic still escapes `try_dispatch`/`try_run` is a bug in this method's own glue code outside
    /// that `catch_unwind`, or a second panic during unwind (a Rust-level abort regardless of this crate).
    ///
    /// **wasm-emit builds — the wasm-TRAP contract (O-13 design §2.4, normative)**: everything above is
    /// about Rust PANICS, which the §7 build configuration (`-Cpanic=unwind` + wasm EH) makes catchable
    /// in-wasm — `Err(Trap)` + [`Self::clear_trap`] work there exactly as natively. A genuine **wasm
    /// trap** (e.g. a `call_indirect` signature mismatch from a backend bug, or `unreachable`) is a
    /// different beast: it unwinds the whole wasm activation WITHOUT running Rust `Drop`s, so any
    /// `RefCell` guard live at that moment is never released and the engine's borrow discipline is
    /// permanently broken. Normatively: *after a wasm trap, this engine (in practice the whole wasm
    /// instance) is discard-only; `clear_trap` and any re-entry are UNDEFINED* — the host must catch the
    /// trap and end the session (activation death + loss of state integrity, not a memory-safety
    /// violation). `clear_trap`'s "RAII released everything" guarantee explicitly does NOT extend to
    /// wasm traps — it presumes a Rust unwind.
    pub fn try_dispatch(&self, idx: usize, args: &[Value]) -> Result<Value, Trap> {
        match catch_unwind(AssertUnwindSafe(|| self.dispatch(idx, args))) {
            Ok(v) => {
                if self.trapped.get() {
                    Err(Trap {
                        idx: self.trapped_idx.get().unwrap_or(idx),
                    })
                } else {
                    Ok(v)
                }
            }
            Err(_) => {
                // The panic never reached module_dispatch/tail_signal's own catch_unwind (no compiled
                // frame was on the stack when it happened) — contain it here instead, exactly as those two
                // do, so this boundary gives the outermost call the SAME "never abort" guarantee nested
                // calls already had.
                self.trapped.set(true);
                self.trapped_idx.set(Some(idx));
                Err(Trap { idx })
            }
        }
    }

    /// [`Self::try_dispatch`] on [`Module::entry`] — the `try_*` counterpart to [`Self::run`], exactly as
    /// `run` is `dispatch` on `Module::entry`.
    pub fn try_run(&self, args: &[Value]) -> Result<Value, Trap> {
        self.try_dispatch(self.module.entry, args)
    }

    /// Un-poison a [`trapped`](Self::trapped) engine so it can keep running after a caught FFI-boundary
    /// panic, instead of staying permanently poisoned for the rest of the process — this is what lets a
    /// guest-level recoverable error (e.g. a condition raised through code that, several layers down,
    /// tripped this engine's own trap mechanism) be handled by the embedder without losing the whole
    /// session.
    ///
    /// **What this guarantees**: after `clear_trap`, [`Self::trapped`] is `false`, and every
    /// `ModuleEngine`-visible field OTHER than the one whose call was in flight when the trap was caught is
    /// completely unaffected — same tiers, same compiled code, same call counts, same results, exactly as
    /// if the trap had never happened (this crate's add-only invariants mean a trap never touches per-idx
    /// storage for any idx other than the one in flight). This is safe because every `RefCell` this crate
    /// touches is released via ordinary RAII `Drop` even when a panic unwinds through the frame holding it
    /// (a Rust language guarantee, not something re-derived here), and the three places that ever set
    /// `trapped` ([`module_dispatch`], [`tail_signal`], and [`Self::try_dispatch`]'s own boundary) only do
    /// so AFTER that unwind has already fully completed — so by the time `trapped` is observably `true`,
    /// there is no live borrow, no half-written tail-call signal, and no partially-appended
    /// [`Self::push_func`] state left anywhere in this engine.
    ///
    /// This guarantee is scoped to `ModuleEngine`'s OWN fields. It says nothing, by itself, about a
    /// resource a compiled backend owns OUTSIDE this struct — e.g. an executable memory mapping a tier-up
    /// compile was in the middle of creating when it panicked. Keeping that RAII-safe across a
    /// mid-construction panic is each backend's own responsibility; see `otter-baseline`'s `ExecBuf::map`
    /// for the concrete case this crate depends on (its executable buffer is constructed, and therefore
    /// droppable/unmappable on unwind, BEFORE the fallible `mprotect` call that can panic).
    ///
    /// **What this does NOT guarantee**: it does not fix the underlying cause. If the trap was caused by a
    /// deterministic condition — malformed bytecode in a specific function, or an internal compiler bug
    /// triggered by that function's shape — calling that SAME function again after `clear_trap` is expected
    /// to trap again, identically and reproducibly (this is a poison-flag reset, not a retry/backoff/
    /// blacklist mechanism; see [`Trap::idx`] for the diagnostic needed to build one at the embedder level).
    /// In particular, call counters are strictly add-only (`saturating_add`, never decremented by
    /// `clear_trap` or anything else — see [`Self::calls`]) and Cranelift (T2) tier-up unconditionally takes
    /// priority over baseline (T1) once a function's count crosses the T2 threshold (see the private
    /// `maybe_tier_up`) — so a function whose T2 compile panics deterministically will keep retrying ONLY
    /// the T2 compile on every subsequent call, forever; it never gets a chance to fall back to T1.
    pub fn clear_trap(&self) {
        self.trapped.set(false);
        self.trapped_idx.set(None);
        self.tail_pending.set(false);
    }
}

/// Returned by [`ModuleEngine::try_dispatch`]/[`ModuleEngine::try_run`] when the engine is (or just became,
/// during this very call) [`trapped`](ModuleEngine::trapped) — i.e. this is a caught FFI-boundary panic
/// (malformed bytecode, or an internal bug in a compiled tier), NEVER a guest-level value. An out-of-range
/// callee index is NOT a trap (it is an existing, defined fallback that returns [`Module::fill`] as an
/// ordinary `Ok` value) — see [`ModuleEngine::try_dispatch`]'s doc for the full trap-vs-value contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trap {
    /// The function index that was being dispatched, at the nearest enclosing panic-catching boundary
    /// ([`module_dispatch`], [`tail_signal`], or [`ModuleEngine::try_dispatch`]'s own boundary around the
    /// whole call), at the moment the underlying panic was caught. This may be deeper in the call tree than
    /// the `idx` passed to `try_dispatch`/[`ModuleEngine::try_run`] — but ONLY if the panic happened inside
    /// a nested call that itself crossed a [`module_dispatch`]/[`tail_signal`] FFI boundary (i.e. the nested
    /// callee was a compiled function). A nested call that stays entirely within the T0 interpreter — no
    /// compiled frame anywhere on the stack, however deep the recursion — crosses no such boundary, so
    /// nothing records its idx before the panic reaches `try_dispatch`'s own outer boundary; in that case
    /// `idx` is the OUTERMOST index (the one originally passed to `try_dispatch`/`try_run`), not the actual
    /// fault site. Either way this is a diagnostic hint (the nearest available site), not a full backtrace.
    pub idx: usize,
}

/// The dispatcher baked into compiled code: `dispatch(ctx, callee_idx, args, argc)`. Reconstitutes the
/// engine from `ctx` (a shared reference — never `&mut`) and forwards to [`ModuleEngine::dispatch`].
///
/// O-13: on wasm-emit builds ONLY, the body is the §2.5-2 **copy-at-once** variant (§13-1) — the
/// args are copied into an owned `Vec` *inside* the `catch_unwind` closure, before `dispatch`
/// runs; see the second cfg block below. The native body is semantically unchanged from the
/// pre-O-13 one (only the shared `idx` conversion is hoisted above the cfg split).
///
/// O-14 §3.2a (A-C1): the callee word is converted FULL-WIDTH via [`otter::interp::callee_index`]
/// — the u64 width check precedes any narrowing cast, so `dispatch`'s loop-head range check (the
/// existing fail-closed `fill` fallback) covers the whole 64-bit OOB space on every target width.
/// The old bare `as usize` truncated to the low 32 bits on a wasm32 build BEFORE that check, so a
/// word `x >= 2^32` with `x mod 2^32 < nfuncs` silently called `f_{x mod 2^32}` there. Native
/// (usize = 64) behavior is bit-for-bit unchanged.
extern "C" fn module_dispatch(eng: *const ModuleEngine, idx: i64, args: *const Value, argc: i64) -> Value {
    // SAFETY: `eng` is the `ctx` the engine itself passed into compiled code (its own `&self` address),
    // valid and immovable for the whole run; only shared `&ModuleEngine` is ever formed.
    let engine = unsafe { &*eng };
    // Pure integer conversion (fn doc above) — panic-free, so it is fine OUTSIDE the catch_unwind
    // wrappers below (the "guards outside" discipline both cfg arms follow).
    let idx = otter::interp::callee_index(idx);

    // Native path — UNCHANGED by O-13 (the caller's args buffer is a native stack slot per
    // activation, so the long-lived borrow below is sound there; only the wasm tier's shared
    // per-function scratch needs the copy variant — design §2.5/§6).
    #[cfg(not(all(target_arch = "wasm32", feature = "wasm-emit")))]
    {
        // `argc <= 0` (incl. a corrupt negative count) ⇒ empty; only then is `args` even read.
        let args: &[Value] = if argc <= 0 || args.is_null() {
            &[]
        } else {
            // SAFETY: compiled code passes a pointer to `argc` (> 0) contiguous arg words in its own frame.
            unsafe { core::slice::from_raw_parts(args, argc as usize) }
        };
        // Panic-safety: a panic must NOT unwind across this `extern "C"` frame (that is a non-unwinding
        // panic → process abort = a DoS from malformed bytecode). Catch it, poison the engine, and return
        // a defined `fill` so the trap is contained and consistent rather than aborting the process.
        match catch_unwind(AssertUnwindSafe(|| engine.dispatch(idx, args))) {
            Ok(v) => v,
            Err(_) => {
                engine.trapped.set(true);
                engine.trapped_idx.set(Some(idx));
                engine.module.fill
            }
        }
    }

    // O-13 wasm-emit path — the §2.5-2 **copy-at-once** variant, §13-1 to the letter ([`tail_signal`]
    // below is the stated model): only the panic-free guard computation stays outside; slice
    // formation + the owned-`Vec` copy + the `dispatch(&vec)` call ALL run inside the
    // `catch_unwind` closure. Putting the copy outside would put its panic surface (`Vec` growth/
    // capacity arithmetic) beyond every catch — this `extern "C"` frame is nounwind, so that panic
    // would abort → an uncatchable wasm trap, reopening the hole O-12 closed at the single
    // busiest JIT entry point. (The allocator's own OOM abort — `handle_alloc_error` — remains
    // uncatchable on every path; same pre-existing residual class as `dispatch`'s `owned`/T0's
    // `argv` allocations, documented, not fixed — §13-1 endnote.)
    //
    // WHY copy at all — the borrow-lifetime payload (§2.5-2/§13-7; NOT observable by any
    // differential test, which is why review must check this window by hand — W-2 panel #3 B#1):
    // `args` points into the CALLING emitted function's per-function scratch
    // ([`CompiledWasmFn::scratch`]), which every nested activation of that same function REUSES.
    // The native body above holds its `&[Value]` across all of `dispatch` — here that borrow
    // would be live while a nested callee re-enters the same emitted caller and stores into the
    // same scratch: a write under a live shared borrow, violating the noalias obligation rustc
    // attaches to `&[Value]` (LLVM-level UB). The copy shrinks the live-borrow window to the
    // `to_vec` line itself, and **no other code runs inside that window**: the window is one
    // slice→Vec `memcpy` plus its allocation — no guest bytecode, no compiled code, no host
    // import, no engine callback can interleave (the only Rust running is `to_vec`; the allocator
    // call inside it never touches live allocations — W-0 M5). From then on `dispatch` borrows
    // only the engine-side owned `vec`, so both halves of the §13-7 twin invariant hold: (i) the
    // emitted code's only memory writes land in its own scratch, and (ii) every buffer a live
    // Rust borrow refers to is allocation-disjoint from every scratch.
    #[cfg(all(target_arch = "wasm32", feature = "wasm-emit"))]
    {
        // Guards only (§13-1: "guards outside") — pure integer/null tests, no panic surface.
        let empty = argc <= 0 || args.is_null();
        match catch_unwind(AssertUnwindSafe(|| {
            let vec: Vec<Value> = if empty {
                Vec::new()
            } else {
                // SAFETY: same contract as the native path — compiled code passes a pointer to
                // `argc` (> 0) contiguous arg words (here: into its own baked scratch, which
                // stays untouched for the whole copy — nothing else executes in this window,
                // see the block comment above). The borrow ends with this expression.
                unsafe { core::slice::from_raw_parts(args, argc as usize) }.to_vec()
            };
            engine.dispatch(idx, &vec)
        })) {
            Ok(v) => v,
            Err(_) => {
                // Err ⇒ poison (§13-1), exactly as the native path and `tail_signal` do.
                engine.trapped.set(true);
                engine.trapped_idx.set(Some(idx));
                engine.module.fill
            }
        }
    }
}

/// The baked helper compiled `TailCall`s call (O-8c, both compiled tiers — T2 Cranelift since O-8c-1, T1
/// baseline since O-8c-2): marshal `(callee, args)` into the engine's tail-request scratch and return
/// immediately — compiled code drops straight into its epilogue right after this call (pure machine code,
/// no Rust in between, on either backend), so [`ModuleEngine::dispatch`]'s trampoline loop picks the
/// request up the instant the compiled call returns. Same ABI as
/// [`module_dispatch`] (`DispatchPtr`) — no new fn-pointer type needed, and no `dest`/return-value
/// threading: the return value is discarded by the caller (`tail_pending`, not the value, is the
/// discriminator — mirrors how `module_dispatch`'s callers never confuse a real result with anything
/// else). See design §8.1/§8.3 and the module doc's reentrancy-safety section.
extern "C" fn tail_signal(eng: *const ModuleEngine, callee: i64, args: *const Value, argc: i64) -> Value {
    // SAFETY: same as `module_dispatch` — `eng` is the engine's own `&self` address, forwarded untouched
    // by compiled code; only a shared `&ModuleEngine` is ever formed.
    let engine = unsafe { &*eng };
    // `argc <= 0` (incl. a corrupt negative count) || `args.is_null()` ⇒ empty slice — same defensive
    // pattern as `module_dispatch` (design §8.8 pick C4: a bare `argc as usize` cast on a negative value
    // wraps to a huge length, so the negative case must be excluded from the cast, not just `== 0`).
    let slice: &[Value] = if argc <= 0 || args.is_null() {
        &[]
    } else {
        // SAFETY: compiled code passes a pointer to `argc` (> 0) contiguous arg words. On the native
        // tiers that buffer is a stack slot in the caller's own still-live frame (the call has not
        // returned yet). On the wasm-emit tier (O-13) it is NOT a frame: it is the calling emitted
        // function's per-function SCRATCH ([`CompiledWasmFn::scratch`], a linear-memory heap buffer
        // shared by every activation of that function) — still valid for exactly this window, because
        // nothing executes between the emitted code's argument stores and this copy (the borrow ends
        // inside this closure, before any guest/compiled code can run again — design §2.5/§13-7).
        unsafe { core::slice::from_raw_parts(args, argc as usize) }
    };
    // Panic-safety (design §8.8 pick U1, post-panel-review hardening): this is a second direct-JIT-callable
    // `extern "C"` entry point, exactly like `module_dispatch` above, and needs the SAME `catch_unwind`
    // wrapper — the module doc's "a trap can never abort the process" guarantee is per-entry-point, not
    // per-function, and `tail_signal` is called directly from raw JIT'd machine code (never through
    // `module_dispatch`), so without this an unwind here would abort at the (non-"C-unwind") ABI boundary.
    // The body is a leaf (no reentrancy). Note `argc` is NOT `u8`-bounded (an earlier comment here
    // claimed that — O-13 design §2.5 flagged it as a misread: the `Call`/`TailCall` `argc: u8` field is
    // an operand-slot INDEX, not the count; the count itself is `inst.operand(argc_op)`, bounded only by
    // the caller's `base+1+argc <= nlocals` check in `otter::validate::check_func`, i.e. up to
    // `MAX_SAFE_NLOCALS` ≈ 67.1M words). That said, no CURRENTLY-reachable panic path exists in this
    // body: `extend_from_slice`'s capacity-overflow panic needs a request past `isize::MAX` bytes, and
    // the argc ceiling caps the copy at ~537 MB (~1.07 GB transiently under doubling growth) — while a
    // genuine allocation FAILURE goes through `handle_alloc_error` (abort / wasm trap, not an unwind),
    // which no catch can contain. The wrapper's justification is exactly §8.8 U1: seal the
    // non-"C-unwind" ABI boundary against ANY panic a future edit to this body might introduce, and
    // mirror `module_dispatch` so such edits inherit the containment for free.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // Copy while the compiled frame is still live (the call hasn't returned), THEN signal — never the
        // other order (design risk §8.6#1).
        let mut buf = engine.tail_args.borrow_mut();
        buf.clear();
        buf.extend_from_slice(slice);
        drop(buf); // release the borrow before the trampoline's own borrow (dispatch()'s swap) runs
        // O-14 §3.2a (A-C1): full-width conversion (`module_dispatch`'s doc) — the trampoline's
        // loop-head range check consumes this on its NEXT iteration, so the width check must
        // happen HERE, before the word is narrowed to a usize cell; a wasm32 build otherwise
        // truncated an out-of-range word into range. Native behavior is bit-for-bit unchanged.
        engine.tail_callee.set(otter::interp::callee_index(callee));
        engine.tail_pending.set(true); // set LAST: this is the discriminant dispatch() checks post-call
    }));
    if outcome.is_err() {
        // A panic here means `tail_pending.set(true)` did NOT run (it's the last statement), so the
        // trampoline's post-call `!tail_pending.get()` check sees `false` and returns this `fill` as a
        // plain value — poisoning still takes effect on the engine's NEXT dispatch (checked at the top of
        // the loop), exactly as an ordinary caught panic in `module_dispatch` already behaves.
        engine.trapped.set(true);
        // Same full-width conversion for the diagnostic hint (A-C1) — `Trap::idx` must name the
        // same index the range check would have seen, not a width-dependent truncation of it.
        engine.trapped_idx.set(Some(otter::interp::callee_index(callee)));
    }
    engine.module.fill // discarded by the caller; only `tail_pending` discriminates tail-vs-value
}

#[cfg(test)]
mod tests {
    use super::*;
    use otter::interp::run_module;
    use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, NumTy, OpDef, OpHints};
    use std::boxed::Box;
    use std::vec;

    // a minimal ISA with Call/Return (same shape as otter's internal call_isa)
    fn call_isa() -> Isa {
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
            effects: vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::Bin(
                    BinOp::new(NumTy::I64, op),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::LocalAt(2)),
                ),
            }],
            hints: pure,
        };
        Isa {
            ops: vec![
                OpDef {
                    name: "mov_imm",
                    effects: vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::OperandImm(1),
                    }],
                    hints: pure,
                }, // 0
                alu("mul", ArithOp::Mul), // 1
                alu("sub", ArithOp::Sub), // 2
                alu("lt", ArithOp::Lt),   // 3
                OpDef {
                    name: "jump_if",
                    effects: vec![Effect::JumpIf {
                        cond: Expr::LocalAt(0),
                        target: 1,
                    }],
                    hints: term,
                }, // 4
                OpDef {
                    name: "copy",
                    effects: vec![Effect::SetLocalAt {
                        reg: 0,
                        value: Expr::LocalAt(1),
                    }],
                    hints: pure,
                }, // 5
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
                }, // 6
                OpDef {
                    name: "ret",
                    effects: vec![Effect::Return {
                        value: Expr::LocalAt(0),
                    }],
                    hints: term,
                }, // 7
                OpDef {
                    name: "halt",
                    effects: vec![Effect::Halt {
                        value: Expr::LocalAt(0),
                    }],
                    hints: term,
                }, // 8
            ],
        }
    }

    const MOV: u16 = 0;
    const MUL_: u16 = 1;
    const SUB_: u16 = 2;
    const LT_: u16 = 3;
    const JIF: u16 = 4;
    const CPY: u16 = 5;
    const CALL: u16 = 6;
    const RET: u16 = 7;
    const HALT_: u16 = 8;

    // main() = fact(arg0);  fact(n) = n<2 ? 1 : n*fact(n-1)
    fn fact_module() -> Module {
        let fact = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 10,
            code: vec![
                Inst::new(MOV, 1, 2, 0),
                Inst::new(LT_, 2, 0, 1),
                Inst::new(JIF, 2, 10, 0),
                Inst::new(MOV, 3, 1, 0),
                Inst::new(SUB_, 4, 0, 3),
                Inst::new(MOV, 5, 1, 0),
                Inst::new(CPY, 6, 4, 0),
                Inst::new(CALL, 7, 5, 1),
                Inst::new(MUL_, 8, 0, 7),
                Inst::new(RET, 8, 0, 0),
                Inst::new(MOV, 9, 1, 0),
                Inst::new(RET, 9, 0, 0),
            ],
        };
        // main passes its arg0 straight to fact
        let main = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 3,
            code: vec![
                Inst::new(MOV, 0, 1, 0),  // callee = func 1 (fact)
                Inst::new(CPY, 1, 0, 0),  // arg0 = n   (main's param at local0... see note)
                Inst::new(CALL, 2, 0, 1), // res = fact(n)
                Inst::new(HALT_, 2, 0, 0),
            ],
        };
        Module {
            funcs: vec![main, fact],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        }
    }

    #[test]
    fn engine_matches_oracle_across_tiers() {
        let isa = call_isa();
        let module = fact_module();
        // main has nparams=1 (n). But its code MOVs callee into local0, overwriting the param. Rebuild
        // main to read the param before clobbering: keep n in a high local.
        // (Simpler: drive fact directly as entry.)
        let module = Module { entry: 1, ..module };
        let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 5);
        // call enough times to climb T0 → T1 → T2, checking the result every time
        for k in 0..12i64 {
            let n = k % 8;
            let got = eng.run(&[n]);
            let want = run_module(&isa, &module, &[n]);
            assert_eq!(
                got,
                want,
                "engine != oracle at call {k} (n={n}), tier {}",
                eng.tier_of(1)
            );
        }
        assert_eq!(eng.tier_of(1), "cranelift", "fact should reach T2");
    }

    // a non-recursive leaf so each `run` is exactly one dispatch (clean monotonic counting)
    fn sq_module() -> Module {
        let sq = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 2,
            code: vec![Inst::new(MUL_, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
        };
        Module {
            funcs: vec![sq],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        }
    }

    #[test]
    fn engine_tiers_up_monotonically() {
        let isa = call_isa();
        let module = sq_module();
        let eng = ModuleEngine::new(isa, module, 3, 7);
        let tiers: Vec<&str> = (0..10)
            .map(|_| {
                eng.run(&[5]);
                eng.tier_of(0)
            })
            .collect();
        // calls 1,2 interpret; 3..6 baseline; 7+ cranelift
        assert_eq!(tiers[0], "interpreter");
        assert_eq!(tiers[1], "interpreter");
        assert_eq!(tiers[2], "baseline", "3rd call crosses t1");
        assert_eq!(tiers[5], "baseline");
        assert_eq!(tiers[6], "cranelift", "7th call crosses t2");
        // result correct at every tier
        for _ in 0..3 {
            assert_eq!(eng.run(&[6]), 36);
        }
    }

    #[test]
    fn oob_callee_returns_fill_not_abort() {
        // An out-of-range callee index (what an erroneous JS `(5)()` produces via the non-function
        // sentinel) must yield a DEFINED value, not a panic that aborts across the `extern "C"`
        // dispatcher when the caller is at a compiled tier. t1=0 forces `main` to baseline immediately,
        // so the bad Call goes through compiled code → module_dispatch → must not abort.
        let main = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 99, 0), // callee = 99 (out of range; only func 0 exists)
                Inst::new(CALL, 1, 0, 0), // res = funcs[99]()
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![main],
            fill: 7777,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100); // compile baseline on first call
        assert_eq!(eng.run(&[]), 7777, "OOB callee → defined fill, no process abort");
    }

    #[test]
    fn recursion_tiers_up_mid_flight_and_stays_correct() {
        // fact recurses, so a single run can tier it up WHILE a recursive call tree is live; the
        // add-only code installation must keep in-flight (older-tier) frames valid. Result must stay
        // exact through the transition.
        let isa = call_isa();
        let module = Module {
            entry: 1,
            ..fact_module()
        };
        let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 4);
        for n in [0i64, 1, 5, 6, 8, 10, 3, 7] {
            assert_eq!(eng.run(&[n]), run_module(&isa, &module, &[n]), "fact({n})");
        }
        assert_eq!(eng.tier_of(1), "cranelift");
    }

    // ===================================================================================================
    // O-9 — `ModuleEngine::push_func`. Test plan §5(a)-(h) of the O-9 design doc, as hardened by its panel
    // review (Lens A/B/C): (e) requires the calling trampoline to actually be at a COMPILED tier before it
    // makes a push-segment call (so the `module_dispatch` FFI boundary is really exercised, not just
    // `run_one`'s Rust closure); (f) stresses the `Box<FuncDef>` address-stability argument (§1.2
    // Alternative C) by forcing `pushed`'s backing `Vec` to reallocate WHILE a live `&FuncDef` into it is
    // held across a nested call; (h) exercises the REAL panic → `catch_unwind` → `trapped` chain (through
    // `module_dispatch`), not a simulation of it.
    // ===================================================================================================

    // n*n, structurally identical to `sq_module`'s function but a distinct value (pushed, not original).
    fn sq_like() -> FuncDef {
        FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 2,
            code: vec![Inst::new(MUL_, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
        }
    }

    #[test]
    fn pushed_func_matches_oracle_across_tiers() {
        // (a) A pushed function tiers up T0 → T1 → T2 exactly like an original one, and matches the
        // interpreter oracle at every step.
        let isa = call_isa();
        let module = sq_module(); // funcs = [sq] (idx 0); base = 1
        let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 5);
        let new_def = sq_like();
        let idx = eng.push_func(new_def.clone());
        assert_eq!(
            idx, 1,
            "first push_func on a 1-function module must land at index 1"
        );
        // An `otter::Module` containing the same function at the same index, purely for the oracle
        // comparison below — `otter::Module` itself is never touched by `push_func` (module doc).
        let extended = Module {
            funcs: vec![module.funcs[0].clone(), new_def],
            fill: module.fill,
            host_ctx: module.host_ctx,
            entry: idx,
        };
        for k in 0..12i64 {
            let n = k % 8;
            let got = eng.dispatch(idx, &[n]);
            let want = run_module(&isa, &extended, &[n]);
            assert_eq!(
                got,
                want,
                "pushed fn != oracle at call {k} (n={n}), tier {}",
                eng.tier_of(idx)
            );
        }
        assert_eq!(eng.tier_of(idx), "cranelift", "pushed fn should reach T2");
    }

    #[test]
    fn push_func_does_not_disturb_an_already_tiered_original_function() {
        // (b) add-only across the original/pushed boundary: tier up an ORIGINAL (recursive) function to
        // T2, THEN push several unrelated functions, and confirm the original keeps working (and keeps
        // its tier — no regression/reset) exactly as before.
        let isa = call_isa();
        let module = Module {
            entry: 1,
            ..fact_module()
        };
        let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 4);
        for n in [0i64, 1, 5, 6, 8, 10, 3, 7] {
            assert_eq!(eng.run(&[n]), run_module(&isa, &module, &[n]), "warmup fact({n})");
        }
        assert_eq!(
            eng.tier_of(1),
            "cranelift",
            "fact should be at T2 before any push"
        );

        for _ in 0..5 {
            let junk = FuncDef {
                nparams: 0,
                consts: vec![],
                nlocals: 1,
                code: vec![Inst::new(RET, 0, 0, 0)],
            };
            eng.push_func(junk);
        }

        for n in [2i64, 9, 4, 11] {
            assert_eq!(
                eng.run(&[n]),
                run_module(&isa, &module, &[n]),
                "post-push fact({n})"
            );
        }
        assert_eq!(
            eng.tier_of(1),
            "cranelift",
            "pushing unrelated functions must not disturb the original segment's tier (add-only)"
        );
    }

    #[test]
    fn pushed_func_tiers_up_monotonically() {
        // (c) a pushed function's OWN tier progression is monotonic (interpreter → baseline → cranelift),
        // same shape as `engine_tiers_up_monotonically` but for a push-segment index.
        let isa = call_isa();
        let module = sq_module();
        let eng = ModuleEngine::new(isa, module, 3, 7);
        let idx = eng.push_func(sq_like());
        assert_eq!(idx, 1);
        let tiers: Vec<&str> = (0..10)
            .map(|_| {
                eng.dispatch(idx, &[5]);
                eng.tier_of(idx)
            })
            .collect();
        assert_eq!(tiers[0], "interpreter");
        assert_eq!(tiers[1], "interpreter");
        assert_eq!(tiers[2], "baseline", "3rd call crosses t1");
        assert_eq!(tiers[5], "baseline");
        assert_eq!(tiers[6], "cranelift", "7th call crosses t2");
        for _ in 0..3 {
            assert_eq!(eng.dispatch(idx, &[6]), 36);
        }
    }

    #[test]
    fn consecutive_push_func_indices_are_dense_and_correct() {
        // (d) several `push_func` calls in a row get dense, strictly increasing indices, and each is
        // independently callable and correct.
        let isa = call_isa();
        let module = sq_module(); // base = 1
        let eng = ModuleEngine::new(isa, module, 10_000_000, 10_000_000); // stay interpreted; not the point here
        let mut idxs = Vec::new();
        for k in 1..=4i64 {
            let def = FuncDef {
                nparams: 1,
                consts: vec![],
                nlocals: 3,
                code: vec![
                    Inst::new(MOV, 1, k as i32, 0), // local1 = k
                    Inst::new(MUL_, 2, 0, 1),       // local2 = n * k
                    Inst::new(RET, 2, 0, 0),
                ],
            };
            let idx = eng.push_func(def);
            assert_eq!(
                idx,
                idxs.len() + 1,
                "push indices must be dense/increasing from base=1"
            );
            idxs.push(idx);
        }
        for (i, &idx) in idxs.iter().enumerate() {
            let k = (i + 1) as i64;
            for n in [0i64, 1, 7, -3] {
                assert_eq!(
                    eng.dispatch(idx, &[n]),
                    n * k,
                    "pushed fn #{i} (k={k}) at idx {idx}, n={n}"
                );
            }
        }
    }

    #[test]
    fn original_trampoline_calls_a_pushed_func_via_runtime_callee() {
        // (e) an ORIGINAL function, already tiered up to a COMPILED tier (asserted below BEFORE the
        // push-segment call, per the O-9 panel review — otherwise this test could pass while only ever
        // exercising `run_one`'s plain Rust callback, never the `module_dispatch` FFI boundary that is
        // this feature's actual new/risky code path), calls a callee index it receives at runtime — and
        // that index may point into the PUSHED segment (opened after the trampoline was compiled).
        let isa = call_isa();
        let trampoline = FuncDef {
            nparams: 2, // local0 = callee idx, local1 = arg
            consts: vec![],
            nlocals: 4,
            code: vec![
                Inst::new(CPY, 2, 0, 0),  // local2 = local0 (callee idx)   -- CALL base slot
                Inst::new(CPY, 3, 1, 0),  // local3 = local1 (arg)          -- base+1 slot
                Inst::new(CALL, 0, 2, 1), // local0 = dispatch(callee=local2, base=2, argc=1)
                Inst::new(RET, 0, 0, 0),
            ],
        };
        let identity = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let module = Module {
            funcs: vec![trampoline.clone(), identity.clone()],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(isa.clone(), module, 2, 4);

        // Warm the trampoline up against the ORIGINAL `identity` callee until it reaches a compiled tier.
        for n in 0..10i64 {
            assert_eq!(eng.dispatch(0, &[1, n]), n, "warmup trampoline(identity, {n})");
        }
        assert_eq!(
            eng.tier_of(0),
            "cranelift",
            "trampoline must be at a compiled tier BEFORE the push-segment call below"
        );

        let double = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 3,
            code: vec![
                Inst::new(MOV, 1, 2, 0),  // local1 = 2
                Inst::new(MUL_, 2, 0, 1), // local2 = local0 * local1
                Inst::new(RET, 2, 0, 0),
            ],
        };
        let idx_double = eng.push_func(double.clone());
        assert_eq!(idx_double, 2);

        // Oracle: the same three functions as one ordinary (never-pushed) `otter::Module`.
        let oracle = Module {
            funcs: vec![trampoline, identity, double],
            fill: 0,
            host_ctx: 0,
            entry: 0,
        };
        for n in [0i64, 1, 5, 100, -7] {
            let got = eng.dispatch(0, &[idx_double as i64, n]);
            let want = run_module(&isa, &oracle, &[idx_double as i64, n]);
            assert_eq!(got, want, "compiled trampoline -> pushed callee, n={n}");
        }
    }

    #[test]
    fn push_func_reentrant_during_nested_dispatch_survives_realloc() {
        // (f) reentrancy safety proxy + `Box<FuncDef>` address-stability stress (O-9 design doc §1.2
        // Alternative C). Manually drive `otter::interp::run_one` on a PUSHED function `A` (not an
        // original one — the address-stability question is specific to the `pushed` store), obtain `A`'s
        // `&FuncDef` from the engine BEFORE the call, and — from inside the dispatch callback `run_one`
        // invokes mid-execution (simulating a host reentering the engine while `A`'s frame is live) — call
        // `push_func` enough times to force `pushed`'s backing `Vec` to grow/reallocate at least once
        // (default `Vec` growth reallocates well within 8 pushes). If `pushed` held `FuncDef`s inline
        // instead of `Box<FuncDef>`, this would dangle `f_a` and read garbage/corrupt bytecode; with the
        // `Box` indirection it must complete with the correct result.
        let isa = call_isa();
        let module = sq_module(); // funcs = [sq]; base = 1
        let eng = ModuleEngine::new(isa.clone(), module.clone(), 10_000_000, 10_000_000); // stay interpreted
        let a_def = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 4,
            code: vec![
                Inst::new(MOV, 1, 0, 0),  // local1 = 0 (callee idx = original `sq`, idx 0)
                Inst::new(CPY, 2, 0, 0),  // local2 = local0 (n)          -- CALL base slot
                Inst::new(CALL, 3, 1, 1), // local3 = dispatch(callee=local1, base=1, argc=1) = sq(n)
                Inst::new(RET, 3, 0, 0),
            ],
        };
        let idx_a = eng.push_func(a_def);
        assert_eq!(idx_a, 1);
        let f_a = eng.func_ref(idx_a).expect("A was just pushed");

        let mut tail = TailReq::default();
        let result = otter::interp::run_one(
            &isa,
            f_a,
            &[7],
            module.fill,
            module.host_ctx,
            &mut |callee, cargs| {
                for k in 0..8usize {
                    let extra = FuncDef {
                        nparams: 0,
                        consts: vec![],
                        nlocals: 1,
                        code: vec![Inst::new(RET, 0, 0, 0)],
                    };
                    let got = eng.push_func(extra);
                    assert_eq!(
                        got,
                        idx_a + 1 + k,
                        "reentrant push indices must stay dense/increasing during A's own live call"
                    );
                }
                eng.dispatch(callee, cargs)
            },
            &mut tail,
        );
        assert!(!tail.pending, "A does not tail-call");
        assert_eq!(
            result, 49,
            "A(7) = sq(7) = 49, even though `pushed` reallocated mid-flight"
        );

        // Post-reallocation, a FRESH lookup of `A` via the normal engine path must still be correct too.
        assert_eq!(
            eng.dispatch(idx_a, &[9]),
            81,
            "A(9) = sq(9) = 81, via the engine after realloc"
        );
    }

    #[test]
    fn oob_callee_returns_fill_not_abort_after_push() {
        // (g) an out-of-range callee stays a defined `fill` (never an abort), including when the pushed
        // segment is non-empty — the OOB index (99) must still fall outside `base + pushed.len()`.
        let main = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 99, 0), // callee = 99 (out of range even after the push below)
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![main],
            fill: 7777,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100); // compile baseline on first call
        let extra = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(MOV, 0, 42, 0), Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(extra);
        assert_eq!(idx, 1);
        assert_eq!(
            eng.run(&[]),
            7777,
            "callee 99 is still OOB after pushing idx 1 → defined fill, no abort"
        );
        assert_eq!(
            eng.dispatch(idx, &[]),
            42,
            "the pushed function itself is still reachable and correct"
        );
    }

    #[test]
    fn pushed_double_borrow_is_a_caught_trap_not_an_abort() {
        // (h) exercise the REAL panic -> `catch_unwind` -> `trapped` chain (not a simulation): deliberately
        // violate the `RefCell` borrow discipline this file documents everywhere (module doc + O-9 design
        // doc §1.4) by holding a live `borrow_mut()` on `pushed` across a call that must read it — through
        // the ACTUAL `module_dispatch` FFI boundary (the trampoline below is forced to a compiled tier
        // first), not through `run_one`'s plain Rust callback. The result must be a caught, defined trap
        // (`trapped() == true`, `fill` returned) — never a process abort (O-9 panel review, Lens A #4).
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 1, 0),  // local0 = 1 (callee idx = the pushed function below)
                Inst::new(CALL, 1, 0, 0), // local1 = dispatch(callee=local0, base=0, argc=0)
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        // t1=0: the trampoline compiles to baseline on its very first call, so its `Call` really goes
        // through the compiled path -> `module_dispatch` (the `extern "C"` + `catch_unwind` boundary),
        // not the interpreter's plain Rust callback.
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(target);
        assert_eq!(
            idx, 1,
            "trampoline's baked callee index (1) must match the pushed function's real index"
        );

        // Hold a live mutable borrow on the SAME `RefCell` the pushed-segment `Call` below must read
        // (`mod tests` is a descendant module, so it can reach this private field directly).
        let guard = eng.pushed.borrow_mut();
        let v = eng.run(&[]);
        drop(guard);

        assert!(
            eng.trapped(),
            "the double-borrow panic must have been caught and poisoned the engine"
        );
        assert_eq!(
            v, -1,
            "a caught panic must yield the module's `fill`, never propagate as an abort"
        );
    }

    // ===================================================================================================
    // O-10 — `ModuleEngine::try_dispatch`/`try_run`/`clear_trap`. Test plan per the O-10 design doc, as
    // hardened by its three-lens panel review:
    //   - Lens B #1: a trap test whose OUTER frame does further arithmetic on the nested trap's `fill`
    //     result before returning, so the final `dispatch()` return value is NOT `fill` — the only test
    //     shape that can actually falsify a naive `v == fill` reimplementation of `try_dispatch` (the bug
    //     this design explicitly guards against by discriminating on the `trapped` flag, never the return
    //     value).
    //   - Lens B #2: OOB-with-pushed-segment-present, through `try_dispatch`/`try_run` themselves (not just
    //     the pre-existing `run`-based test).
    //   - Lens C #1 (must-fix): a trap that occurs OUTSIDE any `module_dispatch`/`tail_signal` FFI boundary
    //     — a direct top-level `try_dispatch` call on a pushed index, with no compiled trampoline anywhere
    //     on the stack — must still come back as `Err(Trap)`, not a raw Rust panic escaping `try_dispatch`.
    //   - Lens C #2: the "retrap loop" analysis (design doc §2(b)) is specifically about a tier-up COMPILE
    //     that panics deterministically (not generic `RefCell` contention); reproduced here with a
    //     genuinely malformed opcode that makes `compile_dispatch_fn` itself panic on every attempt,
    //     through the real `module_dispatch` boundary — also demonstrates (Lens A #2) that a T2 compile
    //     which always panics never gets a chance to fall back to T1.
    // ===================================================================================================

    #[test]
    fn try_dispatch_ok_matches_oracle_across_tiers() {
        // (a) normal calls at every tier return `Ok` matching the interpreter oracle, exactly like `run`.
        let isa = call_isa();
        let module = fact_module();
        let module = Module { entry: 1, ..module };
        let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 5);
        for k in 0..12i64 {
            let n = k % 8;
            let got = eng.try_run(&[n]);
            let want = run_module(&isa, &module, &[n]);
            assert_eq!(got, Ok(want), "try_run != oracle at call {k} (n={n})");
        }
        assert_eq!(eng.tier_of(1), "cranelift");
        assert!(!eng.trapped());
    }

    #[test]
    fn try_run_returns_err_trap_on_real_double_borrow_through_module_dispatch() {
        // (b) the same real (not simulated) double-borrow panic as
        // `pushed_double_borrow_is_a_caught_trap_not_an_abort`, observed through `try_run` — must come back
        // as `Err(Trap{idx})`, `idx` naming the pushed function in flight when the panic was caught.
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 1, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(target);
        assert_eq!(idx, 1);

        let guard = eng.pushed.borrow_mut();
        let result = eng.try_run(&[]);
        drop(guard);

        assert!(matches!(result, Err(Trap { idx: 1 })), "got {result:?}");
        assert!(eng.trapped());
    }

    #[test]
    fn try_dispatch_on_an_already_trapped_engine_reports_the_original_trap_idx_not_the_new_call_idx() {
        // (review finding, post-implementation) `try_dispatch`'s doc explicitly promises that calling an
        // ALREADY-trapped engine returns `Err(Trap{idx})` naming the ORIGINAL trap site, not the idx of
        // this new (short-circuited, no-op) call — `dispatch`'s loop-head check returns `fill` immediately
        // without touching `trapped`/`trapped_idx` at all. No prior test exercised this specific branch: the
        // others all either check right after the trap (same idx) or call `clear_trap` before retrying.
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 1, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx_target = eng.push_func(target); // idx 1 -- this is what will actually trap
        assert_eq!(idx_target, 1);
        let idx_other = eng.push_func(sq_like()); // idx 2 -- an unrelated, perfectly valid function
        assert_eq!(idx_other, 2);

        let guard = eng.pushed.borrow_mut();
        let first = eng.try_run(&[]); // traps at idx 1 (trampoline calls idx 1 -> double-borrow panic)
        assert!(matches!(first, Err(Trap { idx: 1 })), "got {first:?}");
        assert!(eng.trapped());

        // NO clear_trap() here -- call a DIFFERENT, perfectly-valid idx while still poisoned. `dispatch`'s
        // loop-head short-circuit means this call never touches `idx_other` at all.
        let second = eng.try_dispatch(idx_other, &[5]);
        drop(guard);

        assert!(
            matches!(second, Err(Trap { idx: 1 })),
            "an already-trapped engine must report the ORIGINAL trap's idx (1), not the idx just passed \
             in (2), got {second:?}"
        );
        assert!(eng.trapped());

        // Sanity: once actually cleared, `idx_other` was never poisoned and works immediately.
        eng.clear_trap();
        assert_eq!(eng.try_dispatch(idx_other, &[5]), Ok(25));
    }

    #[test]
    fn try_dispatch_oob_returns_ok_fill_not_err() {
        // (c) an out-of-range callee is the pre-existing, defined `fill` fallback, NOT a trap.
        let main = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 99, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![main],
            fill: 7777,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        assert_eq!(eng.try_run(&[]), Ok(7777));
        assert!(!eng.trapped());
    }

    #[test]
    fn try_dispatch_oob_returns_ok_fill_after_push() {
        // (c2, Lens B #2) same as `oob_callee_returns_fill_not_abort_after_push`, but verified through
        // `try_dispatch`/`try_run` themselves — an OOB index that stays out of range even after the push
        // must still be `Ok(fill)`, never `Err`; the pushed function itself must still be reachable.
        let main = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 99, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![main],
            fill: 7777,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let extra = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(MOV, 0, 42, 0), Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(extra);
        assert_eq!(idx, 1);
        assert_eq!(eng.try_run(&[]), Ok(7777));
        assert!(!eng.trapped());
        assert_eq!(eng.try_dispatch(idx, &[]), Ok(42));
    }

    #[test]
    fn clear_trap_leaves_unrelated_pushed_function_unaffected() {
        // (d) after a trap on one pushed function + `clear_trap`, a COMPLETELY UNRELATED pushed function
        // keeps working, matches the oracle, and its own tier/call-count progression is untouched.
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 1, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx_target = eng.push_func(target);
        assert_eq!(idx_target, 1);
        let idx_sq = eng.push_func(sq_like());
        assert_eq!(idx_sq, 2);

        // Warm the unrelated function up a bit BEFORE the trap, so we can confirm its tier/count survive
        // untouched.
        for n in [1i64, 2, 3] {
            assert_eq!(eng.dispatch(idx_sq, &[n]), n * n);
        }
        let calls_before = eng.calls(idx_sq);
        let tier_before = eng.tier_of(idx_sq);

        let guard = eng.pushed.borrow_mut();
        let result = eng.try_run(&[]);
        drop(guard);
        assert!(matches!(result, Err(Trap { idx: 1 })), "got {result:?}");
        eng.clear_trap();
        assert!(!eng.trapped());

        assert_eq!(
            eng.calls(idx_sq),
            calls_before,
            "trap + clear_trap must not touch an unrelated index's call count"
        );
        assert_eq!(
            eng.tier_of(idx_sq),
            tier_before,
            "trap + clear_trap must not touch an unrelated index's tier"
        );
        for n in [0i64, 1, 5, 7] {
            assert_eq!(eng.try_dispatch(idx_sq, &[n]), Ok(n * n));
        }
    }

    #[test]
    fn clear_trap_does_not_fix_an_external_condition_that_still_holds() {
        // (e) `clear_trap` is a poison-flag reset, not a retry/backoff mechanism. This test uses an
        // externally-held `RefCell` borrow (the same proxy the rest of this file's trap tests use for a
        // real, reproducible `module_dispatch`-boundary panic) — NOT the tier-up-compiler-panic scenario
        // from the design doc's §2(b) analysis, which is exercised separately below by
        // `clear_trap_retrap_loop_pins_to_the_same_backend_forever` (Lens C #2) using a genuinely malformed
        // opcode instead.
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 1, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(target);
        assert_eq!(idx, 1);

        let guard = eng.pushed.borrow_mut();
        let first = eng.try_run(&[]);
        assert!(matches!(first, Err(Trap { idx: 1 })), "got {first:?}");
        eng.clear_trap();
        assert!(!eng.trapped());

        let second = eng.try_run(&[]); // same guard, same condition -> must retrap identically
        assert!(
            matches!(second, Err(Trap { idx: 1 })),
            "clear_trap does not fix the underlying cause — same condition must retrap identically, \
             got {second:?}"
        );
        eng.clear_trap();
        drop(guard); // remove the real cause

        let third = eng.try_run(&[]);
        assert!(
            third.is_ok(),
            "once the real condition is gone, the same call must succeed, got {third:?}"
        );
    }

    #[test]
    fn try_run_discriminates_by_flag_not_by_return_value() {
        // (Lens B #1) the OUTER (compiled) frame does further arithmetic on the nested trap's `fill` result
        // before returning, so the final `dispatch()` return value is emphatically NOT `fill` — this is the
        // only test shape that can falsify a naive `v == fill` reimplementation of `try_dispatch` (the
        // design's whole point: discriminate on the `trapped` flag, never the return value). A naive
        // `if v == fill { Err } else { Ok(v) }` implementation would see `v == -6` here and wrongly return
        // `Ok(-6)`.
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 4,
            code: vec![
                Inst::new(MOV, 0, 1, 0),  // local0 = 1 (callee idx = the pushed target below)
                Inst::new(CALL, 1, 0, 0), // local1 = dispatch(callee=local0, argc=0) -> traps, = fill (-1)
                Inst::new(MOV, 2, 5, 0),  // local2 = 5
                Inst::new(SUB_, 3, 1, 2), // local3 = local1 - local2 = fill - 5 = -6  (!= fill)
                Inst::new(HALT_, 3, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(target);
        assert_eq!(idx, 1);

        let guard = eng.pushed.borrow_mut();
        let result = eng.try_run(&[]);
        drop(guard);

        assert!(
            matches!(result, Err(Trap { idx: 1 })),
            "try_run must discriminate by the `trapped` flag, not by whether the return value equals \
             `fill` (the outer frame here transforms it via arithmetic before returning); got {result:?}"
        );
        assert!(eng.trapped());
    }

    #[test]
    fn try_dispatch_catches_a_top_level_panic_with_no_compiled_frame_on_the_stack() {
        // (Lens C #1, must-fix) a panic that happens on the FIRST-EVER dispatch of an index — with NO
        // `module_dispatch`/`tail_signal` FFI boundary anywhere on the stack yet, because `try_dispatch` was
        // called directly, not reached through a compiled caller's `Call` — must still come back as
        // `Err(Trap)`, never escape as a raw Rust panic. This is exactly `push_func`'s REPL use case ("push,
        // then immediately call") — the scenario `try_dispatch` exists to serve.
        let module = Module {
            funcs: vec![],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 100);
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(RET, 0, 0, 0)],
        };
        let idx = eng.push_func(target);
        assert_eq!(idx, 0, "first push_func on a 0-function module lands at index 0");

        // Hold a live borrow on the SAME `RefCell` `in_range` must read for a pushed idx -- `try_dispatch`
        // here goes STRAIGHT into `dispatch`'s loop head with NO compiled trampoline anywhere on the stack.
        let guard = eng.pushed.borrow_mut();
        let result = eng.try_dispatch(idx, &[]);
        drop(guard);

        assert!(
            matches!(result, Err(Trap { idx: 0 })),
            "a top-level (non-nested) panic must be caught by try_dispatch's own boundary, not escape as \
             a raw panic; got {result:?}"
        );
        assert!(eng.trapped());
    }

    #[test]
    fn clear_trap_retrap_loop_pins_to_the_same_backend_forever() {
        // (Lens C #2 + Lens A #2) the design doc's §2(b) "retrap loop" analysis is specifically about a
        // TIER-UP COMPILE that panics deterministically (not generic `RefCell` contention, which
        // `clear_trap_does_not_fix_an_external_condition_that_still_holds` above already covers
        // separately). Reproduced here with a genuinely malformed opcode -- `compile_dispatch_fn` panics on
        // `isa.op(inst.op)` (an out-of-range opcode index) on EVERY attempt, through the real
        // `module_dispatch` boundary. With t1==t2==0, cranelift is tried first every time (per Lens A #2:
        // `maybe_tier_up` unconditionally `return`s once `c >= t2_threshold`, regardless of success or
        // failure) -- so this function can NEVER fall back to baseline (T1), no matter how many times it's
        // retried after `clear_trap`.
        let trampoline = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(MOV, 0, 1, 0),
                Inst::new(CALL, 1, 0, 0),
                Inst::new(HALT_, 1, 0, 0),
            ],
        };
        let module = Module {
            funcs: vec![trampoline],
            fill: -1,
            host_ctx: 0,
            entry: 0,
        };
        let eng = ModuleEngine::new(call_isa(), module, 0, 0); // t1=t2=0: compile immediately, cranelift first
        let malformed = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            // opcode 9999 is far outside `call_isa()`'s 9-entry `ops` table -- `isa.op(9999)` panics on the
            // very first pre-scan inside `compile_dispatch_fn`, well before either backend allocates any
            // executable memory.
            code: vec![Inst::new(9999, 0, 0, 0)],
        };
        let idx = eng.push_func(malformed);
        assert_eq!(idx, 1);

        for attempt in 0..3 {
            let result = eng.try_run(&[]);
            assert!(
                matches!(result, Err(Trap { idx: 1 })),
                "attempt {attempt}: got {result:?}"
            );
            assert!(eng.trapped());
            assert_eq!(
                eng.tier_of(idx),
                "interpreter",
                "attempt {attempt}: a T2 compile that always panics must never fall back to T1 -- the \
                 slot stays uncompiled forever"
            );
            eng.clear_trap();
        }
        assert!(
            !eng.trapped(),
            "clear_trap must leave the engine usable for OTHER calls even though this idx keeps retrapping"
        );
    }

    // --- O-11: HostFnCtx1/HostFnCtx2 + Expr::HostCtx1/HostCtx2 (Module::host_ctx threading) -----------

    extern "C" fn hostctx1_fn(host_ctx: Value, x: Value) -> Value {
        host_ctx.wrapping_add(x)
    }
    extern "C" fn hostctx2_fn(host_ctx: Value, x: Value, y: Value) -> Value {
        host_ctx ^ x ^ y
    }

    const HCTX1: u16 = 9;
    const HCTX2: u16 = 10;
    const TAILCALL: u16 = 11;

    /// `call_isa()` (0..=8: mov_imm/mul/sub/lt/jump_if/copy/call/ret/halt) extended with `HostCtx1`
    /// (9), `HostCtx2` (10), and `TailCall` (11) — additive, so every existing `call_isa()`-based test
    /// (`fact_module`/`sq_module` and friends) keeps working unmodified against this ISA too.
    fn call_isa_with_hostctx() -> Isa {
        let mut isa = call_isa();
        isa.ops.push(OpDef {
            name: "hostctx1",
            effects: vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::HostCtx1(hostctx1_fn, Box::new(Expr::LocalAt(1))),
            }],
            hints: OpHints {
                pure: false,
                terminator: false,
            },
        }); // 9
        isa.ops.push(OpDef {
            name: "hostctx2",
            effects: vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::HostCtx2(
                    hostctx2_fn,
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::LocalAt(2)),
                ),
            }],
            hints: OpHints {
                pure: false,
                terminator: false,
            },
        }); // 10
        isa.ops.push(OpDef {
            name: "tailcall",
            effects: vec![Effect::TailCall { base: 0, argc: 1 }],
            hints: OpHints {
                pure: false,
                terminator: true,
            },
        }); // 11
        isa
    }

    /// A [`ModuleEngine::push_func`]-injected function inherits the SAME `module.host_ctx` an original
    /// function sees — `maybe_tier_up`/`dispatch` never receive a per-function `host_ctx` (there is only
    /// ever one `Module`/`host_ctx` per engine), so this is true "for free", but O-11's design doc §6c
    /// calls it out as a required test, and it exercises `push_func` + `HostCtx1` together (a
    /// combination no other test here covers).
    #[test]
    fn pushed_func_inherits_module_host_ctx() {
        let isa = call_isa_with_hostctx();
        let base = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(HALT_, 0, 0, 0)],
        };
        let module = Module {
            funcs: vec![base],
            fill: 0,
            host_ctx: 0xC0FFEE_i64,
            entry: 0,
        };
        let eng = ModuleEngine::new(isa, module, 3, 8);
        // pushed(a): r1 = HostCtx1(host_ctx, a); return r1
        let pushed = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 2,
            code: vec![Inst::new(HCTX1, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
        };
        let idx = eng.push_func(pushed);
        assert_eq!(idx, 1);
        for k in 0..12i64 {
            let a = k % 7;
            let got = eng.dispatch(idx, &[a]);
            assert_eq!(
                got,
                hostctx1_fn(0xC0FFEE, a),
                "pushed fn host_ctx inheritance at call {k} (tier {})",
                eng.tier_of(idx)
            );
        }
        assert_eq!(eng.tier_of(idx), "cranelift");
    }

    /// **Multi-instance non-interference** (design §6d, the core O-11 requirement): two `ModuleEngine`s
    /// built from two `Module`s that share the SAME bytecode but have DIFFERENT `host_ctx` words never
    /// observe each other's context, at every tier, even when calls are interleaved (engine A, then B,
    /// then A, …) — verifying `host_ctx` is truly baked per-`Module`/per-compile, not accidentally
    /// shared via a static/thread-local or any other cross-instance state.
    #[test]
    fn two_module_engines_with_different_host_ctx_do_not_interfere() {
        let isa = call_isa_with_hostctx();
        // func(a): r1 = HostCtx1(host_ctx, a); r2 = HostCtx2(host_ctx, r1, a); return r2 -- exercises
        // BOTH HostCtx1 and HostCtx2 in the same differential run.
        let func = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 3,
            code: vec![
                Inst::new(HCTX1, 1, 0, 0),
                Inst::new(HCTX2, 2, 1, 0),
                Inst::new(RET, 2, 0, 0),
            ],
        };
        let module_a = Module {
            funcs: vec![func.clone()],
            fill: 0,
            host_ctx: 0xAAAA_AAAA_i64,
            entry: 0,
        };
        let module_b = Module {
            funcs: vec![func],
            fill: 0,
            host_ctx: i64::MIN, // full-width value, deliberately far from module_a's
            entry: 0,
        };
        let eng_a = ModuleEngine::new(isa.clone(), module_a, 2, 5);
        let eng_b = ModuleEngine::new(isa, module_b, 2, 5);
        // Interleave: A, B, A, B, … through T0 -> T1 -> T2 for BOTH engines.
        for k in 0..12i64 {
            let x = k % 5;
            let got_a = eng_a.run(&[x]);
            let got_b = eng_b.run(&[x]);
            let want_a = hostctx2_fn(0xAAAA_AAAA, hostctx1_fn(0xAAAA_AAAA, x), x);
            let want_b = hostctx2_fn(i64::MIN, hostctx1_fn(i64::MIN, x), x);
            assert_eq!(got_a, want_a, "engine A at call {k} (tier {})", eng_a.tier_of(0));
            assert_eq!(got_b, want_b, "engine B at call {k} (tier {})", eng_b.tier_of(0));
        }
        assert_eq!(eng_a.tier_of(0), "cranelift");
        assert_eq!(eng_b.tier_of(0), "cranelift");
    }

    /// **Heterogeneous-tier caller/callee** (panel C finding 3): an interpreted (T0) caller invoking an
    /// ALREADY-T2-compiled callee — via BOTH an ordinary `Call` and a `TailCall` — must see the correct
    /// `host_ctx` inside the callee's `HostCtx1`. This specifically exercises `ModuleEngine::dispatch`'s
    /// `run_one` branch (the T0 caller path) finding `best_code(idx)` already `Some` for the callee (the
    /// nested `self.dispatch()` call from `run_one`'s closure / from the `TailReq` trampoline resolves
    /// straight into `call_compiled`, never re-interpreting the callee) — a call-boundary combination the
    /// per-function warmup tests above don't isolate (there, caller and callee tier up together).
    #[test]
    fn hostctx_correct_when_t0_caller_invokes_a_t2_callee_via_call_and_tailcall() {
        let isa = call_isa_with_hostctx();
        // callee(a) [func 0]: r1 = HostCtx1(host_ctx, a); return r1
        let callee = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 2,
            code: vec![Inst::new(HCTX1, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
        };
        // caller_call(a) [func 1]: ordinary Call into callee (func 0), forward its result.
        //   locals: 0=a(param), 1=block[0]=callee idx, 2=block[1]=a, 3=res
        let caller_call = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 4,
            code: vec![
                Inst::new(MOV, 1, 0, 0),  // block[0] = callee idx (func 0)
                Inst::new(CPY, 2, 0, 0),  // block[1] = a
                Inst::new(CALL, 3, 1, 1), // res = callee(a)  (dest=3, base=1, argc=1)
                Inst::new(RET, 3, 0, 0),
            ],
        };
        // caller_tail(a) [func 2]: TailCall into callee (func 0).
        //   locals: 0=a(param), 1=block[0]=callee idx, 2=block[1]=a
        let caller_tail = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 3,
            code: vec![
                Inst::new(MOV, 1, 0, 0),      // block[0] = callee idx (func 0)
                Inst::new(CPY, 2, 0, 0),      // block[1] = a
                Inst::new(TAILCALL, 1, 1, 0), // tailcall callee(a) -- base=1, argc=1
            ],
        };
        let module = Module {
            funcs: vec![callee, caller_call, caller_tail],
            fill: 0,
            host_ctx: 0xFEED_BEEF_i64,
            entry: 0,
        };
        let eng = ModuleEngine::new(isa, module, 2, 4);

        // Warm the CALLEE (idx 0) to T2 FIRST, dispatching it directly (never through Call/TailCall).
        for k in 0..10i64 {
            assert_eq!(
                eng.dispatch(0, &[k]),
                hostctx1_fn(0xFEED_BEEF, k),
                "warmup call {k}"
            );
        }
        assert_eq!(
            eng.tier_of(0),
            "cranelift",
            "callee should have reached T2 before any caller runs"
        );

        // The ordinary-Call caller (idx 1) is dispatched for the FIRST time here -- still T0 (this is its
        // 1st call; t1 threshold is 2) -- straight into the already-T2 callee.
        assert_eq!(eng.tier_of(1), "interpreter");
        let got_call = eng.dispatch(1, &[7]);
        assert_eq!(
            got_call,
            hostctx1_fn(0xFEED_BEEF, 7),
            "T0 caller (ordinary Call) -> T2 callee"
        );

        // Same for the TailCall caller (idx 2), also its 1st call -- exercises `run_one`'s `TailReq`
        // trampoline (still inside `dispatch`'s T0/`run_one` branch) resolving into the SAME T2 callee.
        assert_eq!(eng.tier_of(2), "interpreter");
        let got_tail = eng.dispatch(2, &[7]);
        assert_eq!(
            got_tail,
            hostctx1_fn(0xFEED_BEEF, 7),
            "T0 caller (TailCall) -> T2 callee"
        );

        assert_eq!(
            eng.tier_of(0),
            "cranelift",
            "callee's tier must be unaffected by being called"
        );
    }

    /// O-14 §3.2a (A-C1) value-level regression: a FULL-WIDTH out-of-range callee word — one that
    /// is only out of range above bit 32 (`2^32 + k` with `k < nfuncs`, `-2^32`, `u32::MAX + 1`)
    /// — must take the SAME defined `fill` fallback as a small OOB index like 99, at every tier
    /// (T0 `run_one`/`TailReq`, T1 baseline and T2 cranelift through `module_dispatch`/
    /// `tail_signal`), and must NOT call the function its low 32 bits happen to name. On this
    /// 64-bit test target the old bare `as usize` already preserved all 64 bits, so this is a
    /// semantic PIN of "the width check precedes the cast" (`otter::interp::callee_index`): it
    /// goes red if anyone reintroduces a width-dependent truncation (`as u32`-style) into the
    /// shared conversion, and it is the 64-bit-executable half of the A-C1 gate (the wasm32 half
    /// is the same conversion being target-independent by construction — one shared helper).
    #[test]
    fn full_width_oob_callee_returns_fill_not_a_truncated_call_across_tiers() {
        let isa = call_isa_with_hostctx(); // for TAILCALL (op 11)
        // caller_call(w) [func 0]: res = funcs[w](); return res
        //   locals: 0=w(param), 1=block[0]=callee word, 2=res
        let caller_call = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 3,
            code: vec![
                Inst::new(CPY, 1, 0, 0),  // block[0] = callee word
                Inst::new(CALL, 2, 1, 0), // res = funcs[w]()   (dest=2, base=1, argc=0)
                Inst::new(RET, 2, 0, 0),
            ],
        };
        // caller_tail(w) [func 1]: tailcall funcs[w]()
        let caller_tail = FuncDef {
            nparams: 1,
            consts: vec![],
            nlocals: 2,
            code: vec![
                Inst::new(CPY, 1, 0, 0),      // block[0] = callee word
                Inst::new(TAILCALL, 1, 0, 0), // base=1, argc=0
            ],
        };
        // target [func 2]: what a 32-bit truncation of `2^32 + 2` would name. Returns 42.
        let target = FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(MOV, 0, 42, 0), Inst::new(RET, 0, 0, 0)],
        };
        const FILL: Value = 7777;
        let module = Module {
            funcs: vec![caller_call, caller_tail, target],
            fill: FILL,
            host_ctx: 0,
            entry: 0,
        };
        // Full-width OOB words whose LOW 32 BITS are in range (nfuncs = 3):
        //   2^32 + 2 → low bits 2 (target)          — the design's `2^32 + k, k < nfuncs` case
        //   -2^32    → low bits 0 (caller_call)     — the negative case M4's -3 does NOT cover
        //   2^32     → low bits 0, == u32::MAX + 1  — the exact wrap boundary
        let oob = [(1i64 << 32) + 2, -(1i64 << 32), 1i64 << 32];
        for (t1, t2, tier) in [
            (100, 100, "interpreter"),
            (0, 100, "baseline"),
            (0, 0, "cranelift"),
        ] {
            let eng = ModuleEngine::new(isa.clone(), module.clone(), t1, t2);
            // Non-vacuity: an in-range callee word (2) really calls through to `target` at this
            // tier — so the `fill` results below are the guard's doing, not broken plumbing.
            assert_eq!(eng.dispatch(0, &[2]), 42, "in-range Call sanity at ({t1},{t2})");
            assert_eq!(
                eng.dispatch(1, &[2]),
                42,
                "in-range TailCall sanity at ({t1},{t2})"
            );
            assert_eq!(eng.calls(2), 2, "target called exactly twice by the sanity runs");
            for &w in &oob {
                assert_eq!(
                    eng.dispatch(0, &[w]),
                    FILL,
                    "Call callee={w}: full-width OOB must be the fill fallback at ({t1},{t2})"
                );
                assert_eq!(
                    eng.dispatch(1, &[w]),
                    FILL,
                    "TailCall callee={w}: full-width OOB must be the fill fallback at ({t1},{t2})"
                );
            }
            // …and the OOB runs called NOTHING (fill as data, not a call, not a trap):
            assert_eq!(
                eng.calls(2),
                2,
                "a full-width OOB callee must never reach the function its low 32 bits name"
            );
            assert!(!eng.trapped(), "fill fallback is a defined value, never a trap");
            assert_eq!(eng.tier_of(0), tier, "Call caller ran at the intended tier");
            assert_eq!(eng.tier_of(1), tier, "TailCall caller ran at the intended tier");
        }
    }
}
