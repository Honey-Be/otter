//! H1a — the `ModuleEngine` double stack guard (byte-budget primary + depth backstop), gate suite
//! per the H-package design (v2 §H1a):
//!
//! * negative: over-limit recursion → in-process `Err(Trap)` + `trap_reason()` (never an abort),
//!   separately for the budget (defaults) and the backstop (small `depth_limit`), across tiers;
//! * positive: within-limit deep recursion equals the `run_module` oracle at every tier — run on an
//!   explicit `Builder::stack_size` 16 MiB thread (debug-profile flake prevention);
//! * pin (iii): 100,000-deep TAIL recursion at `depth_limit = 8` succeeds at every tier (trampoline
//!   iterations are not activations);
//! * recoverability (O-10, retry-SAME-input form): a guard trap + limit raise + `clear_trap` lets
//!   the IDENTICAL call succeed, for both guards;
//! * pin (ii): an already-trapped engine preserves the FIRST trap's reason/idx;
//! * pin (i): `clear_trap` from the MIDDLE of a live call tree leaves the guard armed AND still
//!   measuring from the ORIGINAL outer stack base (budget-discriminating form — a `clear_trap`
//!   mutant that resets `depth`/`stack_base` re-latches the base at the deep clear point and turns
//!   the expected trap into `Ok`, flipping the assertion) with depth bookkeeping consistent;
//! * mutation subprocess harness: `current_exe` re-exec of env-guarded (`H1_DEPTH_CRASH=1`) hidden
//!   `#[test]`s via `--exact`. In a NORMAL build the two guarded children contain the overflow as a
//!   `Trap` and exit 0 — deleting a guard turns exactly that assertion red (the child dies by
//!   signal/134 instead, without taking the parent test process down). The third child DISABLES both
//!   guards through the public API and must genuinely die by signal/exit-134 — the non-vacuity proof
//!   that the recursion the guarded children survive really is process-fatal without the guard.

// One `unsafe` in this file: `h_observe_and_clear` reconstitutes `&ModuleEngine` from a
// thread-local raw pointer (an `extern "C"` host builtin cannot capture) — same shape as the
// engine's own ctx reconstitution, argued at the fn.
#![allow(unsafe_code)]

use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, Value, interp};
use otter_engine::{ModuleEngine, NEVER, Trap, TrapReason};

use std::cell::{Cell, RefCell};

const EQ0: u16 = 0;
const JUMP_IF: u16 = 1;
const SUB1: u16 = 2;
const SET_IMM: u16 = 3;
const COPY: u16 = 4;
const TAILCALL: u16 = 5;
const CALL: u16 = 6;
const RET: u16 = 7;
#[allow(dead_code)]
const HALT: u16 = 8;
const HOSTCLEAR: u16 = 9;

thread_local! {
    /// The engine under test, for [`h_observe_and_clear`] (the mid-tree `clear_trap` probe). Set by
    /// the one test that uses `HOSTCLEAR`, on the same thread that runs the engine.
    static ENGINE_PTR: Cell<*const ModuleEngine> = const { Cell::new(std::ptr::null()) };
    /// What [`h_observe_and_clear`] saw at its call moment: `(trapped, reason)`.
    static OBSERVED: RefCell<Vec<(bool, TrapReason)>> = const { RefCell::new(Vec::new()) };
}

/// A host builtin that observes the engine's trap state and calls `clear_trap` — from the MIDDLE of
/// a live call tree (it runs under the entry function's own `dispatch()` activation, depth >= 1).
extern "C" fn h_observe_and_clear(_host_ctx: Value, x: Value) -> Value {
    ENGINE_PTR.with(|p| {
        // SAFETY: the pointer is set (on this same thread) to an engine that outlives the whole
        // `try_run` call this builtin runs inside; only a shared reference is formed.
        let eng = unsafe { &*p.get() };
        OBSERVED.with(|o| o.borrow_mut().push((eng.trapped(), eng.trap_reason())));
        eng.clear_trap();
    });
    x
}

fn ctrl(name: &'static str, eff: Effect) -> OpDef {
    OpDef {
        name,
        effects: vec![eff],
        hints: OpHints {
            pure: false,
            terminator: true,
        },
    }
}
fn set(name: &'static str, value: Expr) -> OpDef {
    OpDef {
        name,
        effects: vec![Effect::SetLocalAt { reg: 0, value }],
        hints: OpHints {
            pure: true,
            terminator: false,
        },
    }
}
fn bin(ty: NumTy, op: ArithOp, a: Expr, b: Expr) -> Expr {
    Expr::Bin(BinOp::new(ty, op), Box::new(a), Box::new(b))
}

/// `tailcall.rs`'s test ISA + a `HOSTCLEAR` op (opcode 9) for the mid-tree `clear_trap` gate.
fn test_isa() -> Isa {
    Isa {
        ops: vec![
            set(
                "eq0",
                bin(NumTy::I64, ArithOp::Eq, Expr::LocalAt(1), Expr::Imm(0)),
            ),
            ctrl(
                "jump_if",
                Effect::JumpIf {
                    cond: Expr::LocalAt(0),
                    target: 1,
                },
            ),
            set(
                "sub1",
                bin(NumTy::I64, ArithOp::Sub, Expr::LocalAt(1), Expr::Imm(1)),
            ),
            set("set_imm", Expr::OperandImm(1)),
            set("copy", Expr::LocalAt(1)),
            ctrl("tailcall", Effect::TailCall { base: 0, argc: 1 }),
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
            },
            ctrl(
                "ret",
                Effect::Return {
                    value: Expr::LocalAt(0),
                },
            ),
            ctrl(
                "halt",
                Effect::Halt {
                    value: Expr::LocalAt(0),
                },
            ),
            OpDef {
                name: "hostclear",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::HostCtx1(h_observe_and_clear, Box::new(Expr::LocalAt(1))),
                }],
                hints: OpHints {
                    pure: false,
                    terminator: false,
                },
            },
        ],
    }
}

/// The GENUINELY-NESTING recursion the guards exist for (every step is a full `dispatch()`
/// activation — contrast [`tail_module`]):
/// `rec(n) [idx 0]: if n == 0 { 42 } else { rec(n - 1) }` + an unrelated `leaf(x) [idx 1]: x`.
fn rec_module() -> Module {
    let rec = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),
            Inst::new(JUMP_IF, 1, 6, 0),
            Inst::new(SUB1, 3, 0, 0),    // block[1] = n - 1
            Inst::new(SET_IMM, 2, 0, 0), // block[0] = callee idx 0 (self)
            Inst::new(CALL, 4, 2, 1),    // r4 = rec(n - 1)
            Inst::new(RET, 4, 0, 0),
            Inst::new(SET_IMM, 4, 42, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    };
    let leaf = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 1,
        code: vec![Inst::new(RET, 0, 0, 0)],
    };
    Module {
        funcs: vec![rec, leaf],
        fill: -7,
        host_ctx: 0,
        entry: 0,
    }
}

/// `tailcall.rs`'s countdown: `f(n): if n == 0 { 42 } else { tailcall f(n - 1) }` — every step is a
/// TRAMPOLINE iteration, not a new activation (pin (iii)'s subject).
fn tail_module() -> Module {
    let f = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),
            Inst::new(JUMP_IF, 1, 5, 0),
            Inst::new(SUB1, 3, 0, 0),
            Inst::new(SET_IMM, 2, 0, 0),
            Inst::new(TAILCALL, 2, 1, 0),
            Inst::new(SET_IMM, 4, 42, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    };
    Module {
        funcs: vec![f],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    }
}

/// The three tier shapes every gate below covers: T0-pinned, T1-settled, T2-direct-ish. `(t1, t2,
/// expected tier_of(entry) after a warm run)`.
fn tier_configs() -> [(u32, u32, &'static str); 3] {
    [
        (NEVER, NEVER, "interpreter"),
        (0, NEVER, "baseline"),
        (0, 0, "cranelift"),
    ]
}

/// Run `f` on a thread with an EXPLICIT stack size (the H1a gate requirement: guard behavior must
/// not ride on the test runner's default stack). `join` propagates any assertion panic.
fn with_stack<T: Send + 'static>(bytes: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(bytes)
        .spawn(f)
        .expect("spawn guard-test thread")
        .join()
        .expect("guard-test thread must complete without panicking (and without aborting)")
}

// --- negative gates: the guard fires as an in-process Trap, never an abort ------------------------

#[test]
fn budget_guard_traps_in_process_with_reason_across_tiers() {
    // Default budget (1 MiB), depth backstop pushed out of the way (u32::MAX is a REAL limit, not
    // NEVER — see set_depth_limit's doc) so the PRIMARY guard is what fires. 16 MiB thread: the
    // trap must come from the budget check, with native stack to spare.
    for (t1, t2, tier) in tier_configs() {
        with_stack(16 << 20, move || {
            let isa = test_isa();
            let module = rec_module();
            let oracle_small = interp::run_module(&isa, &module, &[5]);
            let eng = ModuleEngine::new(isa, module, t1, t2);
            eng.set_depth_limit(u32::MAX);
            let r = eng.try_run(&[200_000]);
            assert_eq!(
                r,
                Err(Trap { idx: 0 }),
                "over-budget recursion must trap in-process at ({t1},{t2})"
            );
            assert!(eng.trapped());
            assert_eq!(
                eng.trap_reason(),
                TrapReason::StackBudget,
                "the BYTE BUDGET (primary) must be what fires at defaults ({t1},{t2})"
            );
            // O-10 recoverability: clear + a shallow call on the SAME engine succeeds.
            eng.clear_trap();
            assert_eq!(
                eng.try_run(&[5]),
                Ok(oracle_small),
                "recovered engine at ({t1},{t2})"
            );
            assert!(!eng.trapped());
            assert_eq!(
                eng.tier_of(0),
                tier,
                "entry function settled at the intended tier"
            );
        });
    }
}

#[test]
fn depth_backstop_traps_in_process_with_reason_across_tiers() {
    // Budget pushed out of the way (large), small depth_limit: the BACKSTOP fires, at a depth far
    // below anything that could threaten the 16 MiB thread.
    for (t1, t2, _tier) in tier_configs() {
        with_stack(16 << 20, move || {
            let eng = ModuleEngine::new(test_isa(), rec_module(), t1, t2);
            eng.set_stack_budget(8 << 20);
            eng.set_depth_limit(64);
            let r = eng.try_run(&[200_000]);
            assert_eq!(
                r,
                Err(Trap { idx: 0 }),
                "over-depth recursion must trap in-process at ({t1},{t2})"
            );
            assert_eq!(
                eng.trap_reason(),
                TrapReason::DepthLimit,
                "backstop reason at ({t1},{t2})"
            );
            // Within-limit recursion on the same engine after clear_trap: depth 40 < 64.
            eng.clear_trap();
            assert_eq!(
                eng.try_run(&[40]),
                Ok(42),
                "within-limit recursion recovers at ({t1},{t2})"
            );
        });
    }
}

// --- O-10 recoverability, retry-SAME-input form ----------------------------------------------------

#[test]
fn raising_the_limit_after_clear_trap_makes_the_same_call_succeed() {
    // The H1a/O-10 doc contract verbatim: a guard trap is NOT deterministic-in-the-bytecode — raise
    // the limit + `clear_trap` and the IDENTICAL call genuinely succeeds. One gate per guard, both
    // with the SAME input on the SAME engine (the earlier negative gates only retry smaller inputs).
    with_stack(16 << 20, || {
        // Depth backstop form: rec(100) is 101 nested activations — over a 64 limit, within 4096.
        // Budget pushed out of the way so ONLY the backstop can fire.
        let eng = ModuleEngine::interpreter_only(test_isa(), rec_module());
        eng.set_stack_budget(12 << 20);
        eng.set_depth_limit(64);
        assert_eq!(eng.try_run(&[100]), Err(Trap { idx: 0 }));
        assert_eq!(eng.trap_reason(), TrapReason::DepthLimit);
        eng.set_depth_limit(4096);
        eng.clear_trap();
        assert_eq!(
            eng.try_run(&[100]),
            Ok(42),
            "the SAME call must succeed after raising depth_limit + clear_trap"
        );
        assert!(!eng.trapped());

        // Budget isomorph: 101 activations cannot fit a 64 KiB budget at any build profile
        // (a T0 round-trip alone is ~1.3 KiB in release), but fit 12 MiB easily. Depth stays at
        // its 4096 default so ONLY the byte budget can fire.
        let eng = ModuleEngine::interpreter_only(test_isa(), rec_module());
        eng.set_stack_budget(64 << 10);
        assert_eq!(eng.try_run(&[100]), Err(Trap { idx: 0 }));
        assert_eq!(eng.trap_reason(), TrapReason::StackBudget);
        eng.set_stack_budget(12 << 20);
        eng.clear_trap();
        assert_eq!(
            eng.try_run(&[100]),
            Ok(42),
            "the SAME call must succeed after raising stack_budget + clear_trap"
        );
        assert!(!eng.trapped());
    });
}

// --- positive gate: within-limit deep recursion, every tier == oracle -----------------------------

#[test]
fn deep_but_within_limit_recursion_matches_oracle_across_tiers() {
    // 800 genuine nested activations (default depth_limit 4096 is not in play; budget raised to
    // 12 MiB on a 16 MiB thread so debug-profile frame sizes can't flake the gate).
    for (t1, t2, tier) in tier_configs() {
        with_stack(16 << 20, move || {
            let isa = test_isa();
            let module = rec_module();
            let n = 800i64;
            let oracle = interp::run_module(&isa, &module, &[n]);
            assert_eq!(oracle, 42);
            let eng = ModuleEngine::new(isa, module, t1, t2);
            eng.set_stack_budget(12 << 20);
            for _ in 0..3 {
                assert_eq!(
                    eng.try_run(&[n]),
                    Ok(oracle),
                    "within-limit deep recursion at ({t1},{t2})"
                );
            }
            assert!(!eng.trapped(), "no guard may fire within limits at ({t1},{t2})");
            assert_eq!(
                eng.tier_of(0),
                tier,
                "recursion warmed the entry to the intended tier"
            );
        });
    }
}

// --- pin (iii): proper tail calls are NOT activations ---------------------------------------------

#[test]
fn tail_recursion_100k_deep_succeeds_at_depth_limit_8_across_tiers() {
    // THE pin for "count once per dispatch() entry, never per trampoline iteration": at
    // depth_limit = 8, a 100,000-step tail recursion must complete at EVERY tier. Counting loop
    // iterations (the 'natural' wrong implementation) fails this instantly.
    for (t1, t2, tier) in tier_configs() {
        let isa = test_isa();
        let module = tail_module();
        let oracle = interp::run_module(&isa, &module, &[100_000]);
        assert_eq!(oracle, 42);
        let eng = ModuleEngine::new(isa, module, t1, t2);
        eng.set_depth_limit(8);
        for _ in 0..3 {
            assert_eq!(
                eng.try_run(&[100_000]),
                Ok(oracle),
                "tail recursion must be depth-guard-free at ({t1},{t2})"
            );
        }
        assert!(!eng.trapped());
        assert_eq!(eng.tier_of(0), tier);
    }
}

// --- pin (ii): first trap's reason/idx preserved on an already-trapped engine ---------------------

#[test]
fn already_trapped_engine_preserves_first_reason_and_idx() {
    let eng = ModuleEngine::interpreter_only(test_isa(), rec_module());
    eng.set_depth_limit(16);
    let first = eng.try_run(&[100_000]); // traps: DepthLimit at idx 0
    assert_eq!(first, Err(Trap { idx: 0 }));
    assert_eq!(eng.trap_reason(), TrapReason::DepthLimit);

    // A later call on the poisoned engine — to a DIFFERENT, perfectly valid function — must report
    // the ORIGINAL trap (idx 0, DepthLimit), not idx 1 and not Panic.
    let second = eng.try_dispatch(1, &[9]);
    assert_eq!(
        second,
        Err(Trap { idx: 0 }),
        "original trap idx must be preserved"
    );
    assert_eq!(
        eng.trap_reason(),
        TrapReason::DepthLimit,
        "original reason must be preserved"
    );

    eng.clear_trap();
    assert_eq!(eng.try_dispatch(1, &[9]), Ok(9), "leaf was never poisoned");
    assert_eq!(
        eng.trap_reason(),
        TrapReason::Panic,
        "reason resets to the idle default"
    );
}

// --- pin (i): mid-tree clear_trap leaves the guard armed and the bookkeeping consistent -----------

/// Budget-DISCRIMINATING form. The naive two-`Call` shape (trap, `hostclear`, call again, expect a
/// re-trap) cannot kill a `clear_trap` mutant that resets `depth`/`stack_base`: after such a reset
/// the very next `enter_frame` is a 0→1 transition that RE-LATCHES the base, and an over-LIMIT
/// second recursion still traps identically from the new reference point — the assertion never
/// flips. So discriminate with the BYTE budget and a base that must stay put:
///
/// * `descend(n, m)` [idx 0] recurses `K` levels; at the bottom (depth `K+1`, the whole tree live)
///   it calls `hostclear` (= `clear_trap` MID-TREE) and then the budget-aimed probe `rec(m)`
///   [idx 1];
/// * the budget `B*` is calibrated by bisection on this very engine/module (T0-pinned, so
///   per-activation frames are uniform) to sit midway between "descent alone fits" (`b_desc`) and
///   "descent + probe fits" (`b_full`);
/// * correct impl: `clear_trap` leaves `depth`/`stack_base` alone (RAII `Drop` is the sole
///   decrementer), so the probe keeps measuring from the ORIGINAL outer base — already `K` frames
///   deep — and must trap (`StackBudget`, idx 1 = the probe's own callee index);
/// * reset mutant: the bottom probe entry after the reset re-latches `stack_base` at the deep
///   clear point, so the probe pays only its own `M+1` frames instead of riding on the descent's
///   `K` — two assertions go red, in order (mutation applied + observed on 2026-09-04, then
///   reverted): the CALIBRATION gap collapses first (`b_full == b_desc` — from a re-latched base,
///   completing the probe needs no budget beyond the descent's, so the "healthy gap" assert
///   fails); and were the calibration bypassed with hardcoded budgets, the final gate flips too
///   (probe stays under `B* ≈ b_desc + gap/2` from the new base → `Ok(42)` where `Err` is
///   asserted).
#[test]
fn clear_trap_mid_tree_leaves_guard_armed_and_depth_consistent() {
    const K: i64 = 200; // descent depth (K+1 activations)
    const M: i64 = 100; // probe depth (M+1 activations) — half of K, see the margin math above

    // descend(n, m) [idx 0]: if n == 0 { hostclear; ret rec(m) } else { ret descend(n-1, m) }
    let descend = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 7,
        code: vec![
            Inst::new(EQ0, 2, 0, 0),     // r2 = (n == 0)
            Inst::new(JUMP_IF, 2, 7, 0), // bottom reached -> pc 7
            Inst::new(SET_IMM, 3, 0, 0), // block[0] = self idx 0
            Inst::new(SUB1, 4, 0, 0),    // block[1] = n - 1
            Inst::new(COPY, 5, 1, 0),    // block[2] = m
            Inst::new(CALL, 6, 3, 2),    // r6 = descend(n - 1, m)
            Inst::new(RET, 6, 0, 0),
            Inst::new(HOSTCLEAR, 2, 0, 0), // MID-TREE clear_trap, K+1 activations live
            Inst::new(SET_IMM, 3, 1, 0),   // block[0] = rec idx 1
            Inst::new(COPY, 4, 1, 0),      // block[1] = m
            Inst::new(CALL, 6, 3, 1),      // r6 = rec(m) — the budget-aimed probe
            Inst::new(RET, 6, 0, 0),
        ],
    };
    // rec(n) [idx 1]: the usual countdown recursion (self-callee = 1), returning 42.
    let rec = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),
            Inst::new(JUMP_IF, 1, 6, 0),
            Inst::new(SUB1, 3, 0, 0),
            Inst::new(SET_IMM, 2, 1, 0), // self idx 1
            Inst::new(CALL, 4, 2, 1),
            Inst::new(RET, 4, 0, 0),
            Inst::new(SET_IMM, 4, 42, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    };
    let module = Module {
        funcs: vec![descend, rec],
        fill: -7,
        host_ctx: 0,
        entry: 0,
    };

    // Calibration bracket. 64 MiB thread so the upper budget is genuinely usable stack.
    const LO: usize = 16 << 10;
    const HI: usize = 32 << 20;
    with_stack(64 << 20, move || {
        // T0-pinned: uniform per-activation frames make the bisection meaningful, and HOSTCLEAR
        // must run on the interpreter path with the whole descent live.
        let eng = Box::new(ModuleEngine::interpreter_only(test_isa(), module));
        ENGINE_PTR.with(|p| p.set(&*eng as *const ModuleEngine));

        // One attempt at budget `b` -> (bottom reached?, whole run Ok?). A `hostclear` firing is
        // the bottom-reached witness (it pushes into OBSERVED).
        let attempt = |b: usize| {
            eng.set_stack_budget(b);
            eng.clear_trap();
            OBSERVED.with(|o| o.borrow_mut().clear());
            let ok = eng.try_run(&[K, M]) == Ok(42);
            let bottom = OBSERVED.with(|o| !o.borrow().is_empty());
            (bottom, ok)
        };
        let bisect = |pred: &dyn Fn(usize) -> bool| {
            let (mut lo, mut hi) = (LO, HI); // invariant: !pred(lo) && pred(hi)
            while hi - lo > 4096 {
                let mid = lo + (hi - lo) / 2;
                if pred(mid) { hi = mid } else { lo = mid }
            }
            hi
        };
        assert_eq!(attempt(LO), (false, false), "16 KiB cannot fit the descent");
        assert_eq!(attempt(HI), (true, true), "32 MiB must fit descent + probe");
        let b_desc = bisect(&|b| attempt(b).0); // ~ bytes consumed by the K-deep descent
        let b_full = bisect(&|b| attempt(b).1); // ~ bytes for descent + M-deep probe
        // The gap is the probe's own M+1 frames; a T0 round-trip costs >= ~1.3 KiB even in
        // release, so demand a healthy gap before trusting the midpoint.
        assert!(
            b_full > b_desc + (40 << 10),
            "calibration degenerate: b_desc={b_desc} b_full={b_full}"
        );
        let b_star = b_desc + (b_full - b_desc) / 2;

        // THE gate (see the doc comment): correct impl -> StackBudget trap at the probe's callee
        // idx 1; a depth/base-resetting clear_trap -> Ok(42) -> RED here.
        eng.set_stack_budget(b_star);
        eng.clear_trap();
        OBSERVED.with(|o| o.borrow_mut().clear());
        let r = eng.try_run(&[K, M]);
        ENGINE_PTR.with(|p| p.set(std::ptr::null()));
        assert_eq!(
            r,
            Err(Trap { idx: 1 }),
            "mid-tree clear_trap must leave the budget measuring from the ORIGINAL outer base"
        );
        assert_eq!(eng.trap_reason(), TrapReason::StackBudget);
        // The mid-tree observation: nothing had trapped yet when hostclear ran at the bottom (the
        // clear is exercised on a LIVE, untrapped tree; the reason idles at Panic by contract).
        OBSERVED.with(|o| {
            assert_eq!(
                o.borrow().as_slice(),
                &[(false, TrapReason::Panic)],
                "hostclear must run exactly once, mid-tree, before any trap"
            );
        });

        // After the whole episode, depth bookkeeping is consistent: a generous budget + clear and
        // a shallow probe call works (an over- or under-counted depth would trap early or leak).
        eng.set_stack_budget(HI);
        eng.clear_trap();
        assert_eq!(
            eng.try_dispatch(1, &[10]),
            Ok(42),
            "depth bookkeeping intact after the whole episode"
        );
        assert!(!eng.trapped());
    });
}

// --- mutation subprocess harness (design: current_exe re-exec + env guard + --exact) --------------

/// Hidden child (only meaningful under `H1_DEPTH_CRASH=1`): budget guard ONLY (backstop at the
/// unreachable-real-limit u32::MAX), default 1 MiB budget, 4 MiB thread. In a normal build the
/// recursion TRAPS in-process and this test (= the child process) exits 0; with the budget guard
/// mutated out, the recursion overflows the 4 MiB thread → abort → the parent gate goes red.
#[test]
fn h1_child_budget_guard_traps() {
    if std::env::var("H1_DEPTH_CRASH").as_deref() != Ok("1") {
        return; // harness-only body; a plain `cargo test` run skips the deep recursion
    }
    with_stack(4 << 20, || {
        let eng = ModuleEngine::interpreter_only(test_isa(), rec_module());
        eng.set_depth_limit(u32::MAX);
        let r = eng.try_run(&[1_000_000]);
        assert_eq!(r, Err(Trap { idx: 0 }));
        assert_eq!(eng.trap_reason(), TrapReason::StackBudget);
    });
}

/// Hidden child: depth backstop ONLY (budget effectively out of the way), default 4096 limit, on a
/// thread large enough (128 MiB) that even debug-profile frames keep 4096 activations well within
/// the native stack. Same red-condition as above, for the OTHER guard.
#[test]
fn h1_child_depth_guard_traps() {
    if std::env::var("H1_DEPTH_CRASH").as_deref() != Ok("1") {
        return;
    }
    with_stack(128 << 20, || {
        let eng = ModuleEngine::interpreter_only(test_isa(), rec_module());
        eng.set_stack_budget(usize::MAX);
        let r = eng.try_run(&[1_000_000]);
        assert_eq!(r, Err(Trap { idx: 0 }));
        assert_eq!(eng.trap_reason(), TrapReason::DepthLimit);
    });
}

/// Hidden child: BOTH guards configured out of the way via the public API (`usize::MAX` budget +
/// `u32::MAX` real-but-unreachable depth) on a small 2 MiB thread — the recursion the guarded
/// children survive must now genuinely kill the process (stack-overflow abort). Never returns
/// under H1_DEPTH_CRASH=1.
#[test]
fn h1_child_unguarded_recursion_crashes() {
    if std::env::var("H1_DEPTH_CRASH").as_deref() != Ok("1") {
        return;
    }
    with_stack(2 << 20, || {
        let eng = ModuleEngine::interpreter_only(test_isa(), rec_module());
        eng.set_stack_budget(usize::MAX);
        eng.set_depth_limit(u32::MAX);
        let _ = eng.try_run(&[1_000_000]); // overflows the 2 MiB thread → process abort
        unreachable!("with both guards configured away, the recursion must have aborted");
    });
}

/// The parent gate: re-exec `current_exe` per child with the env guard + `--exact`. Guarded
/// children must exit 0 (Trap contained in-process — a mutated-out guard flips this to a
/// signal/134 death, observed HERE without killing the test run); the unguarded child must die by
/// signal (or exit 134), proving the harness's recursion is genuinely process-fatal without the
/// guards (non-vacuity).
#[test]
#[cfg(unix)]
fn h1a_mutation_harness_guarded_children_trap_and_unguarded_child_aborts() {
    use std::os::unix::process::ExitStatusExt;
    let exe = std::env::current_exe().expect("current_exe");
    let run = |name: &str| {
        std::process::Command::new(&exe)
            .args([name, "--exact", "--test-threads=1"])
            .env("H1_DEPTH_CRASH", "1")
            .output()
            .expect("spawn child test process")
    };

    for name in ["h1_child_budget_guard_traps", "h1_child_depth_guard_traps"] {
        let out = run(name);
        assert!(
            out.status.success(),
            "guarded child `{name}` must contain the overflow as an in-process Trap and exit 0 \
             (a mutated-out guard dies here: code={:?} signal={:?}); stderr:\n{}",
            out.status.code(),
            out.status.signal(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let out = run("h1_child_unguarded_recursion_crashes");
    assert!(
        !out.status.success(),
        "with both guards configured away the child must NOT exit cleanly (vacuous harness otherwise)"
    );
    assert!(
        out.status.signal().is_some() || out.status.code() == Some(134),
        "unguarded child must die of the stack overflow (signal or exit 134), got code={:?} signal={:?}",
        out.status.code(),
        out.status.signal()
    );
}
