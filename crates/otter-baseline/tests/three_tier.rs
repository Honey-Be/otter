//! Integration test: the full **T0 → T1 → T2** adaptive pipeline.
//! Interpreter (T0) → copy-and-patch baseline (T1, `otter-baseline`) → optimizing Cranelift (T2,
//! `otter-cranelift`). The result must be identical at every tier; promotion must happen at the
//! configured thresholds. Also cross-checks baseline == cranelift == interpreter directly.

use otter::engine::Engine;
use otter::interp::{interpret_with, run_module};
use otter::sample;
use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, UnOp};
use otter_baseline::{
    BaselineBackend, compile as baseline_compile, compile_module as baseline_compile_module,
};
use otter_cranelift::{
    CraneliftBackend, compile_fn as cranelift_compile, compile_module as cranelift_compile_module,
};

#[test]
fn t0_t1_t2_promotion_with_identical_results() {
    let mut eng = Engine::new(sample::isa());
    eng.add_tier(Box::new(BaselineBackend), 3); // T1 at 3 calls
    eng.add_tier(Box::new(CraneliftBackend), 10); // T2 at 10 calls
    let f = eng.register(sample::sum_to_program(), 5, 1);

    assert_eq!(eng.level(f), 0);
    assert_eq!(eng.tier_name(f), "interpreter");

    for k in 0..40i64 {
        let n = (k * 5) % 37; // vary the argument
        let r = eng.call(f, &[n]);
        assert_eq!(r, n * (n + 1) / 2, "call k={k} n={n}");

        let c = eng.calls(f);
        let (lvl, name) = if c >= 10 {
            (2, "cranelift")
        } else if c >= 3 {
            (1, "baseline")
        } else {
            (0, "interpreter")
        };
        assert_eq!(eng.level(f), lvl, "level after {c} calls");
        assert_eq!(eng.tier_name(f), name, "tier name after {c} calls");
    }
    assert_eq!(eng.tier_name(f), "cranelift");
}

#[test]
fn all_three_tiers_agree() {
    let isa = sample::isa();
    let prog = sample::sum_to_program();
    let base = baseline_compile(&isa, &prog, 5, 1);
    let opt = cranelift_compile(&isa, &prog, 5, 1);
    for n in [0i64, 1, 5, 42, 1000, 123_456] {
        let oracle = interpret_with(&isa, &prog, 5, &[n]);
        assert_eq!(base.call(&[n]), oracle, "baseline n={n}");
        assert_eq!(opt.call(&[n]), oracle, "cranelift n={n}");
        assert_eq!(oracle, n * (n + 1) / 2);
    }
}

/// The **host-call op** (baked fn pointer in the IR) must produce bit-identical results on the
/// interpreter, the baseline (aligned `call` to the baked address), and Cranelift (`call_indirect`).
/// This is the mechanism the L1 runtime uses to invoke JS-semantic builtins on NaN-boxed words.
#[test]
fn host_call_op_agrees_across_tiers() {
    extern "C" fn fma(a: i64, b: i64) -> i64 {
        a.wrapping_mul(b).wrapping_add(7)
    }
    extern "C" fn twist(a: i64) -> i64 {
        (a.rotate_left(13)) ^ 0x5555_5555
    }

    let h = OpHints {
        pure: false,
        terminator: false,
    };
    let isa = Isa {
        ops: vec![
            // op0: local[op0] = fma(local[op1], local[op2])
            OpDef {
                name: "fma",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::Host2(fma, Box::new(Expr::LocalAt(1)), Box::new(Expr::LocalAt(2))),
                }],
                hints: h,
            },
            // op1: local[op0] = twist(local[op1])
            OpDef {
                name: "twist",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::Host1(twist, Box::new(Expr::LocalAt(1))),
                }],
                hints: h,
            },
            // op2: halt local[op0]
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
        ],
    };
    // r2 = fma(r0, r1); r2 = twist(r2); halt r2
    let prog = vec![
        Inst::new(0, 2, 0, 1),
        Inst::new(1, 2, 2, 0),
        Inst::new(2, 2, 0, 0),
    ];

    let base = baseline_compile(&isa, &prog, 3, 2);
    let opt = cranelift_compile(&isa, &prog, 3, 2);
    for (a, b) in [
        (0i64, 0i64),
        (1, 2),
        (-3, 5),
        (123_456, -789),
        (i64::MAX, 2),
        (i64::MIN, -1),
    ] {
        let oracle = interpret_with(&isa, &prog, 3, &[a, b]);
        assert_eq!(oracle, twist(fma(a, b)), "oracle a={a} b={b}");
        assert_eq!(base.call(&[a, b]), oracle, "baseline a={a} b={b}");
        assert_eq!(opt.call(&[a, b]), oracle, "cranelift a={a} b={b}");
    }
}

/// A minimal ISA with `Call`/`Return` (mirrors `otter`'s own `call_isa`) for testing **module/call
/// lowering** independently of the L1 frontend.
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

/// Module call lowering — BOTH backends (`compile_module`) must equal `run_module` (oracle) on a
/// hand-built recursive module, verifying the codegen at the `otter` layer (independent of the L1
/// frontend): `interp::run_module == baseline::compile_module == cranelift::compile_module`.
#[test]
fn cranelift_module_matches_run_module() {
    const MOV: u16 = 0;
    const MUL_: u16 = 1;
    const SUB_: u16 = 2;
    const LT_: u16 = 3;
    const JIF: u16 = 4;
    const CPY: u16 = 5;
    const CALL: u16 = 6;
    const RET: u16 = 7;
    const HALT_: u16 = 8;

    // fact(n) [func 1]: n < 2 → 1, else n * fact(n-1).
    let fact = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 10,
        code: vec![
            Inst::new(MOV, 1, 2, 0),  // two = 2
            Inst::new(LT_, 2, 0, 1),  // cmp = n < 2
            Inst::new(JIF, 2, 10, 0), // if cmp goto base
            Inst::new(MOV, 3, 1, 0),  // one = 1
            Inst::new(SUB_, 4, 0, 3), // nm1 = n - 1
            Inst::new(MOV, 5, 1, 0),  // callee = func 1
            Inst::new(CPY, 6, 4, 0),  // arg = nm1
            Inst::new(CALL, 7, 5, 1), // res = fact(nm1)
            Inst::new(MUL_, 8, 0, 7), // prod = n * res
            Inst::new(RET, 8, 0, 0),  // return prod
            Inst::new(MOV, 9, 1, 0),  // base: one = 1
            Inst::new(RET, 9, 0, 0),  // return 1
        ],
    };
    let main = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(MOV, 0, 1, 0),
            Inst::new(MOV, 1, 6, 0),
            Inst::new(CALL, 2, 0, 1),
            Inst::new(HALT_, 2, 0, 0),
        ],
    };
    let module = Module {
        funcs: vec![main, fact],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    let isa = call_isa();
    let oracle = run_module(&isa, &module, &[]);
    assert_eq!(oracle, 720);
    assert_eq!(
        cranelift_compile_module(&isa, &module).run(&[]),
        oracle,
        "cranelift module"
    );
    assert_eq!(
        baseline_compile_module(&isa, &module).run(&[]),
        oracle,
        "baseline module"
    );
}

/// Higher-order: a callee passed as an ARGUMENT (through the arg buffer into a parameter slot) and then
/// called — isolates the "callee from a param" path for both module backends, independent of host ops.
#[test]
fn higher_order_module_matches_run_module() {
    const MOV: u16 = 0;
    const MUL_: u16 = 1;
    const CPY: u16 = 5;
    const CALL: u16 = 6;
    const RET: u16 = 7;
    const HALT_: u16 = 8;

    // sq(x) [func 2] = x * x
    let sq = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 2,
        code: vec![Inst::new(MUL_, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
    };
    // ap(f, v) [func 1] = f(v)   (f = local0 raw index, v = local1)
    let ap = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(CPY, 2, 0, 0),  // block[0] = callee = f
            Inst::new(CPY, 3, 1, 0),  // block[1] = arg0 = v
            Inst::new(CALL, 4, 2, 1), // res = f(v)   (dest=4, base=2, argc=1)
            Inst::new(RET, 4, 0, 0),
        ],
    };
    // main() = ap(sq, 5)   (passes sq's raw index 2 as an argument)
    let main = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(MOV, 0, 1, 0),  // callee = ap = func 1
            Inst::new(MOV, 1, 2, 0),  // arg0 = sq = func 2
            Inst::new(MOV, 2, 5, 0),  // arg1 = 5
            Inst::new(CALL, 3, 0, 2), // res = ap(sq, 5)  (dest=3, base=0, argc=2)
            Inst::new(HALT_, 3, 0, 0),
        ],
    };
    let module = Module {
        funcs: vec![main, ap, sq],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    let isa = call_isa();
    let oracle = run_module(&isa, &module, &[]);
    assert_eq!(oracle, 25, "ap(sq, 5) = 5*5");
    assert_eq!(
        cranelift_compile_module(&isa, &module).run(&[]),
        oracle,
        "cranelift higher-order"
    );
    assert_eq!(
        baseline_compile_module(&isa, &module).run(&[]),
        oracle,
        "baseline higher-order"
    );
}

/// O-11: `HostCtx1`/`HostCtx2` in **Table mode** (`compile_module`, both backends) must agree with the
/// oracle for a range of `host_ctx` values including the FULL 64-bit width (`i64::MIN`, values with the
/// top bit set) — not just something that fits a 32-bit immediate, so a baseline register-staging bug
/// (see `emit_call_host_ctx1`/`2`'s doc — the load-operand-then-move-then-immediate ordering) or a
/// truncating/sign-extending mistake on either backend would be caught here. This whole file only
/// compiles with `otter_baseline` linked in (the crate itself is `x86_64`-only), so this test is
/// naturally x86_64-gated — no extra `#[cfg]` needed for the T1 assertions below.
#[test]
fn hostctx_module_matches_oracle_across_tiers_with_extreme_values() {
    extern "C" fn combine_ctx(host_ctx: i64, a: i64, b: i64) -> i64 {
        host_ctx ^ a.wrapping_mul(b).wrapping_add(7)
    }
    extern "C" fn twist_ctx(host_ctx: i64, a: i64) -> i64 {
        host_ctx.rotate_left(13) ^ a
    }
    const CTX2: u16 = 0;
    const CTX1: u16 = 1;
    const HALT_: u16 = 2;

    let h = OpHints {
        pure: false,
        terminator: false,
    };
    let isa = Isa {
        ops: vec![
            OpDef {
                name: "ctx2",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::HostCtx2(
                        combine_ctx,
                        Box::new(Expr::LocalAt(1)),
                        Box::new(Expr::LocalAt(2)),
                    ),
                }],
                hints: h,
            },
            OpDef {
                name: "ctx1",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::HostCtx1(twist_ctx, Box::new(Expr::LocalAt(1))),
                }],
                hints: h,
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
        ],
    };
    // r2 = combine_ctx(host_ctx, a, b); r2 = twist_ctx(host_ctx, r2); halt r2
    let body = vec![
        Inst::new(CTX2, 2, 0, 1),
        Inst::new(CTX1, 2, 2, 0),
        Inst::new(HALT_, 2, 0, 0),
    ];
    let func = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 3,
        code: body,
    };

    for (a, b, host_ctx) in [
        (0i64, 0i64, 0i64),
        (1, 2, 0xDEAD_BEEFi64),
        (-3, 5, i64::MIN),
        (123_456, -789, i64::MIN ^ 1),
        (i64::MAX, 2, -1i64),
        (i64::MIN, -1, i64::MAX),
    ] {
        let module = Module {
            funcs: vec![func.clone()],
            fill: 0,
            host_ctx,
            entry: 0,
        };
        let oracle = run_module(&isa, &module, &[a, b]);
        let want = twist_ctx(host_ctx, combine_ctx(host_ctx, a, b));
        assert_eq!(oracle, want, "oracle a={a} b={b} host_ctx={host_ctx}");
        assert_eq!(
            cranelift_compile_module(&isa, &module).run(&[a, b]),
            oracle,
            "cranelift a={a} b={b} host_ctx={host_ctx}"
        );
        assert_eq!(
            baseline_compile_module(&isa, &module).run(&[a, b]),
            oracle,
            "baseline a={a} b={b} host_ctx={host_ctx}"
        );
    }
}

/// O-11 (panel A/B finding #1): the `otter::engine::Engine` (single-function, `Module`-less tier-up
/// layer — `compile_fn`/`compile`/`compile_resumable`, NOT `ModuleEngine`) has no `Module` to read a
/// `host_ctx` from, so `HostCtx1` is fixed at `host_ctx = 0` on EVERY tier of THIS engine — verified here
/// across the whole T0 -> T1 -> T2 promotion pipeline: all three tiers must agree with EACH OTHER (this
/// is a different word than a `Module` executing the identical `Expr` would see — see `Expr::HostCtx1`'s
/// doc — so this test intentionally does NOT compare against `interp::run_module`).
#[test]
fn engine_hostctx_fixed_at_zero_across_all_tiers() {
    extern "C" fn ctx_probe(host_ctx: i64, x: i64) -> i64 {
        // Returns `x` unchanged iff `host_ctx == 0` (this fn's entire job is to make that observable);
        // any other `host_ctx` corrupts the result, so a plain equality check against the oracle formula
        // below fails loudly if any tier ever bakes a nonzero `host_ctx` here.
        if host_ctx == 0 { x } else { -1 }
    }
    let h = OpHints {
        pure: false,
        terminator: false,
    };
    let isa = Isa {
        ops: vec![
            OpDef {
                name: "ctx1",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::HostCtx1(ctx_probe, Box::new(Expr::LocalAt(1))),
                }],
                hints: h,
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
        ],
    };
    // r1 = HostCtx1(x); halt r1
    let prog = vec![Inst::new(0, 1, 0, 0), Inst::new(1, 1, 0, 0)];

    let mut eng = Engine::new(isa);
    eng.add_tier(Box::new(BaselineBackend), 3);
    eng.add_tier(Box::new(CraneliftBackend), 10);
    let f = eng.register(prog, 2, 1);
    for k in 0..15i64 {
        let x = k % 5;
        let got = eng.call(f, &[x]);
        assert_eq!(got, x, "call k={k} x={x} tier={}", eng.tier_name(f));
    }
    assert_eq!(eng.tier_name(f), "cranelift");
}

// --- W-0.5 (O-13 design §4.1 ④): Un(Neg/Convert) differential coverage ---------------------------
//
// Before W-0.5 the only cross-tier fuzz coverage of ALU ops was i64 `Bin` (4 ops) — which is exactly
// how the `Neg(F16)` three-way divergence (T0 = T1 ≠ T2, all three numerically wrong AND violating
// the NumTy normalization invariant) went unnoticed. These tests pin `Un(Neg)` across the FULL
// 11-`NumTy` width and representative `Un(Convert)` pairs at all three tiers, with the T0
// interpreter as the oracle AND the oracle itself pinned to the canonical `UnOp::eval`.

/// All 11 `NumTy`s in `NumTy::tag` order (== the opcode order of [`neg_isa`]).
const ALL_NUMTYS: [NumTy; 11] = [
    NumTy::I8,
    NumTy::I16,
    NumTy::I32,
    NumTy::I64,
    NumTy::U8,
    NumTy::U16,
    NumTy::U32,
    NumTy::U64,
    NumTy::F16,
    NumTy::F32,
    NumTy::F64,
];

/// opcode of `HALT` in [`neg_isa`] (after the 11 per-type neg ops).
const NEG_ISA_HALT: u16 = 11;

/// Test-local ISA: opcode `t` = `local[op0] = Neg(NumTy::from_tag(t)) local[op1]` for each of the 11
/// types, opcode 11 = halt.
fn neg_isa() -> Isa {
    let mut ops: Vec<OpDef> = ALL_NUMTYS
        .iter()
        .map(|&ty| OpDef {
            name: "neg",
            effects: vec![Effect::SetLocalAt {
                reg: 0,
                value: Expr::Un(UnOp::Neg(ty), Box::new(Expr::LocalAt(1))),
            }],
            hints: OpHints {
                pure: true,
                terminator: false,
            },
        })
        .collect();
    ops.push(OpDef {
        name: "halt",
        effects: vec![Effect::Halt {
            value: Expr::LocalAt(0),
        }],
        hints: OpHints {
            pure: false,
            terminator: true,
        },
    });
    Isa { ops }
}

/// Deterministic LCG over full 64-bit words (same constants as `otter-cranelift`'s fuzz).
fn lcg64(seed: u64) -> impl FnMut() -> i64 {
    let mut state = seed;
    move || {
        let mut step = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        ((u64::from(step()) << 32) | u64::from(step())) as i64
    }
}

/// `Un(Neg)` × all 11 `NumTy`s: baseline (T1) and cranelift (T2) must equal the interpreter (T0)
/// bit-for-bit, and T0 must equal the canonical `UnOp::eval`, on a fixed adversarial word set
/// (full-width, sign-bit-only, float specials, NON-normalized slots) plus 200 fuzzed words per type.
#[test]
fn un_neg_all_numtys_agree_across_tiers() {
    let isa = neg_isa();
    let fixed: [i64; 16] = [
        0,
        1,
        -1,
        i64::MIN,
        i64::MAX,
        0x3C00,                            // 1.0f16
        0x7E01,                            // f16 NaN with payload
        0xFC00,                            // -inf f16
        0x8000_0000u32 as i64,             // -0.0f32 bits
        f32::NAN.to_bits() as i64,         // f32 NaN
        (-0.0f64).to_bits() as i64,        // f64 sign bit only
        f64::NAN.to_bits() as i64,         // f64 NaN
        f64::INFINITY.to_bits() as i64,    // +inf f64
        0x0123_4567_89AB_CDEFu64 as i64,   // arbitrary full-width
        (0xFFFFu64 << 48 | 0x3C00) as i64, // non-normalized f16 slot (garbage high bits)
        0x1_0000,                          // exactly one bit above the low 16
    ];
    let mut next = lcg64(0x0DDB_1A5E_5BAD_5EED);
    for (opc, &ty) in ALL_NUMTYS.iter().enumerate() {
        let prog = vec![Inst::new(opc as u16, 1, 0, 0), Inst::new(NEG_ISA_HALT, 1, 0, 0)];
        let base = baseline_compile(&isa, &prog, 2, 1);
        let opt = cranelift_compile(&isa, &prog, 2, 1);
        let check = |a: i64| {
            let oracle = interpret_with(&isa, &prog, 2, &[a]);
            assert_eq!(
                oracle,
                UnOp::Neg(ty).eval(a),
                "T0 != canonical eval: {ty:?} a={a:#018X}"
            );
            assert_eq!(base.call(&[a]), oracle, "baseline {ty:?} a={a:#018X}");
            assert_eq!(opt.call(&[a]), oracle, "cranelift {ty:?} a={a:#018X}");
        };
        for a in fixed {
            check(a);
        }
        for _ in 0..200 {
            check(next());
        }
    }
}

/// `Neg(F16)` IEEE-754 binary16 reference cases, pinned by VALUE (not just cross-tier agreement) at
/// every tier — the W-0.5 canonical fix's decisive gate. Includes the non-normalized-input
/// truncation contract and the normalization invariant on the result.
#[test]
fn neg_f16_ieee_cases_across_tiers() {
    let isa = neg_isa();
    const NEG_F16: u16 = 8; // NumTy::F16.tag()
    let prog = vec![Inst::new(NEG_F16, 1, 0, 0), Inst::new(NEG_ISA_HALT, 1, 0, 0)];
    let base = baseline_compile(&isa, &prog, 2, 1);
    let opt = cranelift_compile(&isa, &prog, 2, 1);
    let table: [(i64, i64, &str); 5] = [
        (0x3C00, 0xBC00, "1.0 -> -1.0"),
        (0x0000, i64::from(0x8000u16), "+0.0 -> -0.0"),
        (0x7E01, i64::from(0xFE01u16), "NaN payload preserved"),
        (0xFC00, 0x7C00, "-inf -> +inf"),
        // non-normalized slot: the 16 IEEE bits are taken from the low bits, high bits dropped
        (0xFFFF_FFFF_FFFF_3C00u64 as i64, 0xBC00, "truncate non-normalized"),
    ];
    for (a, want, what) in table {
        assert_eq!(interpret_with(&isa, &prog, 2, &[a]), want, "T0 {what}");
        assert_eq!(base.call(&[a]), want, "baseline {what}");
        assert_eq!(opt.call(&[a]), want, "cranelift {what}");
        assert_eq!((want as u64) >> 16, 0, "reference value itself normalized");
    }
}

/// `Un(Convert)` representative pairs (int→float, float→int saturating incl. NaN/±inf, narrowing,
/// f16 both directions, unsigned widths) — all three tiers must agree bit-for-bit with T0.
#[test]
fn un_convert_representatives_agree_across_tiers() {
    use otter::sample::{
        CONV_F16_F64, CONV_F32_F64, CONV_F64_F16, CONV_F64_U8, CONV_I64_F32, CONV_U8_I64, CONV_U64_F64, F2I,
        HALT, I2F,
    };
    let isa = sample::isa();
    let f64b = |x: f64| x.to_bits() as i64;
    let f32b = |x: f32| i64::from(x.to_bits());
    let cases: &[(u16, &[i64])] = &[
        (I2F, &[0, 1, -1, i64::MAX, i64::MIN, 1_000_000]),
        (
            F2I,
            &[
                f64b(0.5),
                f64b(-1.5),
                f64b(f64::NAN),
                f64b(f64::INFINITY),
                f64b(f64::NEG_INFINITY),
                f64b(1e300),
            ],
        ),
        (CONV_I64_F32, &[0, -7, 1 << 40, i64::MIN, i64::MAX]),
        (
            CONV_F32_F64,
            &[f32b(1.5), f32b(f32::NAN), f32b(-0.0), f32b(f32::INFINITY)],
        ),
        (
            CONV_F64_U8,
            &[f64b(300.0), f64b(-5.0), f64b(f64::NAN), f64b(255.9)],
        ),
        (CONV_U8_I64, &[0xFF, 0x1FF, -1]),
        (
            CONV_F64_F16,
            &[f64b(0.1), f64b(65536.0), f64b(-0.0), f64b(f64::NAN), f64b(1e-8)],
        ),
        (
            CONV_F16_F64,
            &[
                0x3C00,
                i64::from(0x8000u16),
                0x7E01,
                0xFC00,
                0xFFFF_3C00u32 as i64,
            ],
        ),
        (CONV_U64_F64, &[-1, i64::MIN, 1, i64::MAX]),
    ];
    for &(op, inputs) in cases {
        let prog = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
        let base = baseline_compile(&isa, &prog, 2, 1);
        let opt = cranelift_compile(&isa, &prog, 2, 1);
        for &a in inputs {
            let oracle = interpret_with(&isa, &prog, 2, &[a]);
            assert_eq!(base.call(&[a]), oracle, "baseline conv op={op} a={a:#018X}");
            assert_eq!(opt.call(&[a]), oracle, "cranelift conv op={op} a={a:#018X}");
        }
    }
}
