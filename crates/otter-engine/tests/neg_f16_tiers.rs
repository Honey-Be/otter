//! W-0.5 (O-13 design §4.1 ④, ModuleEngine leg): `Neg(F16)` through the adaptive **T0 → T1 → T2**
//! promotion pipeline. Before W-0.5 the three tiers were ALREADY bit-divergent on `Neg(F16)`
//! (T0 = T1 = full 64-bit two's complement, T2 = 16-bit truncating two's complement — all three
//! numerically wrong and T0's own normalization invariant violated), so a function crossing a tier
//! boundary would silently change its observable result. After the canonical fix (IEEE binary16
//! sign flip, `^ 0x8000`), the result must be the SAME correct bits at every tier the engine
//! promotes through — asserted here by pinned VALUE on every single call, not just cross-call
//! agreement.

use otter::{Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, UnOp};
use otter_engine::ModuleEngine;

const NEG_F16: u16 = 0; // local[op0] = Neg(F16) local[op1]
const HALT: u16 = 1; // halt local[op0]

fn isa() -> Isa {
    Isa {
        ops: vec![
            OpDef {
                name: "neg.f16",
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::Un(UnOp::Neg(NumTy::F16), Box::new(Expr::LocalAt(1))),
                }],
                hints: OpHints {
                    pure: true,
                    terminator: false,
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
        ],
    }
}

#[test]
fn neg_f16_bit_identical_across_module_engine_promotion() {
    // entry(x) = -x at f16: r1 = Neg(F16) r0; halt r1
    let entry = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 2,
        code: vec![Inst::new(NEG_F16, 1, 0, 0), Inst::new(HALT, 1, 0, 0)],
    };
    let module = Module {
        funcs: vec![entry],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    // T1 at 2 calls, T2 at 4 calls → the loop below crosses BOTH tier boundaries.
    let eng = ModuleEngine::new(isa(), module, 2, 4);

    // IEEE binary16 reference pairs (bits in, bits out) — incl. NaN payload preservation and the
    // non-normalized-slot truncation contract.
    let table: [(i64, i64); 5] = [
        (0x3C00, 0xBC00),                                    // 1.0 -> -1.0
        (0x0000, i64::from(0x8000u16)),                      // +0.0 -> -0.0
        (0x7E01, i64::from(0xFE01u16)),                      // NaN payload preserved
        (0xFC00, 0x7C00),                                    // -inf -> +inf
        (0xFFFF_FFFF_FFFF_3C00u64 as i64, 0xBC00u16 as i64), // non-normalized -> truncated
    ];

    let mut seen = Vec::new();
    for k in 0..12usize {
        let (a, want) = table[k % table.len()];
        let tier = eng.tier_of(0);
        let got = eng.run(&[a]);
        assert_eq!(
            got, want,
            "call {k} at tier {tier}: Neg(F16)({a:#X}) must be the IEEE bits"
        );
        seen.push(tier);
    }
    // The pipeline must actually have crossed the tier boundaries (thresholds 2 and 4).
    assert!(seen.contains(&"interpreter"), "T0 exercised: {seen:?}");
    assert!(seen.contains(&"baseline"), "T1 exercised: {seen:?}");
    assert_eq!(eng.tier_of(0), "cranelift", "must end at T2: {seen:?}");
}
