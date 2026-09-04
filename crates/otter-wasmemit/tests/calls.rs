//! W-2 §8(a) harness — the **call family** (`Call`/`TailCall`, design §4.3) run differentially
//! against the multi-function oracle [`otter::interp::run_module`], through a mini engine that is
//! a **line-by-line mirror of `otter-engine`'s dispatcher** (design §8a C#6: "T0 `run_one`
//! recursion" would blow the host stack at 1M-deep tail recursion; the stub must BE the engine's
//! trampoline loop). Every mirrored statement carries a comment naming the
//! `otter-engine/src/lib.rs` construct it mirrors — **structural anchors** (function/statement
//! names), never line numbers, which go stale on every engine edit (W-6 re-anchored this whole
//! file after W-3 shifted its W-2-era line numbers):
//!
//! * [`mini_dispatch`]        ↔ `ModuleEngine::dispatch` (the trampoline loop)
//! * the dispatch table stub  ↔ `module_dispatch` (the baked `extern "C"` dispatcher)
//! * the tail table stub      ↔ `tail_signal` (the baked tail-request helper)
//! * [`call_wasm`]            ↔ `ModuleEngine::call_compiled`
//!
//! **Copy-at-entry (§2.5-2 mirror — a DISCIPLINE mirror, not a test-protected property)**: the
//! dispatch stub receives `args_ptr` pointing into the *calling emitted function's own scratch*
//! and copies those words into an owned `Vec` immediately, before any callee code runs —
//! mirroring the ORDERING the W-3 `module_dispatch` copy-at-once variant must follow (design §10
//! W-2 row). Be precise about what the tests below do and do not establish (panel #3 lens B
//! finding #1, proven by mutation): the fact test's memory assertion demonstrates that the
//! danger window is REAL — a self-recursive `Call` reuses the SAME per-function scratch at every
//! nesting level, so the outer activation's marshaled words genuinely are clobbered by its own
//! callees. But removing the stub's copy entirely leaves every differential green, because every
//! consumer (the emitted prologue's argc-guarded loads, `run_one`'s locals init, `call_wasm`'s
//! staging) reads its arguments before its own callee's first clobber anyway — any safe-Rust
//! copy-timing variant is observationally equivalent. §2.5-2's actual payload is the
//! BORROW-LIFETIME half (§13-1/13-7: shrinking the live `&[Value]` window to the copy loop so
//! the engine's `noalias` obligations hold), which is UB-class and therefore CANNOT be protected
//! by differential tests at all — W-3's copy-at-once variant is guarded by review against the
//! §13 norms, not by this suite. The final authority on trampoline semantics is the §8(c) E2E
//! run on the real engine (§8a C#6).
//!
//! Oracle scope (§8a B#7): `run_module` treats a NESTED `Halt` as whole-module exit while
//! `run_one`/compiled tiers treat it like `Return`, so this corpus contains no nested `Halt`
//! (the only `Halt` sits in a top-frame `main`).

mod common;

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::time::Instant;

use common::{CALL, COPY, HALT, JUMP_IF, MOV_IMM, RET, TAILCALL, bin_opcode};
use otter::interp::{TailReq, run_module, run_one};
use otter::{FuncDef, Inst, Isa, Module as IrModule, Value};
use otter_wasmemit::{HelperIdx, Refusal, compile_dispatch_fn, scratch_words};
use wasmtime::{
    AsContextMut, Caller, Config, Engine, Func, Instance, Memory, MemoryType, Module, Ref, RefType, Store,
    StoreContextMut, Table, TableType, TypedFunc,
};

// --- table layout (slot 0 stays null) + memory layout --------------------------------------------

const BINOP_SLOT: u32 = 3;
const CONVERT_SLOT: u32 = 5;
const UNOP_SLOT: u32 = 7;
const DISPATCH_SLOT: u32 = 11;
const TAIL_SLOT: u32 = 12;
const TABLE_MIN: u32 = 32;

/// Mock linear memory: 16 pages (1 MiB).
const MEMORY_PAGES: u32 = 16;
/// Per-function marshaling scratches are bump-allocated from here (exact-size, §2.5-1).
const SCRATCH_BASE: u32 = 0x1_0000;
/// LIFO staging area for [`call_wasm`]'s argument marshaling — the harness analogue of the
/// native caller-frame buffer, deliberately disjoint from every function's scratch (§13-7(ii)).
const STAGING_BASE: u32 = 0x8_0000;
/// A junk ctx word: emitted code must forward it opaquely at most — never dereference it.
const CTX_JUNK: i32 = 0x7EAD;
/// Per-wasm-activation fuel budget — see the re-arm note in [`call_wasm`].
const FUEL_BUDGET: u64 = 100_000_000;

const FILL: Value = 0x5EED_F00D_5EED_F00D_u64 as Value;

fn helper_idx() -> HelperIdx {
    HelperIdx {
        dispatch: DISPATCH_SLOT,
        tail: TAIL_SLOT,
        binop: BINOP_SLOT,
        convert: CONVERT_SLOT,
        unop: UNOP_SLOT,
    }
}

// --- the mini engine (otter-engine mirror) --------------------------------------------------------

/// The immutable module-level data, shared (via `Arc`) between the store data and the dispatcher
/// so [`mini_dispatch`] can hold `&FuncDef`/`&Isa` across the mutable store borrows the nested
/// wasm calls need — the same separation the engine gets from `&self` + interior mutability.
struct Env {
    isa: Isa,
    module: IrModule,
}

/// One function's fixed wasm tier: its instantiated export + its baked scratch address.
#[derive(Clone)]
struct CompiledWasm {
    f: TypedFunc<(i32, i32, i64), i64>,
    scratch_addr: u32,
}

/// Store data — the mirror of `ModuleEngine`'s per-engine state, with the same interior-mutability
/// discipline (`Cell`/`RefCell` behind a shared borrow; every borrow released before any nested
/// call — the engine's module-doc rule, inherited wholesale):
struct St {
    env: Arc<Env>,
    /// `Some` = this function runs at the wasm tier, `None` = T0 — the mirror of the engine's
    /// `best_code`, FIXED per function up front (the mirror has no counters/tier-up).
    compiled: Vec<Option<CompiledWasm>>,
    memory: Option<Memory>,
    /// The engine's `tail_pending`/`tail_callee`/`tail_args` fields — the compiled-tier
    /// tail-signal triple, mirrored field-for-field.
    tail_pending: Cell<bool>,
    tail_callee: Cell<usize>,
    tail_args: RefCell<Vec<Value>>,
    /// [`call_wasm`]'s LIFO staging bump pointer.
    staging_top: Cell<u32>,
}

/// Copy `argc` argument words at `ptr` in the mock linear memory into an owned `Vec` — the
/// copy-at-entry step both table stubs perform FIRST (crate-doc rationale). The `argc <= 0 →
/// empty` arm mirrors the engine's defensive slice guards (the `argc <= 0 || args.is_null()`
/// arm in both `module_dispatch` and `tail_signal` — a negative count must never reach the
/// length computation).
fn read_args(caller: &mut Caller<'_, St>, ptr: i32, argc: i64) -> Vec<Value> {
    if argc <= 0 {
        return Vec::new();
    }
    let mem = caller
        .data()
        .memory
        .expect("memory installed before any dispatch");
    let mut buf = vec![0u8; argc as usize * 8];
    mem.read(&mut *caller, ptr as u32 as usize, &mut buf)
        .expect("args live in the mock memory");
    buf.chunks_exact(8)
        .map(|c| i64::from_le_bytes(c.try_into().expect("chunks_exact(8)")))
        .collect()
}

/// Mirror of `ModuleEngine::call_compiled`: invoke one wasm-tier callee
/// with `args` as its argument words. The native engine passes a raw pointer to the caller-owned
/// buffer backing `cur`; the wasm-ABI equivalent is a **LIFO staging area** in the mock
/// memory, bump-allocated per activation — deliberately NEVER any function's baked scratch, so
/// every buffer a live Rust-side slice refers to stays allocation-disjoint from every scratch
/// (§13-7(ii) in mirror form). Emitted code reads `args` only in its argc-guarded prologue, and
/// the slot is released (LIFO) when the activation returns.
///
/// **Fuel**: re-armed before EVERY wasm activation, not once per test — fuel is Store-cumulative
/// and the 1M-deep tail-recursion gate makes 1M separate one-level wasm calls from the trampoline
/// loop; a single Store-wide budget would either starve that test or be too large to catch
/// runaway loops. Per-activation re-arming keeps each activation's headroom identical and
/// generous while a lowering bug spinning inside ONE activation still traps loudly (the W-1b
/// harness rationale, panel #2b lens B#2). Nested activations re-arm the shared meter too, which
/// only ever ADDS outer headroom — the meter turns hangs into loud failures; it is not an
/// accountant.
fn call_wasm(ctx: &mut StoreContextMut<'_, St>, cw: CompiledWasm, args: &[Value]) -> Value {
    let (mem, addr) = {
        let st = ctx.data();
        let top = st.staging_top.get();
        st.staging_top.set(top + (args.len() as u32) * 8);
        (st.memory.expect("memory installed"), top)
    };
    assert!(
        addr as usize + args.len() * 8 <= (MEMORY_PAGES as usize) * 65_536,
        "staging overflow — bump MEMORY_PAGES"
    );
    ctx.set_fuel(FUEL_BUDGET).expect("set_fuel");
    let mut bytes = Vec::with_capacity(args.len() * 8);
    for a in args {
        bytes.extend_from_slice(&a.to_le_bytes());
    }
    mem.write(&mut *ctx, addr as usize, &bytes)
        .expect("staging write");
    let r =
        cw.f.call(&mut *ctx, (CTX_JUNK, addr as i32, args.len() as i64))
            .expect("emitted f must not trap");
    ctx.data().staging_top.set(addr); // LIFO release
    r
}

/// The harness dispatcher — a **line-by-line mirror of `ModuleEngine::dispatch`**; each
/// `dispatch:` comment maps a statement to the engine statement it mirrors (structural anchors —
/// crate doc). Deliberate differences, each marked at its site: no `trapped` poison state (the
/// loop-head check — the harness has no trap machinery; a panic IS the test failure), no
/// `bump_count`/`maybe_tier_up` (tiers are fixed per function up front, which is also
/// why `best_code` collapses to a vec read), no O-9 pushed segment (`func_ref` — `funcs` is
/// fixed, so it's plain indexing).
///
/// The two tail-signal consumptions are mirrored EXACTLY — the engine-cell one for wasm callees
/// (the post-`call_compiled` consume, consume-IMMEDIATELY rule included) and the `TailReq`
/// out-param one for T0 callees (the post-`run_one` check) — feeding this ONE loop, engine-style.
/// **This loop, not recursion,** is what
/// keeps 1M-deep tail recursion native-stack-bounded (each tail step is one loop iteration at
/// constant host-stack depth); only a non-tail `Call` recurses, through the T0 nested-dispatch
/// closure or the dispatch stub, exactly like the engine.
fn mini_dispatch(ctx: &mut StoreContextMut<'_, St>, mut idx: usize, args: &[Value]) -> Value {
    let env = ctx.data().env.clone();
    let mut owned: Vec<Value> = Vec::new(); // dispatch: prologue `owned`
    let mut cur: &[Value] = args; // dispatch: prologue `cur`
    let mut tail = TailReq::default(); // dispatch: prologue `tail`
    loop {
        // dispatch: the trampoline loop
        // dispatch: loop-head `trapped || !in_range` fail-closed check (poison half omitted —
        // fn doc): an out-of-range callee yields `fill`, never a panic.
        if idx >= env.module.funcs.len() {
            return env.module.fill;
        }
        // dispatch: bump_count / maybe_tier_up — no mirror (fixed tiers; see fn doc).
        let best = ctx.data().compiled[idx].clone(); // dispatch: best_code(idx)
        if let Some(cw) = best {
            let v = call_wasm(ctx, cw, cur); // dispatch: call_compiled(p, cur)
            let st = ctx.data();
            if !st.tail_pending.get() {
                return v; // dispatch: `!tail_pending` → plain value, return it
            }
            // dispatch: consume the signal IMMEDIATELY (read-and-cleared in the same breath), so
            // a nested mini_dispatch never sees a stale flag belonging to an outer frame.
            st.tail_pending.set(false); // dispatch: tail_pending.set(false) FIRST
            idx = st.tail_callee.get(); // dispatch: idx = tail_callee
            core::mem::swap(&mut owned, &mut st.tail_args.borrow_mut()); // dispatch: swap(owned, tail_args)
            cur = &owned; // dispatch: cur = &owned (the swap-then-repoint idiom, verbatim)
            continue; // dispatch: loop re-entry, no new stack frame
        }
        // dispatch: T0 arm — interpret with a tail-call out-param; `tail` is a per-dispatch LOCAL.
        tail.pending = false; // dispatch: tail.pending = false before run_one
        let f = &env.module.funcs[idx]; // dispatch: func_ref (in range per the check above)
        let v = run_one(
            // dispatch: the run_one call, argument-for-argument
            &env.isa,
            f,
            cur,
            env.module.fill,
            env.module.host_ctx,
            // dispatch: the nested-dispatch closure recurses into THIS dispatcher, exactly like
            // the engine's `&mut |callee, cargs| self.dispatch(callee, cargs)`.
            &mut |callee, cargs| mini_dispatch(ctx, callee, cargs),
            &mut tail,
        );
        if !tail.pending {
            return v; // dispatch: `!tail.pending` → plain value, return it
        }
        idx = tail.callee; // dispatch: idx = tail.callee
        core::mem::swap(&mut owned, &mut tail.args); // dispatch: swap(owned, tail.args)
        cur = &owned; // dispatch: cur = &owned
    }
}

struct CallHarness {
    store: Store<St>,
}

impl CallHarness {
    /// Build the mirror engine for `env`, fixing each function's tier by `compiled_mask`
    /// (`true` = compile to wasm and install; `false` = stays T0, like an `Untried` slot).
    fn new(env: Arc<Env>, compiled_mask: &[bool]) -> CallHarness {
        assert_eq!(compiled_mask.len(), env.module.funcs.len());
        let mut config = Config::new();
        config.consume_fuel(true); // W-1b rationale: runaway loops must trap loudly, not hang CI
        let engine = Engine::new(&config).expect("engine");
        let nfuncs = env.module.funcs.len();
        let mut store = Store::new(
            &engine,
            St {
                env: env.clone(),
                compiled: vec![None; nfuncs],
                memory: None,
                tail_pending: Cell::new(false), // ModuleEngine::new — same initial triple
                tail_callee: Cell::new(0),      // ModuleEngine::new
                tail_args: RefCell::new(Vec::new()), // ModuleEngine::new
                staging_top: Cell::new(STAGING_BASE),
            },
        );
        let memory = Memory::new(&mut store, MemoryType::new(MEMORY_PAGES, None)).expect("memory");
        store.data_mut().memory = Some(memory);
        let table = Table::new(
            &mut store,
            TableType::new(RefType::FUNCREF, TABLE_MIN, None),
            Ref::Func(None),
        )
        .expect("table");

        // Mirror of `module_dispatch`, the §2.3 #7 table entry, statement-for-statement:
        //   `let engine = unsafe { &*eng }` (reconstitute  ↔ state rides in the Store; `_ctx` is
        //             the engine from `ctx`)                 the junk word emitted code forwards
        //                                                    opaquely (asserted junk, never read);
        //   the argc<=0/null → empty-slice guard          ↔ read_args' argc<=0 arm;
        //   (W-3 §2.5-2 copy-at-once variant)             ↔ slice formation and copy are ONE step:
        //             read_args materializes an owned Vec BEFORE any callee code runs — the
        //             copy-at-entry mirror (crate doc; pinned by the fact scratch-reuse test);
        //   `engine.dispatch(idx, args)`                  ↔ mini_dispatch;
        //   the catch_unwind + poison wrapper             ↔ none — no trap machinery in the
        //             harness; a panic aborts the test loudly, which is what a harness wants.
        let dispatch_stub = Func::wrap(
            &mut store,
            |mut caller: Caller<'_, St>, _ctx: i32, idx: i64, args: i32, argc: i64| -> i64 {
                let args = read_args(&mut caller, args, argc);
                let mut ctx = caller.as_context_mut();
                // module_dispatch: `callee_index(idx)` — the A-C1 full-width conversion (a no-op
                // on this 64-bit harness host, mirrored for statement-for-statement fidelity).
                mini_dispatch(&mut ctx, otter::interp::callee_index(idx), &args)
            },
        );
        // Mirror of `tail_signal`, the §2.3 #8 table entry: copy the args
        // out of the (still-live) emitted caller's scratch FIRST, then set callee, then set
        // `pending` LAST — never another order (tail_signal's copy-then-signal block, design risk
        // §8.6#1; `pending` is the discriminant the trampoline checks post-call, so it must
        // become true only once the request is fully formed). Returns `fill`, discarded by the
        // emitted caller (tail_signal's trailing `engine.module.fill`).
        let tail_stub = Func::wrap(
            &mut store,
            |mut caller: Caller<'_, St>, _ctx: i32, callee: i64, args: i32, argc: i64| -> i64 {
                let copied = read_args(&mut caller, args, argc); // tail_signal: slice guard + copy-at-entry
                let st = caller.data();
                let mut buf = st.tail_args.borrow_mut(); // tail_signal: tail_args.borrow_mut()
                buf.clear(); // tail_signal: buf.clear()
                buf.extend_from_slice(&copied); // tail_signal: buf.extend_from_slice(slice)
                drop(buf); // tail_signal: drop(buf) — release before the trampoline's own borrow (the swap) runs
                st.tail_callee.set(otter::interp::callee_index(callee)); // tail_signal: tail_callee.set(callee_index(callee)) — A-C1 full-width
                st.tail_pending.set(true); // tail_signal: tail_pending.set(true) — set LAST: the discriminant
                st.env.module.fill // tail_signal: return fill
            },
        );
        // Canonical-helper stubs (same delegation as the W-1 harness — the SAME code T0 runs).
        let binop = Func::wrap(&mut store, |a: i64, b: i64, ty: u32, op: u32| -> i64 {
            otter::otter_binop(a, b, ty, op)
        });
        let convert = Func::wrap(&mut store, |a: i64, from: u32, to: u32| -> i64 {
            otter::otter_convert(a, from, to)
        });
        let unop = Func::wrap(&mut store, |a: i64, ty: u32, op: u32| -> i64 {
            otter::otter_unop(a, ty, op)
        });
        for (slot, f) in [
            (DISPATCH_SLOT, dispatch_stub),
            (TAIL_SLOT, tail_stub),
            (BINOP_SLOT, binop),
            (CONVERT_SLOT, convert),
            (UNOP_SLOT, unop),
        ] {
            table
                .set(&mut store, u64::from(slot), Ref::Func(Some(f)))
                .expect("table.set");
        }

        // Per-function tier choice: compile + instantiate the wasm ones. Scratches are exact-size
        // (§2.5-1: `scratch_words` sizes what the lowering stores — single source) and address-
        // distinct (min 1 word) so the scratch-inspection assertions can never alias.
        let resolve = |_: usize| -> Option<u32> { None }; // corpus has no host fns — refuse loudly if one appears
        let mut scratch_next = SCRATCH_BASE;
        for (idx, f) in env.module.funcs.iter().enumerate() {
            if !compiled_mask[idx] {
                continue;
            }
            let words = scratch_words(&env.isa, f).expect("valid test function");
            let scratch_addr = scratch_next;
            scratch_next += (words.max(1) as u32) * 8;
            assert!(
                scratch_next <= STAGING_BASE,
                "scratch region overflow — grow the layout"
            );
            let bytes = compile_dispatch_fn(
                &env.isa,
                idx,
                f,
                env.module.fill,
                env.module.host_ctx,
                helper_idx(),
                &resolve,
                scratch_addr,
            )
            .expect("call-family function must compile since W-2");
            let module = Module::new(&engine, &bytes).expect("emitted module must validate");
            let instance =
                Instance::new(&mut store, &module, &[memory.into(), table.into()]).expect("instantiate");
            let tf = instance
                .get_typed_func::<(i32, i32, i64), i64>(&mut store, "f")
                .expect("export f of the CompiledFn type");
            store.data_mut().compiled[idx] = Some(CompiledWasm { f: tf, scratch_addr });
        }
        CallHarness { store }
    }

    /// The mirror of `ModuleEngine::run`/`dispatch` as the public entry: run function `idx`.
    fn dispatch(&mut self, idx: usize, args: &[Value]) -> Value {
        let mut ctx = self.store.as_context_mut();
        mini_dispatch(&mut ctx, idx, args)
    }

    fn scratch_addr(&self, idx: usize) -> u32 {
        self.store.data().compiled[idx]
            .as_ref()
            .expect("function is wasm-tier")
            .scratch_addr
    }

    /// Read one 8-byte word of the mock linear memory (scratch inspection).
    fn read_mem_word(&mut self, addr: u32) -> Value {
        let mem = self.store.data().memory.expect("memory");
        let mut b = [0u8; 8];
        mem.read(&self.store, addr as usize, &mut b).expect("read");
        i64::from_le_bytes(b)
    }
}

fn env_of(funcs: Vec<FuncDef>, entry: usize) -> Arc<Env> {
    Arc::new(Env {
        isa: common::isa(),
        module: IrModule {
            funcs,
            fill: FILL,
            host_ctx: 0,
            entry,
        },
    })
}

/// The multi-function T0 oracle (design §8a): bit-for-bit ground truth for every tier mix.
fn oracle(env: &Env, args: &[Value]) -> Value {
    run_module(&env.isa, &env.module, args)
}

// --- test programs (mirroring the otter-engine test-module shapes) --------------------------------

/// `fact(n) = n < 2 ? 1 : n * fact(n-1)` — the engine differential shape (`fact_module` in
/// otter-engine's own test module)
/// on the shared test ISA: a SELF-RECURSIVE `Call`, the §2.5 scratch-reuse core case.
/// locals: 0=n 1=t 2=block0(callee) 3=block1(arg) 4=res 5=tmp.
fn fact_env() -> Arc<Env> {
    let (lt, sub, mul) = (bin_opcode(3, 4), bin_opcode(3, 1), bin_opcode(3, 2));
    let fact = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 5, 2, 0), // 0: tmp = 2
            Inst::new(lt, 1, 0, 5),      // 1: t = n < 2
            Inst::new(JUMP_IF, 1, 9, 0), // 2: if t goto 9
            Inst::new(MOV_IMM, 5, 1, 0), // 3: tmp = 1
            Inst::new(sub, 3, 0, 5),     // 4: block1 = n - 1
            Inst::new(MOV_IMM, 2, 0, 0), // 5: block0 = fact (self)
            Inst::new(CALL, 4, 2, 1),    // 6: res = fact(n-1) — recursive, NON-tail
            Inst::new(mul, 4, 0, 4),     // 7: res = n * res
            Inst::new(RET, 4, 0, 0),     // 8
            Inst::new(MOV_IMM, 4, 1, 0), // 9: res = 1
            Inst::new(RET, 4, 0, 0),     // 10
        ],
    };
    env_of(vec![fact], 0)
}

/// Mutual NON-tail recursion: `f(n) = n==0 ? base : other(n-1) + inc`.
/// locals: 0=n 1=t 2=block0 3=block1 4=res 5=tmp.
fn mutual_fn(other_idx: i32, base_val: i32, inc: i32) -> FuncDef {
    let (eq, sub, add) = (bin_opcode(3, 5), bin_opcode(3, 1), bin_opcode(3, 0));
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 5, 0, 0),         // 0: tmp = 0
            Inst::new(eq, 1, 0, 5),              // 1: t = n == 0
            Inst::new(JUMP_IF, 1, 10, 0),        // 2
            Inst::new(MOV_IMM, 5, 1, 0),         // 3
            Inst::new(sub, 3, 0, 5),             // 4: block1 = n - 1
            Inst::new(MOV_IMM, 2, other_idx, 0), // 5: block0 = the other function
            Inst::new(CALL, 4, 2, 1),            // 6: res = other(n-1)
            Inst::new(MOV_IMM, 5, inc, 0),       // 7
            Inst::new(add, 4, 4, 5),             // 8: res += inc
            Inst::new(RET, 4, 0, 0),             // 9
            Inst::new(MOV_IMM, 4, base_val, 0),  // 10
            Inst::new(RET, 4, 0, 0),             // 11
        ],
    }
}

fn mutual_env() -> Arc<Env> {
    env_of(vec![mutual_fn(1, 7, 1), mutual_fn(0, 11, 2)], 0)
}

/// `countdown(n) = n==0 ? 42 : tailcall countdown(n-1)` — the O-8 decisive-gate program
/// (`countdown_module` in otter-engine's tests/tailcall.rs) in its wasm form.
/// locals: 0=n 1=t 2=block0 3=block1 4=tmp.
fn tail_fn(callee_idx: i32, base_val: i32) -> FuncDef {
    let (eq, sub) = (bin_opcode(3, 5), bin_opcode(3, 1));
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(MOV_IMM, 4, 0, 0),          // 0: tmp = 0
            Inst::new(eq, 1, 0, 4),               // 1: t = n == 0
            Inst::new(JUMP_IF, 1, 7, 0),          // 2
            Inst::new(MOV_IMM, 4, 1, 0),          // 3
            Inst::new(sub, 3, 0, 4),              // 4: block1 = n - 1
            Inst::new(MOV_IMM, 2, callee_idx, 0), // 5: block0 = callee
            Inst::new(TAILCALL, 2, 1, 0),         // 6: tailcall callee(n-1)
            Inst::new(MOV_IMM, 4, base_val, 0),   // 7
            Inst::new(RET, 4, 0, 0),              // 8
        ],
    }
}

fn countdown_env() -> Arc<Env> {
    env_of(vec![tail_fn(0, 42)], 0)
}

/// Mutual TAIL recursion: `[even, odd]` — `even(0)=1`, `odd(0)=0`, else tailcall the other.
fn parity_env() -> Arc<Env> {
    env_of(vec![tail_fn(1, 1), tail_fn(0, 0)], 0)
}

/// The engine's `mixed_module` (otter-engine's tests/tailcall.rs): `main` makes an
/// ordinary `Call` into `helper`; `helper` SELF-tail-recurses to the base case, then tail-calls
/// INTO `leaf` (cross-function `TailCall`). Result is always 999. `Halt` appears ONLY in the top
/// frame (oracle scope, §8a B#7).
fn mixed_env() -> Arc<Env> {
    let (eq, sub) = (bin_opcode(3, 5), bin_opcode(3, 1));
    let main = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4, // 0=n 1=block0 2=block1 3=r
        code: vec![
            Inst::new(MOV_IMM, 1, 1, 0), // 0: block0 = helper
            Inst::new(COPY, 2, 0, 0),    // 1: block1 = n
            Inst::new(CALL, 3, 1, 1),    // 2: r = helper(n) — ordinary, non-tail
            Inst::new(HALT, 3, 0, 0),    // 3: top-frame Halt only
        ],
    };
    let helper = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5, // 0=n 1=t 2=block0 3=block1 4=tmp
        code: vec![
            Inst::new(MOV_IMM, 4, 0, 0),
            Inst::new(eq, 1, 0, 4),
            Inst::new(JUMP_IF, 1, 7, 0),
            Inst::new(MOV_IMM, 4, 1, 0),
            Inst::new(sub, 3, 0, 4),       // block1 = n - 1
            Inst::new(MOV_IMM, 2, 1, 0),   // block0 = helper (self)
            Inst::new(TAILCALL, 2, 1, 0),  // 6: self tail call
            Inst::new(MOV_IMM, 2, 2, 0),   // 7: block0 = leaf
            Inst::new(MOV_IMM, 3, 999, 0), // 8: block1 = 999
            Inst::new(TAILCALL, 2, 1, 0),  // 9: cross-function tail call
        ],
    };
    let leaf = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 1,
        code: vec![Inst::new(RET, 0, 0, 0)],
    };
    env_of(vec![main, helper, leaf], 0)
}

/// argc-boundary pair: the caller marshals ALL `n` of its params into a call block; the callee
/// folds them ORDER-SENSITIVELY (`acc = acc * 31 + p_k`, wrapping) so a dropped, swapped, or
/// mis-offset marshaling slot changes the result. §2.5-1's point made executable: `argc` is an
/// operand VALUE (i32-wide), not a `u8`-bounded count — 255/256/257/1000 are all legitimate.
fn marshal_env(n: usize) -> Arc<Env> {
    let (add, mul) = (bin_opcode(3, 0), bin_opcode(3, 2));
    let ni = n as i32;
    // caller locals: 0..n = params, n = block0(callee), n+1..2n+1 = args block, 2n+1 = res.
    let mut caller_code = vec![Inst::new(MOV_IMM, ni, 1, 0)];
    for k in 0..ni {
        caller_code.push(Inst::new(COPY, ni + 1 + k, k, 0));
    }
    caller_code.push(Inst::new(CALL, 2 * ni + 1, ni, ni));
    caller_code.push(Inst::new(RET, 2 * ni + 1, 0, 0));
    let caller = FuncDef {
        nparams: n,
        consts: vec![],
        nlocals: 2 * n + 2,
        code: caller_code,
    };
    // callee locals: 0..n = params, n = acc, n+1 = the constant 31.
    let mut callee_code = vec![Inst::new(MOV_IMM, ni, 0, 0), Inst::new(MOV_IMM, ni + 1, 31, 0)];
    for k in 0..ni {
        callee_code.push(Inst::new(mul, ni, ni, ni + 1));
        callee_code.push(Inst::new(add, ni, ni, k));
    }
    callee_code.push(Inst::new(RET, ni, 0, 0));
    let callee = FuncDef {
        nparams: n,
        consts: vec![],
        nlocals: n + 2,
        code: callee_code,
    };
    env_of(vec![caller, callee], 0)
}

/// argc = 0: the callee's parameter is never supplied → stays `fill` (argc-guard), scratch unused.
fn zero_argc_env() -> Arc<Env> {
    let caller = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 2, // 0 = block0(callee), 1 = res
        code: vec![
            Inst::new(MOV_IMM, 0, 1, 0),
            Inst::new(CALL, 1, 0, 0), // argc = 0
            Inst::new(RET, 1, 0, 0),
        ],
    };
    let callee = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 1,
        code: vec![Inst::new(RET, 0, 0, 0)],
    };
    env_of(vec![caller, callee], 0)
}

fn distinct_word(k: usize) -> Value {
    (k as i64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as i64)
        .rotate_left((k % 61) as u32)
        ^ 0x0123_4567_89AB_CDEF
}

// --- tests ---------------------------------------------------------------------------------------

/// (6) `scratch_words` unit correctness: max over ALL `Call`/`TailCall` sites, 0 when call-free,
/// a clean `ValidationFailed` (never a panic) on malformed input.
#[test]
fn scratch_words_scans_all_call_sites() {
    let isa = common::isa();
    // Call argc 3, Call argc 7, TailCall argc 5 → max is 7 (and TailCall sites count too).
    let f = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 12,
        code: vec![
            Inst::new(CALL, 11, 0, 3),
            Inst::new(CALL, 11, 0, 7),
            Inst::new(TAILCALL, 0, 5, 0),
        ],
    };
    assert_eq!(scratch_words(&isa, &f).unwrap(), 7);

    let call_free = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 1,
        code: vec![Inst::new(HALT, 0, 0, 0)],
    };
    assert_eq!(scratch_words(&isa, &call_free).unwrap(), 0);

    let malformed = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 1,
        code: vec![],
    };
    assert!(matches!(
        scratch_words(&isa, &malformed),
        Err(Refusal::ValidationFailed(_))
    ));
}

/// (1-core) Self-recursive `Call` (`fact`) at the wasm tier: bit-identical to the oracle, and the
/// **scratch-reuse demonstration** of §2.5 — every activation of the ONE function marshals into
/// the SAME per-function scratch, so the outer activation's marshaled argument is clobbered by
/// its own callees (asserted on the mock memory). What this test establishes is that the danger
/// window is REAL — not that copy-at-entry is what keeps the answer right (panel #3 lens B
/// finding #1 proved, by removing the copy, that every consumer reads its arguments before its
/// own callee's first clobber, so any safe-Rust copy timing is observationally equivalent). The
/// copy's real payload is engine-side and UB-class: without it, a live `&[Value]` over
/// engine-owned scratch would be held across `run_one` while callees rewrite it — the §13-7
/// borrow-lifetime violation the W-3 `module_dispatch` variant exists to prevent, guarded by
/// review (it is not differentially testable).
#[test]
fn fact_self_recursion_reuses_one_scratch_and_matches_oracle() {
    let env = fact_env();
    assert_eq!(scratch_words(&env.isa, &env.module.funcs[0]).unwrap(), 1);
    let mut h = CallHarness::new(env.clone(), &[true]);
    for n in [0i64, 1, 2, 3, 7, 10, 12, 20] {
        assert_eq!(h.dispatch(0, &[n]), oracle(&env, &[n]), "fact({n})"); // 20! still fits i64
    }
    // Scratch inspection after fact(10): the marshaling writes were [9], [8], …, [1] — all to the
    // one scratch. The LAST writer is the innermost calling activation, fact(2) marshaling [1].
    h.dispatch(0, &[10]);
    let w = h.read_mem_word(h.scratch_addr(0));
    assert_eq!(
        w, 1,
        "scratch holds the INNERMOST activation's marshal — nested reuse is real"
    );
    assert_ne!(
        w, 9,
        "the outermost activation's marshaled [9] was clobbered by its own callees"
    );
}

/// (1) wasm↔T0 tier matrix on mutual NON-tail recursion: the same module runs at all four
/// per-function tier choices — T0→T0, wasm→T0, T0→wasm, wasm→wasm — and every mix is
/// bit-identical to the `run_module` oracle.
#[test]
fn call_tier_matrix_mutual_recursion_matches_oracle() {
    let env = mutual_env();
    for mask in [[false, false], [true, false], [false, true], [true, true]] {
        let mut h = CallHarness::new(env.clone(), &mask);
        for n in [0i64, 1, 2, 3, 10, 25] {
            assert_eq!(
                h.dispatch(0, &[n]),
                oracle(&env, &[n]),
                "mutual n={n} mask={mask:?}"
            );
        }
    }
}

/// (1) The engine's mixed call graph (`Call` + self-`TailCall` + cross-`TailCall`) at ALL 8
/// per-function tier choices — the full wasm-caller→T0-callee / T0-caller→wasm-callee /
/// wasm→wasm matrix in one shape, nested tail-trampolining under an ordinary call included.
#[test]
fn mixed_call_graph_all_tier_choices_match_oracle() {
    let env = mixed_env();
    for bits in 0u32..8 {
        let mask = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
        let mut h = CallHarness::new(env.clone(), &mask);
        for n in [0i64, 1, 5, 300] {
            let got = h.dispatch(0, &[n]);
            assert_eq!(got, 999, "mixed n={n} mask={mask:?}");
            assert_eq!(got, oracle(&env, &[n]), "oracle n={n} mask={mask:?}");
        }
    }
}

/// (2) **The O-8 decisive gate, wasm edition**: 1,000,000-deep self tail recursion through the
/// wasm-compiled countdown completes, correct, in bounded native stack. Boundedness is by
/// construction, visible in the code: every tail step is ONE iteration of `mini_dispatch`'s loop
/// (wasm activation returns → signal consumed → loop re-enters) at constant host-stack depth —
/// the mirror never recurses on a tail signal, exactly like the engine's trampoline loop. A naive
/// recursive lowering (or stub) would blow the host stack orders of magnitude below 1M.
#[test]
fn one_million_deep_tail_recursion_stays_bounded_via_wasm_trampoline() {
    let env = countdown_env();
    let mut h = CallHarness::new(env.clone(), &[true]);
    let t = Instant::now();
    let got = h.dispatch(0, &[1_000_000]);
    let dt = t.elapsed();
    assert_eq!(got, 42);
    assert_eq!(got, oracle(&env, &[1_000_000]));
    eprintln!("1M-deep wasm tail recursion: {dt:?}");
}

/// (3) Mutual tail recursion (`even`/`odd`) at depth 1M — all-wasm, and the two mixed tier
/// splits, where the ONE trampoline loop alternates wasm iterations (engine-cell signal) and T0
/// iterations (`TailReq` signal) every step — the engine's unified-loop discriminant, mirrored.
#[test]
fn mutual_tail_recursion_1m_even_odd_across_tier_splits() {
    let env = parity_env();
    let mut all_wasm = CallHarness::new(env.clone(), &[true, true]);
    assert_eq!(all_wasm.dispatch(0, &[1_000_000]), 1, "even(1M)");
    assert_eq!(all_wasm.dispatch(0, &[1_000_001]), 0, "even(1M+1)");
    assert_eq!(all_wasm.dispatch(0, &[1_000_000]), oracle(&env, &[1_000_000]));
    for mask in [[true, false], [false, true]] {
        let mut h = CallHarness::new(env.clone(), &mask);
        assert_eq!(h.dispatch(0, &[1_000_000]), 1, "even(1M) mask={mask:?}");
    }
}

/// (4) argc boundaries 255/256/257/1000 (§2.5-5): `scratch_words` returns exactly the site's
/// argc, and the marshaling is slot-exact end-to-end (order-sensitive fold over DISTINCT words —
/// any dropped/duplicated/mis-offset slot flips the result), for wasm→wasm, wasm→T0 and T0→wasm.
#[test]
fn argc_boundary_marshaling_is_slot_exact() {
    for n in [255usize, 256, 257, 1000] {
        let env = marshal_env(n);
        assert_eq!(
            scratch_words(&env.isa, &env.module.funcs[0]).unwrap(),
            n,
            "caller n={n}"
        );
        assert_eq!(
            scratch_words(&env.isa, &env.module.funcs[1]).unwrap(),
            0,
            "callee is call-free"
        );
        let args: Vec<Value> = (0..n).map(distinct_word).collect();
        let want = oracle(&env, &args);
        for mask in [[true, true], [true, false], [false, true]] {
            let mut h = CallHarness::new(env.clone(), &mask);
            assert_eq!(h.dispatch(0, &args), want, "n={n} mask={mask:?}");
        }
    }
}

/// (4) argc = 0: no store, no read — the callee's absent parameter stays `fill` on every tier mix
/// (and a 0-word function's `scratch_addr` is never consulted at all).
#[test]
fn zero_argc_call_supplies_nothing_and_callee_sees_fill() {
    let env = zero_argc_env();
    assert_eq!(scratch_words(&env.isa, &env.module.funcs[0]).unwrap(), 0);
    for mask in [[false, false], [true, false], [false, true], [true, true]] {
        let mut h = CallHarness::new(env.clone(), &mask);
        let got = h.dispatch(0, &[]);
        assert_eq!(got, FILL, "mask={mask:?}");
        assert_eq!(got, oracle(&env, &[]), "oracle mask={mask:?}");
    }
}

/// (5, panel #3 lens C finding #2) A **jump-free tail chain** — the one {path}×{Call,TailCall}
/// runtime cell no other test executes: every function here lowers through the STRAIGHT-LINE
/// path (no `JumpTo`/`JumpIf` anywhere), and the tail calls carry argc = 2 and argc = 1 (all
/// other executed `TailCall`s in this suite are dispatcher-path, argc = 1). Shape is the panel's
/// own scratch-verified chain: `main —Call→ f0 —TailCall(argc=2)→ f1 —TailCall(argc=1)→ leaf`,
/// across all 16 per-function tier masks.
#[test]
fn jump_free_tail_chain_covers_straight_line_tailcall_across_all_masks() {
    let (add, sub, mul) = (bin_opcode(3, 0), bin_opcode(3, 1), bin_opcode(3, 2));
    // main(n): res = f0(n, n+3). locals: 0=n 1=block0 2=block1a 3=block1b 4=res 5=tmp.
    let main = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 5, 0, 0), // tmp = 0
            Inst::new(add, 2, 0, 5),     // arg0 = n
            Inst::new(MOV_IMM, 5, 3, 0), // tmp = 3
            Inst::new(add, 3, 0, 5),     // arg1 = n + 3
            Inst::new(MOV_IMM, 1, 1, 0), // callee = f0
            Inst::new(CALL, 4, 1, 2),    // res = f0(n, n+3)
            Inst::new(RET, 4, 0, 0),
        ],
    };
    // f0(a, b): tailcall f1(a+b, a*2). locals: 0=a 1=b 2=block0 3=arg0 4=arg1 5=tmp.
    let f0 = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(add, 3, 0, 1),      // arg0 = a + b
            Inst::new(MOV_IMM, 5, 2, 0),  // tmp = 2
            Inst::new(mul, 4, 0, 5),      // arg1 = a * 2
            Inst::new(MOV_IMM, 2, 2, 0),  // callee = f1
            Inst::new(TAILCALL, 2, 2, 0), // tailcall f1(a+b, a*2) — straight-line, argc = 2
        ],
    };
    // f1(x, y): tailcall leaf(x - y). locals: 0=x 1=y 2=block0 3=arg0.
    let f1 = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(sub, 3, 0, 1),      // arg0 = x - y
            Inst::new(MOV_IMM, 2, 3, 0),  // callee = leaf
            Inst::new(TAILCALL, 2, 1, 0), // tailcall leaf(x - y) — straight-line, argc = 1
        ],
    };
    // leaf(v): v * 31 + 7. locals: 0=v 1=res 2=tmp.
    let leaf = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(MOV_IMM, 2, 31, 0),
            Inst::new(mul, 1, 0, 2),
            Inst::new(MOV_IMM, 2, 7, 0),
            Inst::new(add, 1, 1, 2),
            Inst::new(RET, 1, 0, 0),
        ],
    };
    let env = env_of(vec![main, f0, f1, leaf], 0);
    assert_eq!(scratch_words(&env.isa, &env.module.funcs[1]).unwrap(), 2);
    for bits in 0u32..16 {
        let mask: Vec<bool> = (0..4).map(|i| bits & (1 << i) != 0).collect();
        let mut h = CallHarness::new(env.clone(), &mask);
        for n in [0i64, 1, 5, 300] {
            assert_eq!(h.dispatch(0, &[n]), oracle(&env, &[n]), "n={n} mask={mask:?}");
        }
    }
}

/// A one-function module whose only `Call` targets `callee_word` — a garbage callee (out of
/// range, or negative and sign-extended huge). locals: 0=n 1=block0(callee) 2=block1(arg) 3=res.
fn oob_call_env(callee_word: i32) -> Arc<Env> {
    let caller = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(MOV_IMM, 1, callee_word, 0), // block0 = the garbage callee word
            Inst::new(COPY, 2, 0, 0),              // block1 = n
            Inst::new(CALL, 3, 1, 1),              // res = <garbage>(n)
            Inst::new(RET, 3, 0, 0),
        ],
    };
    env_of(vec![caller], 0)
}

/// The `TailCall` twin of [`oob_call_env`]: the tail signal carries the garbage callee, so the
/// fallback happens one trampoline iteration LATER, at the loop head — not at the call site.
fn oob_tail_env(callee_word: i32) -> Arc<Env> {
    let caller = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(MOV_IMM, 1, callee_word, 0), // block0 = the garbage callee word
            Inst::new(COPY, 2, 0, 0),              // block1 = n
            Inst::new(TAILCALL, 1, 1, 0),          // tailcall <garbage>(n)
        ],
    };
    env_of(vec![caller], 0)
}

/// (7 — W-3's share of panel #3 lens A N1) **Fail-closed OOB-callee tier parity**: a wasm-tier
/// caller whose `Call`/`TailCall` callee word is garbage (out of range, or negative — the sign
/// extension makes a huge `usize`) must get the engine's DEFINED `fill` fallback, bit-identical
/// to what a T0 caller gets for the same program.
///
/// Deliberately NO `run_module` oracle here: `run_module` has no OOB-callee guard at all (it
/// panics in `make_frame` — the O-12 "known gaps" item), so it cannot adjudicate this shape. The
/// canonical semantics for an engine-dispatched call ARE the engine's own: `dispatch`'s loop-head
/// range check returns `fill`, never a panic (otter-engine `ModuleEngine::dispatch`, the
/// `trapped/in_range` loop-head check — mirrored verbatim at [`mini_dispatch`]'s loop head;
/// structurally anchored, like every engine reference in this file since the W-6 re-anchoring —
/// crate doc) — so the test asserts `== FILL` directly and cites that
/// mirror as its ground truth, plus wasm-tier == T0-tier agreement for the parity half. Both
/// entry paths into the check are covered: `Call` reaches it through the dispatch stub
/// (immediately), `TailCall` through the trampoline's NEXT loop iteration (the signal is
/// consumed first, then the range check runs on the garbage callee). The REAL engine's wasm-path
/// run of this same shape is the W-5 E2E gate's job — this suite pins the mirror only.
#[test]
fn oob_or_garbage_callee_word_falls_back_to_fill_on_the_wasm_tier_like_t0() {
    for garbage in [99i32, 1, -1, -1000] {
        for (name, env) in [
            ("call", oob_call_env(garbage)),
            ("tailcall", oob_tail_env(garbage)),
        ] {
            let mut got = Vec::new();
            for mask in [[true], [false]] {
                let mut h = CallHarness::new(env.clone(), &mask);
                let v = h.dispatch(0, &[5]);
                assert_eq!(v, FILL, "{name} garbage={garbage} mask={mask:?}");
                got.push(v);
            }
            assert_eq!(got[0], got[1], "wasm-tier vs T0 parity: {name} garbage={garbage}");
        }
    }
}
