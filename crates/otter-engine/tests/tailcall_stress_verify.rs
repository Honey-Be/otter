//! INDEPENDENT verification stress tests for O-8c (design §8, tier-parity policy), written fresh (not
//! copied from `tailcall.rs`) as the final falsification pass on the claim "T1 (baseline) and T2
//! (cranelift) fully support `Effect::TailCall`, at full tier parity with T0". Deliberately exercises
//! shapes the implementation-phase tests did not: (a) mutual tail recursion across two DIFFERENT
//! functions (not self-recursion), (b) a tail call landing on a callee that gets promoted MID-FLIGHT,
//! within the same top-level `run()`, by ordinary (non-tail) `Call`s that precede the final tail call,
//! and (c) an explicit, standalone falsification that TailCall-containing functions reach "baseline".

use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, interp};
use otter_engine::ModuleEngine;

const EQ0: u16 = 0; // local[op0] = (local[op1] == 0)
const JUMP_IF: u16 = 1; // if local[op0] != 0 goto op1
const SUB1: u16 = 2; // local[op0] = local[op1] - 1
const SET_IMM: u16 = 3; // local[op0] = op1 (raw immediate)
const COPY: u16 = 4; // local[op0] = local[op1]
const TAILCALL: u16 = 5;
const CALL: u16 = 6;
const RET: u16 = 7;
const ADD_IMM: u16 = 8; // local[op0] = local[op1] + op1_imm... see below (uses OperandImm-style add)

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
            // local[op0] = local[op1] + 1000  (used by the leaf callee in the mid-flight-promotion test)
            set(
                "add1000",
                bin(NumTy::I64, ArithOp::Add, Expr::LocalAt(1), Expr::Imm(1000)),
            ),
        ],
    }
}

// ===========================================================================================
// (a) Mutual tail recursion between two DIFFERENT functions: is_even <-> is_odd.
// ===========================================================================================

/// is_even(n): if n == 0 { 1 } else { tailcall is_odd(n - 1) }     [func 0]
/// is_odd(n):  if n == 0 { 0 } else { tailcall is_even(n - 1) }    [func 1]
fn mutual_module() -> Module {
    let is_even = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5, // 0=n,1=eq,2=block0(callee),3=block1(n-1),4=base-case result
        code: vec![
            Inst::new(EQ0, 1, 0, 0),      // slot1 = (n == 0)
            Inst::new(JUMP_IF, 1, 5, 0),  // if slot1 goto pc5 (base case)
            Inst::new(SET_IMM, 2, 1, 0),  // slot2 = is_odd idx (func 1)
            Inst::new(SUB1, 3, 0, 0),     // slot3 = n - 1
            Inst::new(TAILCALL, 2, 1, 0), // tailcall is_odd(n - 1)
            Inst::new(SET_IMM, 4, 1, 0),  // pc5: slot4 = 1
            Inst::new(RET, 4, 0, 0),      // return 1
        ],
    };
    let is_odd = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),
            Inst::new(JUMP_IF, 1, 5, 0),
            Inst::new(SET_IMM, 2, 0, 0), // slot2 = is_even idx (func 0)
            Inst::new(SUB1, 3, 0, 0),
            Inst::new(TAILCALL, 2, 1, 0), // tailcall is_even(n - 1)
            Inst::new(SET_IMM, 4, 0, 0),  // pc5: slot4 = 0
            Inst::new(RET, 4, 0, 0),
        ],
    };
    Module {
        funcs: vec![is_even, is_odd],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    }
}

/// (a) + (c): mutual (cross-function) tail recursion, run deep enough to cross every threshold used
/// below WITHIN a single call, at three forced-tier configurations. Directly falsifies the exact gap
/// the tier-parity policy closes: a TailCall-containing function (in fact TWO, calling each other) must
/// reach "baseline" (not just "cranelift"), and must stay correct + bounded doing so.
#[test]
fn mutual_tail_recursion_across_two_functions_cross_tier() {
    let isa = test_isa();
    let module = mutual_module();
    // is_even(n) == 1 iff n is even, matching standard parity. Use a large, ODD depth so both functions
    // are genuinely exercised (each recursion step alternates function identity) and the recursion is
    // deep enough to be a real native-stack-boundedness test if trampolining ever broke.
    let n = 1_000_001i64; // odd
    let oracle = interp::run_module(&isa, &module, &[n]);
    assert_eq!(oracle, 0, "1,000,001 is odd -> is_even returns 0");
    // sanity: an even depth returns 1
    assert_eq!(interp::run_module(&isa, &module, &[1_000_000]), 1);

    // T0: never promotes.
    let t0 = ModuleEngine::new(isa.clone(), module.clone(), 10_000_000, 10_000_000);
    assert_eq!(t0.run(&[n]), oracle);
    assert_eq!(t0.tier_of(0), "interpreter");
    assert_eq!(t0.tier_of(1), "interpreter");

    // T1: both mutually-recursive functions must reach BASELINE — the literal falsification target of
    // O-8c-2 (T1 was permanently blocked for TailCall-containing functions before this policy landed).
    let t1 = ModuleEngine::new(isa.clone(), module.clone(), 2, 10_000_000);
    assert_eq!(t1.run(&[n]), oracle);
    assert_eq!(
        t1.tier_of(0),
        "baseline",
        "is_even (TailCall-containing, mutually recursive) must reach T1"
    );
    assert_eq!(
        t1.tier_of(1),
        "baseline",
        "is_odd (TailCall-containing, mutually recursive) must reach T1"
    );
    // re-run at baseline tier a few more times for good measure — correctness must hold once settled.
    for k in (0i64..20).map(|k| k * 97 + 1) {
        let want = interp::run_module(&isa, &module, &[k]);
        assert_eq!(
            t1.run(&[k]),
            want,
            "mismatch at n={k} while both funcs @ baseline"
        );
    }

    // T2: both should reach cranelift.
    let t2 = ModuleEngine::new(isa, module, 2, 3);
    assert_eq!(t2.run(&[n]), oracle);
    assert_eq!(t2.tier_of(0), "cranelift");
    assert_eq!(t2.tier_of(1), "cranelift");
}

// ===========================================================================================
// (b) Tail call into a callee that gets promoted MID-FLIGHT, driven by ordinary (non-tail) Calls that
// happen earlier in the SAME `run()` invocation, and whose tier at "payoff" time depends on how many
// times `runner` itself (the tail-caller) has already been run — i.e. it depends on how hot runner's
// OWN caller path (repeated top-level `eng.run()` calls) has made things by the time this particular
// call happens.
// ===========================================================================================

/// runner(n):
///   if n == 0 { tailcall leaf(777) }                      // the "payoff" tail call
///   else      { _ = call leaf(n); tailcall runner(n - 1) } // ordinary Call warms `leaf`; self-tailcall
/// leaf(x): return x + 1000
///
/// Every `runner(n)` call always ultimately returns `leaf(777) == 1777`, regardless of `n` — so the
/// oracle value is constant, letting us drive arbitrarily large `n` (to warm `leaf` deep inside a single
/// `run()`) while keeping the correctness check trivial to state.
fn midflight_module() -> Module {
    let runner = FuncDef {
        nparams: 1,
        consts: vec![],
        // 0=n, 1=eq, 2=block0(callee idx, shared slot), 3=block1(arg, shared slot), 4=call-dest(unused)
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),       // slot1 = (n == 0)
            Inst::new(JUMP_IF, 1, 8, 0),   // if slot1 goto pc8 (payoff)
            Inst::new(SET_IMM, 2, 1, 0),   // slot2 = leaf idx (func 1)
            Inst::new(COPY, 3, 0, 0),      // slot3 = n            (ordinary call's arg)
            Inst::new(CALL, 4, 2, 1),      // slot4 = call leaf(n) — result discarded; warms `leaf`
            Inst::new(SET_IMM, 2, 0, 0),   // slot2 = runner idx (func 0) — reuse slot2
            Inst::new(SUB1, 3, 0, 0),      // slot3 = n - 1          (reuse slot3)
            Inst::new(TAILCALL, 2, 1, 0),  // tailcall runner(n - 1)
            Inst::new(SET_IMM, 2, 1, 0),   // pc7 (payoff): slot2 = leaf idx
            Inst::new(SET_IMM, 3, 777, 0), // slot3 = 777
            Inst::new(TAILCALL, 2, 1, 0),  // tailcall leaf(777)  <- the case under test
        ],
    };
    let leaf = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 2, // 0=x, 1=x+1000
        code: vec![Inst::new(ADD_IMM, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
    };
    Module {
        funcs: vec![runner, leaf],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    }
}

/// (b): drives `leaf` through T0 -> T1 -> T2 via the ordinary `Call`s INSIDE a handful of `runner(n)`
/// invocations (each with a decent `n`, so `leaf` gets called `n` times per top-level `run()`, crossing
/// thresholds mid-flight), then confirms: (1) every result is exactly the oracle 1777 regardless of which
/// tier `leaf` happened to be at when the FINAL tail call landed, and (2) `runner` itself — whose own
/// tier depends on how many top-level `run()` calls have accumulated (i.e. how hot ITS OWN caller path
/// is) — correctly tail-calls into `leaf` at every one of `leaf`'s tiers, including the transition calls
/// where `leaf` flips tier partway through `runner`'s own warm-up loop.
#[test]
fn tail_call_lands_on_a_callee_promoted_mid_flight_by_earlier_ordinary_calls() {
    let isa = test_isa();
    let module = midflight_module();
    let oracle = interp::run_module(&isa, &module, &[50]);
    assert_eq!(oracle, 1777);

    // Low t1/t2 relative to n=50: `leaf` (called n=50 times via ordinary Call before the final tailcall,
    // on EVERY run) crosses both thresholds within the very first `run()` call — so by the second call,
    // `leaf` starts already at cranelift, but the FIRST call is the interesting one: `leaf` promotes
    // from interpreter -> baseline -> cranelift DURING the loop, and the very last op (the payoff tail
    // call) must reach it at whatever tier it landed on.
    let eng = ModuleEngine::new(isa.clone(), module.clone(), 2, 6);
    for n in [50i64, 1, 0, 30, 5, 80] {
        let want = interp::run_module(&isa, &module, &[n]);
        assert_eq!(want, 1777);
        assert_eq!(
            eng.run(&[n]),
            1777,
            "runner({n}) must equal the constant oracle 1777"
        );
    }
    // leaf must have reached the top tier by now (many dispatches across the calls above).
    assert_eq!(eng.tier_of(1), "cranelift", "leaf should have promoted to T2");
    // runner itself: each top-level `run()` bumps runner's OWN count by (n+1) dispatches (n self-tail
    // recursions + 1 payoff tailcall-out), all within the SAME loop as far as counts[runner] is
    // concerned — so it too should have crossed both thresholds by now.
    assert_eq!(eng.tier_of(0), "cranelift", "runner should have promoted to T2");

    // Repeat at a threshold pairing that settles BOTH functions at baseline (never crossing t2) — the
    // decisive falsification that this mixed ordinary-Call/tail-call/self-recursion shape ALSO reaches
    // T1, not only T2.
    let eng_t1 = ModuleEngine::new(isa.clone(), module.clone(), 2, 10_000_000);
    for n in [50i64, 1, 0, 30, 5, 80] {
        assert_eq!(eng_t1.run(&[n]), 1777);
    }
    assert_eq!(eng_t1.tier_of(1), "baseline", "leaf should settle at T1");
    assert_eq!(eng_t1.tier_of(0), "baseline", "runner should settle at T1");

    // And a single fresh engine, called ONE AT A TIME with small n so we can observe the exact call at
    // which `leaf`'s tier flips WHILE `runner` tail-calls into it on that very call — the truest
    // "mid-flight, observed transition" version (as opposed to "many calls happened, check the end
    // state"). Each `run(&[2])` dispatches `leaf` exactly 3 times (2 ordinary Calls at n=2,1 + 1 payoff
    // TailCall at n=0), so with t1=5/t2=11: run 1 stays under t1 (interpreter observed), run 2 crosses t1
    // MID-RUN (compile happens between leaf's 2nd and 3rd dispatch, so the payoff tailcall of that very
    // run lands on the freshly-compiled baseline code) and settles there through run 3, run 4 crosses t2
    // mid-run the same way and settles at cranelift for the rest.
    let eng_fine = ModuleEngine::new(isa.clone(), module.clone(), 5, 11);
    let mut saw_leaf_interp = false;
    let mut saw_leaf_baseline = false;
    let mut saw_leaf_cranelift = false;
    for _ in 0..10 {
        assert_eq!(eng_fine.run(&[2]), 1777); // n=2: 2 ordinary calls to leaf + 1 payoff tailcall, per run
        match eng_fine.tier_of(1) {
            "interpreter" => saw_leaf_interp = true,
            "baseline" => saw_leaf_baseline = true,
            "cranelift" => saw_leaf_cranelift = true,
            other => panic!("unknown tier {other}"),
        }
    }
    assert!(saw_leaf_interp, "leaf must have started interpreted");
    assert!(saw_leaf_baseline, "leaf must have passed through baseline");
    assert!(saw_leaf_cranelift, "leaf must have reached cranelift");
}

// ===========================================================================================
// (c) Standalone, minimal falsification: a TailCall-containing function reaches "baseline" specifically
// (isolated from (a)'s mutual-recursion shape and (b)'s mixed-call shape — the simplest possible case).
// ===========================================================================================

/// count(n): if n == 0 { 42 } else { tailcall count(n - 1) }   (pure self tail recursion)
fn simplest_countdown() -> Module {
    let f = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),
            Inst::new(JUMP_IF, 1, 5, 0),
            Inst::new(SET_IMM, 2, 0, 0),
            Inst::new(SUB1, 3, 0, 0),
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
fn tailcall_function_reaches_baseline_tier_specifically_not_only_interpreter_or_cranelift() {
    let isa = test_isa();
    let module = simplest_countdown();
    // t1 low, t2 set so high a single deep call cannot cross it: settles at baseline and STAYS there.
    let eng = ModuleEngine::new(isa.clone(), module.clone(), 1, 50_000_000);
    let oracle = interp::run_module(&isa, &module, &[200_000]);
    assert_eq!(eng.run(&[200_000]), oracle);
    assert_eq!(oracle, 42);
    let tier = eng.tier_of(0);
    assert_eq!(
        tier, "baseline",
        "the literal falsification target of O-8c: TailCall-containing fn must reach baseline, got {tier}"
    );
    assert_ne!(tier, "interpreter");
    assert_ne!(tier, "cranelift");
    // Confirm it isn't a fluke of the specific threshold by trying a slightly different one.
    let eng2 = ModuleEngine::new(isa, module, 5, 1_000_000);
    for _ in 0..10 {
        assert_eq!(eng2.run(&[3]), 42);
    }
    assert_eq!(eng2.tier_of(0), "baseline");
}
