//! O-8b/O-8c — proper tail calls through the adaptive `ModuleEngine`.
//!
//! O-8b (T0 only): the decisive gate was that deep tail recursion through the engine's dispatcher
//! completes without NATIVE stack overflow (the trampoline re-enters callees on a flat loop instead of
//! recursing through `dispatch`), while a tail-call-containing function stayed interpreter-only forever
//! (no backend could lower `TailCall`).
//!
//! O-8c-1 (T2 only): Cranelift lowers `TailCall` via the `tail_signal` trampoline-signal helper (see
//! `otter-engine`'s module doc + `.claude-research-library/otter-o8-tailcall-design.md` §8), so a
//! tail-call-containing function CAN promote to T2 — and the SAME boundedness property must hold once
//! compiled (a naive recursive lowering would blow the native stack; the trampoline-signal lowering must
//! not). In THIS phase, T1 (baseline) still refused to lower `TailCall` and a tail-call-containing
//! function could never reach it, at any call count.
//!
//! O-8c-2 (this file, T1 baseline — POLICY COMPLETE): baseline now ALSO lowers `TailCall` via the same
//! `tail_signal` mechanism (hand-x86-64, reusing the existing `Call` arg-marshaling + Return/Halt
//! epilogue). The T1 restriction is fully lifted: a `TailCall`-containing function tiers up through
//! T0 → T1 → T2 exactly like any ordinary function — no tier restriction of any kind remains. This is
//! `[[otter-tier-parity-policy]]`'s completion condition for O-8: feature parity across all three tiers,
//! only speed differs. The tests below that used to assert "never reaches baseline" for a TailCall
//! function have been REPLACED/updated accordingly — that assertion is now factually false by design.

use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, interp};
use otter_engine::{ModuleEngine, NEVER};

const EQ0: u16 = 0;
const JUMP_IF: u16 = 1;
const SUB1: u16 = 2;
const SET_IMM: u16 = 3;
const COPY: u16 = 4;
const TAILCALL: u16 = 5;
const CALL: u16 = 6;
const RET: u16 = 7;
const HALT: u16 = 8;

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
        ],
    }
}

/// f(n): if n == 0 { 42 } else { tailcall f(n - 1) }
fn countdown_module() -> Module {
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

#[test]
fn deep_tail_recursion_stays_bounded_at_t0_when_unpromoted() {
    // T0-pinned via the H5 NEVER contract (`interpreter_only` — the old "thresholds far above the
    // call count" idiom was never a contract, a saturating counter eventually meets any real
    // threshold): pure T0 trampoline coverage — design §6b pick T6's "T0-only" leg of the
    // tier-separated gate. If tail calls recursed through `dispatch` (instead of looping), this
    // overflows the native stack.
    let isa = test_isa();
    let module = countdown_module();
    let eng = ModuleEngine::interpreter_only(isa.clone(), module.clone());
    let oracle = interp::run_module(&isa, &module, &[1_000_000]);
    for _ in 0..3 {
        assert_eq!(eng.run(&[1_000_000]), oracle);
    }
    assert_eq!(
        eng.tier_of(0),
        "interpreter",
        "unpromoted by contract (interpreter_only — H5 NEVER)"
    );
}

/// THE decisive O-8c-1 test: a `TailCall`-containing function actually reaches T2 (Cranelift), and once
/// there, 1,000,000 tail-recursive steps still complete without a native stack overflow — proving the
/// `tail_signal` trampoline-signal lowering (not a naive recursive one) is what got emitted. Cross-checked
/// against the `run_module` oracle for the identical module + args.
#[test]
fn deep_tail_recursion_promotes_to_cranelift_and_stays_bounded() {
    let isa = test_isa();
    let module = countdown_module();
    // Low t2: promotes within the first few trampoline iterations of the very first `run` call (well
    // before the 1,000,000-deep recursion completes), so the vast majority of the run executes compiled.
    let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 3);
    let oracle = interp::run_module(&isa, &module, &[1_000_000]);
    for _ in 0..3 {
        assert_eq!(
            eng.run(&[1_000_000]),
            oracle,
            "engine result must match the run_module oracle"
        );
    }
    assert_eq!(
        eng.tier_of(0),
        "cranelift",
        "a TailCall-containing function must be able to reach T2 (O-8c-1)"
    );
}

/// THE decisive O-8c-2 test: a `TailCall`-containing function actually reaches T1 (baseline), and once
/// there, 1,000,000 tail-recursive steps still complete without a native stack overflow — proving
/// baseline's own `tail_signal` trampoline-signal lowering (not a naive `Call`+`Return` one, which would
/// grow the native stack) is what got emitted. `t2` is set far above the recursion depth so the function
/// settles at T1 and never promotes further. Cross-checked against the `run_module` oracle.
#[test]
fn deep_tail_recursion_promotes_to_baseline_and_stays_bounded() {
    let isa = test_isa();
    let module = countdown_module();
    // Low t1, t2 opted out (H5 NEVER — a real T1 cap, not a "large threshold"): promotes to baseline
    // within the first few trampoline iterations, then can never reach T2, so it settles at T1.
    let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, NEVER);
    let oracle = interp::run_module(&isa, &module, &[1_000_000]);
    for _ in 0..3 {
        assert_eq!(
            eng.run(&[1_000_000]),
            oracle,
            "engine result must match the run_module oracle"
        );
    }
    assert_eq!(
        eng.tier_of(0),
        "baseline",
        "a TailCall-containing function must be able to reach T1 (O-8c-2)"
    );
}

#[test]
fn tail_call_function_can_reach_baseline_now_that_o8c2_lifts_the_restriction() {
    // O-8c-2 policy change: a TailCall-containing function is NO LONGER permanently blocked from T1. This
    // is the direct regression test for that lift — it must actually OBSERVE "baseline" at some call (not
    // merely "eventually reach cranelift", which alone wouldn't distinguish "T1 skipped, straight to T2"
    // from "T1 unrestricted"). t1=2, t2=10 leaves a real window where t1 is crossed but t2 is not yet.
    let isa = test_isa();
    let module = countdown_module();
    let eng = ModuleEngine::new(isa, module, 2, 10);
    let mut saw_baseline = false;
    let mut saw_cranelift = false;
    for k in 0..20 {
        // n=0: exactly one dispatch per call (no tail recursion), so `counts` advances by precisely 1 —
        // fine-grained control over which side of t1/t2 each call lands on.
        assert_eq!(eng.run(&[0]), 42, "call {k}");
        match eng.tier_of(0) {
            "baseline" => saw_baseline = true,
            "cranelift" => saw_cranelift = true,
            _ => {}
        }
    }
    assert!(
        saw_baseline,
        "the function should cross t1 and genuinely reach T1 (baseline) within 20 calls — O-8c-2 must \
         lift the old T1 restriction for TailCall-containing functions"
    );
    assert!(
        saw_cranelift,
        "the function should have crossed t2 and reached T2 within 20 calls"
    );
}

#[test]
fn engine_matches_run_module_oracle() {
    let isa = test_isa();
    let module = countdown_module();
    let oracle = interp::run_module(&isa, &module, &[12345]);
    let eng = ModuleEngine::new(test_isa(), countdown_module(), 2, 5);
    assert_eq!(eng.run(&[12345]), oracle);
    assert_eq!(eng.run(&[0]), interp::run_module(&isa, &module, &[0]));
}

#[test]
fn tail_call_under_a_normal_call_through_engine() {
    // main(x): r = call g(x); halt r.   g(a): tailcall h(a).   h(a): return 99.
    let main = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(SET_IMM, 2, 1, 0), // block[0] = g idx
            Inst::new(COPY, 3, 0, 0),    // block[1] = x
            Inst::new(CALL, 1, 2, 1),    // r = call(base=2, argc=1)
            Inst::new(HALT, 1, 0, 0),
        ],
    };
    let g = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(SET_IMM, 1, 2, 0), // block[0] = h idx
            Inst::new(COPY, 2, 0, 0),    // block[1] = a
            Inst::new(TAILCALL, 1, 1, 0),
        ],
    };
    let h = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 2,
        code: vec![Inst::new(SET_IMM, 1, 99, 0), Inst::new(RET, 1, 0, 0)],
    };
    let module = Module {
        funcs: vec![main, g, h],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    let eng = ModuleEngine::new(test_isa(), module, 2, 5);
    // repeat so main/h/g all climb tiers — all must agree with the (uncompiled) semantics throughout.
    for _ in 0..12 {
        assert_eq!(eng.run(&[5]), 99);
    }
    // g (TailCall-containing) is allowed to reach EITHER compiled tier now — unlike O-8b, where it was
    // permanently interpreter-only. By the time 12 calls have run, it should have climbed all the way to
    // T2 (cranelift takes priority over baseline once both are compiled — see `tier_of`); along the way it
    // will genuinely have passed through T1 (baseline) too (O-8c-2), just not observable at this
    // call-granularity (see `tail_call_function_can_reach_baseline_now_that_o8c2_lifts_the_restriction`
    // for a test that DOES observe the intermediate T1 state).
    assert_eq!(
        eng.tier_of(1),
        "cranelift",
        "g should reach T2 within 12 calls at t2=5 (O-8c-1/O-8c-2)"
    );
}

#[test]
fn tail_call_into_a_compiled_callee() {
    // f(n): if n==0 { tailcall g(7) } else { tailcall f(n-1) }.  g(x): return x*... just return const 500.
    // g is a normal (non-tail) function → it tiers up; a tail call INTO it must still work (bounded), and
    // f itself may now ALSO tier up (through EITHER compiled tier — O-8c-2 removed the last restriction),
    // since f contains only TailCall.
    let f = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5, // 0=n,1=t,2=block0,3=arg,4=unused
        code: vec![
            Inst::new(EQ0, 1, 0, 0),
            Inst::new(JUMP_IF, 1, 5, 0),
            Inst::new(SUB1, 3, 0, 0),
            Inst::new(SET_IMM, 2, 0, 0),  // block[0]=f
            Inst::new(TAILCALL, 2, 1, 0), // tailcall f(n-1)
            Inst::new(SET_IMM, 2, 1, 0),  // pc5: block[0]=g idx (1)
            Inst::new(SET_IMM, 3, 7, 0),  // block[1]=7
            Inst::new(TAILCALL, 2, 1, 0), // tailcall g(7)
        ],
    };
    let g = FuncDef {
        nparams: 1,
        consts: vec![500],
        nlocals: 2, // 0=x, 1=const 500
        code: vec![Inst::new(RET, 1, 0, 0)],
    };
    let module = Module {
        funcs: vec![f, g],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    let oracle = interp::run_module(&test_isa(), &module, &[1000]);
    assert_eq!(oracle, 500);
    let eng = ModuleEngine::new(test_isa(), module, 2, 5);
    for _ in 0..12 {
        assert_eq!(eng.run(&[1000]), 500); // g reached via a tail call, even once g is compiled
    }
    // f's own recursion (n=1000, deep enough to cross t2 within a single run) should reach T2: a compiled
    // (cranelift) tail-caller tail-calling into another compiled (cranelift) callee — the mixed case.
    // (f's count also crosses t1 well before t2 within that same first run, so it genuinely passes through
    // T1/baseline too — O-8c-2 — just not observable at this call-granularity, exactly like the "g" test
    // above; `tail_call_function_can_reach_baseline_now_that_o8c2_lifts_the_restriction` observes it.)
    assert_eq!(eng.tier_of(0), "cranelift", "f should reach T2 (O-8c-1/O-8c-2)");
    assert_eq!(
        eng.tier_of(1),
        "cranelift",
        "g (non-tail, ordinary) should also reach T2"
    );
}

/// A MIXED call graph for the cross-tier differential tests below: `main` makes an ordinary (non-tail)
/// `Call` into `helper`; `helper` tail-recurses on ITSELF (`TailCall`) down to the base case, then
/// tail-calls INTO a third function `leaf` (a cross-function `TailCall`, not a self-tail-call). Exercises
/// `Call` + self-`TailCall` + cross-function-`TailCall` together in one run. The result is always 999
/// regardless of `n` (every path bottoms out at `leaf(999)`), so the SAME expected value works at any
/// recursion depth — useful for driving arbitrarily deep n while keeping the oracle comparison trivial.
///
/// main(n):    r = call helper(n); halt r
/// helper(n):  if n == 0 { tailcall leaf(999) } else { tailcall helper(n - 1) }
/// leaf(x):    return x
fn mixed_module() -> Module {
    let main = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4, // 0=n, 1=block[0]=helper idx, 2=block[1]=n, 3=r(dest)
        code: vec![
            Inst::new(SET_IMM, 1, 1, 0), // block[0] = helper idx (func 1)
            Inst::new(COPY, 2, 0, 0),    // block[1] = n
            Inst::new(CALL, 3, 1, 1),    // r = call(base=1, argc=1) = helper(n)
            Inst::new(HALT, 3, 0, 0),
        ],
    };
    let helper = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4, // 0=n, 1=eq, 2=block[0]=callee, 3=block[1]=(n-1 or 999)
        code: vec![
            Inst::new(EQ0, 1, 0, 0),       // slot1 = (n == 0)
            Inst::new(JUMP_IF, 1, 5, 0),   // if slot1 goto pc5 (base case)
            Inst::new(SUB1, 3, 0, 0),      // slot3 = n - 1
            Inst::new(SET_IMM, 2, 1, 0),   // slot2 = helper idx (1) — self
            Inst::new(TAILCALL, 2, 1, 0),  // tailcall helper(n - 1)
            Inst::new(SET_IMM, 2, 2, 0),   // pc5: slot2 = leaf idx (2)
            Inst::new(SET_IMM, 3, 999, 0), // slot3 = 999
            Inst::new(TAILCALL, 2, 1, 0),  // tailcall leaf(999)
        ],
    };
    let leaf = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 1, // 0 = x
        code: vec![Inst::new(RET, 0, 0, 0)],
    };
    Module {
        funcs: vec![main, helper, leaf],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    }
}

/// O-8c-2's headline requirement (design §8.7's completion condition for O-8): the SAME program produces
/// IDENTICAL results at every tier — the `run_module` oracle, an engine forced to settle at T0
/// (interpreter), one forced to settle at T1 (baseline), and one forced to settle at T2 (cranelift) — with
/// `tier_of` confirming each engine instance actually REACHED its intended tier (never just assuming the
/// threshold worked). Pure self-tail-recursion, deep enough (5,000 steps) to definitively cross every
/// threshold used below within a single `run` call.
#[test]
fn cross_tier_differential_pure_tail_recursion() {
    let isa = test_isa();
    let module = countdown_module();
    let n = 5_000i64;
    let oracle = interp::run_module(&isa, &module, &[n]);
    assert_eq!(oracle, 42);

    // T0: pinned by contract (H5 interpreter_only) — stays interpreted at any call count.
    let t0 = ModuleEngine::interpreter_only(isa.clone(), module.clone());
    assert_eq!(t0.run(&[n]), oracle, "T0 engine result must match the oracle");
    assert_eq!(
        t0.tier_of(0),
        "interpreter",
        "t0-forced engine must have stayed T0"
    );

    // T1: low t1, t2 opted out (H5 NEVER) — settles at baseline.
    let t1 = ModuleEngine::new(isa.clone(), module.clone(), 2, NEVER);
    assert_eq!(t1.run(&[n]), oracle, "T1 engine result must match the oracle");
    assert_eq!(
        t1.tier_of(0),
        "baseline",
        "t1-forced engine must have reached T1 (baseline)"
    );

    // T2: both thresholds low — settles at cranelift.
    let t2 = ModuleEngine::new(isa, module, 2, 3);
    assert_eq!(t2.run(&[n]), oracle, "T2 engine result must match the oracle");
    assert_eq!(
        t2.tier_of(0),
        "cranelift",
        "t2-forced engine must have reached T2 (cranelift)"
    );
}

/// Same headline requirement as [`cross_tier_differential_pure_tail_recursion`], but on the MIXED call
/// graph ([`mixed_module`]): an ordinary `Call` into a function that self-tail-recurses and then
/// tail-calls into a THIRD function. `tier_of` is checked for BOTH `main` (the ordinary caller) and
/// `helper` (the `TailCall`-containing function — the one that would have been restricted pre-O-8c-2) at
/// every forced tier, proving the mixed graph tiers up uniformly with no leftover asymmetry.
#[test]
fn cross_tier_differential_mixed_call_and_tailcall_graph() {
    let isa = test_isa();
    let module = mixed_module();
    let n = 500i64;
    let oracle = interp::run_module(&isa, &module, &[n]);
    assert_eq!(oracle, 999, "every path bottoms out at leaf(999)");

    // 5 top-level calls: `main` gets only ONE dispatch per top-level `run` (a single non-tail Call), so it
    // needs several `run`s to cross a threshold itself; `helper` gets `n + 1` dispatches per top-level
    // call (it tail-recurses through all of n..=0), so it crosses every threshold below on the FIRST call.
    const CALLS: usize = 5;

    // T0: pinned by contract (H5 interpreter_only).
    let t0 = ModuleEngine::interpreter_only(isa.clone(), module.clone());
    for _ in 0..CALLS {
        assert_eq!(t0.run(&[n]), oracle, "T0 engine result must match the oracle");
    }
    assert_eq!(
        t0.tier_of(0),
        "interpreter",
        "t0-forced: main must have stayed T0"
    );
    assert_eq!(
        t0.tier_of(1),
        "interpreter",
        "t0-forced: helper must have stayed T0"
    );

    // T1: low t1, t2 opted out (H5 NEVER) — both functions settle at baseline.
    let t1 = ModuleEngine::new(isa.clone(), module.clone(), 2, NEVER);
    for _ in 0..CALLS {
        assert_eq!(t1.run(&[n]), oracle, "T1 engine result must match the oracle");
    }
    assert_eq!(t1.tier_of(0), "baseline", "t1-forced: main must have reached T1");
    assert_eq!(
        t1.tier_of(1),
        "baseline",
        "t1-forced: helper (TailCall-containing) must reach T1 (O-8c-2)"
    );

    // T2: both thresholds low — both functions settle at cranelift.
    let t2 = ModuleEngine::new(isa, module, 2, 3);
    for _ in 0..CALLS {
        assert_eq!(t2.run(&[n]), oracle, "T2 engine result must match the oracle");
    }
    assert_eq!(t2.tier_of(0), "cranelift", "t2-forced: main must have reached T2");
    assert_eq!(
        t2.tier_of(1),
        "cranelift",
        "t2-forced: helper (TailCall-containing) must reach T2"
    );
}
