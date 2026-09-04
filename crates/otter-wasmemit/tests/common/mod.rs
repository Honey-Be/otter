//! Shared fixture for the otter-wasmemit tests: a programmatically generated test [`Isa`] covering
//! every lowerable shape (all 11 [`NumTy`]s × Bin ops, Neg, all 121 Convert pairs, the four
//! host-call [`Expr`]s, `Guard`, `Imm`/`ImmF64`/`OperandImm`, `JumpTo`/`JumpIf` since W-1b, and
//! two multi-effect opcodes — W-1a panel A#3 carry-over), plus the call effects (lowered since W-2 — their runtime coverage lives in calls.rs),
//! the native host builtins (the SAME `extern "C"` fns run natively by the oracle and — via
//! `Func::wrap` stubs — inside the wasmtime table), and the T0 oracle (`otter::interp::run_one`).
#![allow(dead_code)] // the wasmemit test crates share this module; none uses every item

use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, NumTy, OpDef, OpHints, UnOp, Value};

// --- host builtins (baked into the IR; the differential harness table-wraps these same fns) ------

pub extern "C" fn h_add7(x: Value) -> Value {
    x.wrapping_add(7)
}
pub extern "C" fn h_mix(x: Value, y: Value) -> Value {
    x.wrapping_mul(3).wrapping_sub(y)
}
pub extern "C" fn hc_ret_ctx(host_ctx: Value, _x: Value) -> Value {
    host_ctx
}
pub extern "C" fn hc_mix(host_ctx: Value, x: Value, y: Value) -> Value {
    host_ctx ^ x.wrapping_add(y.rotate_left(7))
}

// --- opcode map ----------------------------------------------------------------------------------

pub const MOV_IMM: u16 = 0; // local[op0] = sign-extended i32 immediate op1 (OperandImm)
pub const HALT: u16 = 1; // halt with local[op0]
pub const RET: u16 = 2; // return local[op0]
pub const GUARD: u16 = 3; // speculation marker (no-op on non-deopting tiers)
pub const COPY: u16 = 4; // local[op0] = local[op1]
pub const HOST1: u16 = 5; // local[op0] = h_add7(local[op1])
pub const HOST2: u16 = 6; // local[op0] = h_mix(local[op1], local[op2])
pub const HOSTCTX1: u16 = 7; // local[op0] = hc_ret_ctx(host_ctx, local[op1])
pub const HOSTCTX2: u16 = 8; // local[op0] = hc_mix(host_ctx, local[op1], local[op2])
pub const IMM_F64: u16 = 9; // local[op0] = 1.5f64 bits (ImmF64)
pub const IMM_BIG: u16 = 10; // local[op0] = 0x0123_4567_89AB_CDEF (Imm, > i32 range)
pub const JUMP: u16 = 11; // JumpTo op0 — lowered since W-1b (dispatcher loop)
pub const JUMP_IF: u16 = 12; // JumpIf local[op0] -> op1 — lowered since W-1b (dispatcher loop)
pub const CALL: u16 = 13; // Call dest=op0 base=op1 argc=op2 — lowered since W-2 (calls.rs)
pub const TAILCALL: u16 = 14; // TailCall base=op0 argc=op1 — lowered since W-2 (calls.rs)

/// `BIN_BASE + ty_idx*6 + arith_idx`: `local[op0] = local[op1] <op at ty> local[op2]`.
/// Integer `Div` opcodes EXIST in the table (so the §4.6-3 refusal path has a real opcode to
/// exercise) but are never emitted by the fuzz generator and never executed on the oracle.
pub const BIN_BASE: u16 = 15;
/// `NEG_BASE + ty_idx`: `local[op0] = -local[op1]` at the type.
pub const NEG_BASE: u16 = BIN_BASE + 66;
/// `CONV_BASE + from_idx*11 + to_idx`: `local[op0] = convert(local[op1])`.
pub const CONV_BASE: u16 = NEG_BASE + 11;
/// Multi-effect, control LAST (§4.6-4-legal): `local[op0] -= 1; if local[op0] != 0 goto op1` —
/// the `JumpIf` cond reads the register the first effect just wrote (read-after-write ACROSS the
/// effects of one opcode). Also the termination-guard primitive of the CFG fuzz generator.
pub const DEC_JNZ: u16 = CONV_BASE + 121;
/// Multi-effect, non-control (W-1a panel A#3 carry-over): `local[op0] = local[op1] + local[op2];
/// local[op1] = local[op0] * local[op2]` (i64) — the second effect reads the first's fresh write.
pub const MULTI2: u16 = DEC_JNZ + 1;

pub const ALL_TYS: [NumTy; 11] = [
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
pub const ALL_ARITH: [ArithOp; 6] = [
    ArithOp::Add,
    ArithOp::Sub,
    ArithOp::Mul,
    ArithOp::Div,
    ArithOp::Lt,
    ArithOp::Eq,
];

pub fn bin_opcode(ty_idx: usize, arith_idx: usize) -> u16 {
    BIN_BASE + (ty_idx * 6 + arith_idx) as u16
}
pub fn neg_opcode(ty_idx: usize) -> u16 {
    NEG_BASE + ty_idx as u16
}
pub fn conv_opcode(from_idx: usize, to_idx: usize) -> u16 {
    CONV_BASE + (from_idx * 11 + to_idx) as u16
}

fn set0(name: &'static str, value: Expr) -> OpDef {
    OpDef {
        name,
        effects: vec![Effect::SetLocalAt { reg: 0, value }],
        hints: OpHints {
            pure: true,
            terminator: false,
        },
    }
}

fn terminator(name: &'static str, effect: Effect) -> OpDef {
    OpDef {
        name,
        effects: vec![effect],
        hints: OpHints {
            pure: false,
            terminator: true,
        },
    }
}

/// Build the test ISA. Opcode layout MUST match the constants above.
pub fn isa() -> Isa {
    let mut ops = vec![
        set0("mov_imm", Expr::OperandImm(1)),
        terminator(
            "halt",
            Effect::Halt {
                value: Expr::LocalAt(0),
            },
        ),
        terminator(
            "ret",
            Effect::Return {
                value: Expr::LocalAt(0),
            },
        ),
        OpDef {
            name: "guard",
            effects: vec![Effect::Guard {
                cond: Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Lt),
                    Box::new(Expr::LocalAt(0)),
                    Box::new(Expr::LocalAt(1)),
                ),
            }],
            hints: OpHints {
                pure: false,
                terminator: false,
            },
        },
        set0("copy", Expr::LocalAt(1)),
        set0("host1", Expr::Host1(h_add7, Box::new(Expr::LocalAt(1)))),
        set0(
            "host2",
            Expr::Host2(h_mix, Box::new(Expr::LocalAt(1)), Box::new(Expr::LocalAt(2))),
        ),
        set0("hostctx1", Expr::HostCtx1(hc_ret_ctx, Box::new(Expr::LocalAt(1)))),
        set0(
            "hostctx2",
            Expr::HostCtx2(hc_mix, Box::new(Expr::LocalAt(1)), Box::new(Expr::LocalAt(2))),
        ),
        set0("imm_f64", Expr::ImmF64(1.5)),
        set0("imm_big", Expr::Imm(0x0123_4567_89AB_CDEF)),
        terminator("jump", Effect::JumpTo { target: 0 }),
        terminator(
            "jump_if",
            Effect::JumpIf {
                cond: Expr::LocalAt(0),
                target: 1,
            },
        ),
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
        terminator("tailcall", Effect::TailCall { base: 0, argc: 1 }),
    ];
    assert_eq!(ops.len(), BIN_BASE as usize);
    for ty in ALL_TYS {
        for op in ALL_ARITH {
            ops.push(set0(
                "bin",
                Expr::Bin(
                    BinOp::new(ty, op),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::LocalAt(2)),
                ),
            ));
        }
    }
    assert_eq!(ops.len(), NEG_BASE as usize);
    for ty in ALL_TYS {
        ops.push(set0("neg", Expr::Un(UnOp::Neg(ty), Box::new(Expr::LocalAt(1)))));
    }
    assert_eq!(ops.len(), CONV_BASE as usize);
    for from in ALL_TYS {
        for to in ALL_TYS {
            ops.push(set0(
                "conv",
                Expr::Un(UnOp::Convert { from, to }, Box::new(Expr::LocalAt(1))),
            ));
        }
    }
    assert_eq!(ops.len(), DEC_JNZ as usize);
    ops.push(OpDef {
        name: "dec_jnz",
        effects: vec![
            Effect::SetLocalAt {
                reg: 0,
                value: Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Sub),
                    Box::new(Expr::LocalAt(0)),
                    Box::new(Expr::Imm(1)),
                ),
            },
            Effect::JumpIf {
                cond: Expr::LocalAt(0),
                target: 1,
            },
        ],
        hints: OpHints {
            pure: false,
            terminator: true,
        },
    });
    assert_eq!(ops.len(), MULTI2 as usize);
    ops.push(OpDef {
        name: "multi2",
        effects: vec![
            Effect::SetLocalAt {
                reg: 0,
                value: Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Add),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::LocalAt(2)),
                ),
            },
            Effect::SetLocalAt {
                reg: 1,
                value: Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Mul),
                    Box::new(Expr::LocalAt(0)),
                    Box::new(Expr::LocalAt(2)),
                ),
            },
        ],
        hints: OpHints {
            pure: true,
            terminator: false,
        },
    });
    Isa { ops }
}

/// Every function body's decoded operator list (wasmparser `{:?}` strings), in code-section order —
/// promoted here from `aot.rs` at H4 so the opcode-inventory gate reuses the SAME parser as the
/// trampoline-adjacency assertions (no second body-walker to drift).
pub fn all_body_ops(bytes: &[u8]) -> Vec<Vec<String>> {
    use wasmparser::{Parser, Payload};
    let mut bodies = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("parse") {
            let mut ops = Vec::new();
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                ops.push(format!("{:?}", reader.read().expect("operator")));
            }
            bodies.push(ops);
        }
    }
    bodies
}

/// The decoded operator list of function body `j` (for the trampoline-adjacency assertions).
pub fn body_ops(bytes: &[u8], j: usize) -> Vec<String> {
    all_body_ops(bytes).swap_remove(j)
}

/// A [`FuncDef`] with no consts (the common case in these tests).
pub fn func(nparams: usize, nlocals: usize, code: Vec<Inst>) -> FuncDef {
    FuncDef {
        nparams,
        consts: Vec::new(),
        nlocals,
        code,
    }
}

/// The T0 oracle for a dispatch-shaped function (control flow included): `run_one` initializes
/// the frame exactly like the emitted prologue (fill → argc-guarded params → consts) and treats
/// `Halt` and `Return` alike. `dispatch` is never reached because THIS suite's corpus is call-free — the call
/// family has its own `run_module`-oracled suite with a real dispatcher mirror in calls.rs.
pub fn oracle(isa: &Isa, f: &FuncDef, args: &[Value], fill: Value, host_ctx: Value) -> Value {
    let mut tail = otter::interp::TailReq::default();
    otter::interp::run_one(
        isa,
        f,
        args,
        fill,
        host_ctx,
        &mut |_callee, _args| {
            unreachable!("this suite's corpus is call-free (the call family lives in calls.rs)")
        },
        &mut tail,
    )
}

/// Diverse 64-bit slot words: normalized and non-normalized integers, f16/f32/f64 specials —
/// the word set the canonical-eval tests in `otter` use, extended with float bit patterns.
pub const WORDS: [i64; 12] = [
    0,
    1,
    -1,
    i64::MIN,
    i64::MAX,
    0x3C00,                           // 1.0f16
    0x7E01,                           // f16 NaN with payload
    0x8000_0000_u32 as i64,           // -0.0f32 bits (also i32 sign bit)
    0x7FF8_0000_0000_0000_u64 as i64, // f64 NaN
    0x8000_0000_0000_0000_u64 as i64, // -0.0f64 / i64 sign bit
    0x0123_4567_89AB_CDEF_u64 as i64, // arbitrary full-width
    0x3FF8_0000_0000_0000_u64 as i64, // 1.5f64
];

// --- shared differential-fuzz generators (moved here from tests/differential.rs at O-14 A-1 so
// --- the N-in-1 gate can run the IDENTICAL corpus — same generators, same seeds) -----------------

/// Data registers the straight-line fuzz generator hands out.
pub const NREGS: usize = 6;

/// Deterministic LCG — the exact generator the cranelift/baseline differential fuzz uses.
pub fn lcg(seed: u64) -> impl FnMut() -> u32 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    }
}

pub fn rand_word(next: &mut impl FnMut() -> u32) -> i64 {
    // The LCG's `(state >> 33) as u32` yields 31-bit values, so a plain `(hi << 32) | lo` can never
    // set bits 31 or 63 — which silently kept every fuzzed word (fill, host_ctx, consts, args,
    // immediates) non-negative and let a sign-extension mutation slip past all 400 fuzz cases
    // undetected (W-1a panel #2a, lens B finding #2 — only the explicit tests caught it). Rotate by
    // a random amount so every bit position, both sign bits included, gets coverage.
    let w = u64::from(next()) << 32 | u64::from(next());
    w.rotate_left(next() % 64) as i64
}

/// A random straight-line function exercising the whole W-1a surface: optional MOV_IMM prelude
/// (registers it skips start as fill/args), then Bin (all NumTys, integer Div excluded) / Neg /
/// Convert / host calls / Copy / Guard / literals, ending in Halt or Return.
pub fn gen_straightline(next: &mut impl FnMut() -> u32) -> (FuncDef, Vec<Value>, Value, Value) {
    let nparams = (next() % 4) as usize; // 0..=3
    let nconsts = (next() % 3) as usize; // 0..=2 (nparams + nconsts <= 5 < NREGS)
    let consts: Vec<Value> = (0..nconsts).map(|_| rand_word(next)).collect();

    let mut code = Vec::new();
    for r in 0..NREGS {
        if next().is_multiple_of(2) {
            // rand_word (not a raw `next()`) so immediates cover the full i32 range including
            // negatives — the sign-extension path was fuzz-invisible before (panel #2a, B#2).
            code.push(Inst::new(MOV_IMM, r as i32, rand_word(next) as i32, 0));
        }
    }
    for _ in 0..(3 + next() % 12) {
        let dst = (next() % NREGS as u32) as i32;
        let a = (next() % NREGS as u32) as i32;
        let b = (next() % NREGS as u32) as i32;
        match next() % 10 {
            0..=4 => {
                let ti = (next() % 11) as usize;
                let oi = if ALL_TYS[ti].is_float() {
                    (next() % 6) as usize
                } else {
                    // integer: skip Div (§4.6-3 refused; oracle would panic too)
                    [0, 1, 2, 4, 5][(next() % 5) as usize]
                };
                code.push(Inst::new(bin_opcode(ti, oi), dst, a, b));
            }
            5 | 6 => code.push(Inst::new(neg_opcode((next() % 11) as usize), dst, a, 0)),
            7 => code.push(Inst::new(
                conv_opcode((next() % 11) as usize, (next() % 11) as usize),
                dst,
                a,
                0,
            )),
            8 => {
                let op = [HOST1, HOST2, HOSTCTX1, HOSTCTX2][(next() % 4) as usize];
                code.push(Inst::new(op, dst, a, b));
            }
            _ => {
                let op = [COPY, GUARD, IMM_F64, IMM_BIG][(next() % 4) as usize];
                code.push(Inst::new(op, dst, a, 0));
            }
        }
    }
    let ret_reg = (next() % NREGS as u32) as i32;
    code.push(Inst::new(
        if next().is_multiple_of(2) { HALT } else { RET },
        ret_reg,
        0,
        0,
    ));

    let argc = (next() % (nparams as u32 + 3)) as usize; // 0 ..= nparams+2 (under/over both)
    let args: Vec<Value> = (0..argc)
        .map(|_| {
            if next().is_multiple_of(2) {
                WORDS[(next() % WORDS.len() as u32) as usize]
            } else {
                rand_word(next)
            }
        })
        .collect();
    let fill = rand_word(next);
    let host_ctx = rand_word(next);
    (
        FuncDef {
            nparams,
            consts,
            nlocals: NREGS,
            code,
        },
        args,
        fill,
        host_ctx,
    )
}

// --- random CFG fuzz generator (design §8a C#5 — the W-1b gate's mandatory element) ---------------

/// Data registers the CFG generator hands out (r0..r5; params land in the low ones).
pub const CFG_DATA_REGS: u32 = 6;
/// The termination-budget counter register — written ONLY by the guard blocks' `dec_jnz`.
pub const CFG_COUNTER: i32 = 6;
pub const CFG_NLOCALS: usize = 7;

/// A not-yet-laid-out instruction: opcode + operands, one of which may be a symbolic jump target
/// patched once all pcs are known.
struct ProtoInst {
    op: u16,
    operands: [i32; 3],
    patch: Option<(usize, Sym)>,
}

enum Sym {
    /// Start pc of generated block `i`.
    Block(usize),
    /// Start pc of counter-guard block `g`.
    Guard(usize),
}

/// Generate a random CFG whose *shape* is unconstrained — forward/backward/conditional jumps to
/// arbitrary blocks, irreducible forms included — but whose termination is **structurally
/// guaranteed**:
///
/// * the prelude sets a dedicated counter register (r6) to a small `budget`;
/// * block terminators may jump FORWARD directly, but every backward jump request is rerouted
///   through a fresh **guard block** appended after all blocks: `[dec_jnz r6 → target][halt]`;
/// * a block may also end with **no terminator at all** (panel #2b lens B#1): the run then falls
///   into the next block in pc order — the general mid-merge form of the §3 B#3 fall boundary
///   (a non-terminator run reaching a leader of ordinal > 1), which the old always-terminated
///   menu could never produce (the corpus's only fall boundary was prelude→block 0, ordinal 1,
///   so mutating the boundary's ordinal constant to 1 survived the whole fuzz);
/// * nothing else ever writes r6 (the generator only hands out r0..r5 as destinations).
///
/// Termination argument: in the final pc order the ONLY backward edges are the guard blocks'
/// `dec_jnz` jumps (blocks are laid out in index order, so `j > i` targets are forward, the
/// guard blocks sit after everything, so entering one is forward too — and a no-terminator
/// fall-through steps forward to pc+1, so it adds no backward edge either). Any non-terminating
/// execution over a finite pc set would have to traverse some backward edge infinitely often;
/// each such traversal decrements r6, which starts at `budget > 0`, is decremented from positive
/// values only, and diverts to the guard's `halt` the moment it reaches 0. So backward edges are
/// traversed at most `budget` times in total and every execution halts within
/// `(budget + 1) × code_len` steps — on the oracle and in wasm alike. (Backward *requests* are
/// explicitly weighted up in the target draw — panel #2b lens B#3: with a uniform draw 162/300
/// corpus cases never executed a backward edge; the weighting changes which targets are
/// requested, never who may branch backward, so the argument above is untouched.)
///
/// §4.6-cleanliness by construction: the last instruction is always a `halt` (final catch-all or
/// a guard's), so no end fall-through — the catch-all `halt` sits between the last block and the
/// guard blocks precisely so that a last block ending on the no-terminator arm falls onto a
/// `halt`, never off the end; every `JumpIf`/`dec_jnz` has a following instruction, so no
/// trailing `JumpIf`; integer `Div` is excluded from the op menu; and all opcodes put control
/// last. A `Refusal` from the compiler is therefore a generator bug and must fail the test loudly
/// (silently skipping "refused" cases would shrink coverage).
pub fn gen_cfg(next: &mut impl FnMut() -> u32) -> (FuncDef, Vec<Value>, Value, Value) {
    let nparams = (next() % 4) as usize; // 0..=3, all inside the data regs
    let nblocks = 2 + (next() % 6) as usize;
    let budget = 8 + (next() % 25) as i32;

    let mut code: Vec<ProtoInst> = Vec::new();
    let mut guards: Vec<usize> = Vec::new(); // guard g re-enters block guards[g]
    let push = |code: &mut Vec<ProtoInst>, op: u16, operands: [i32; 3], patch: Option<(usize, Sym)>| {
        code.push(ProtoInst { op, operands, patch });
    };

    // prelude: the termination budget
    push(&mut code, MOV_IMM, [CFG_COUNTER, budget, 0], None);

    let mut block_start = vec![0usize; nblocks];
    for (b, start) in block_start.iter_mut().enumerate() {
        *start = code.len();
        // body: 1..=3 random straight-line ops over the data regs
        for _ in 0..(1 + next() % 3) {
            let dst = (next() % CFG_DATA_REGS) as i32;
            let a = (next() % CFG_DATA_REGS) as i32;
            let c = (next() % CFG_DATA_REGS) as i32;
            match next() % 8 {
                0..=2 => {
                    let ti = (next() % 11) as usize;
                    let oi = if ALL_TYS[ti].is_float() {
                        (next() % 6) as usize
                    } else {
                        [0, 1, 2, 4, 5][(next() % 5) as usize] // integer Div excluded (§4.6-3)
                    };
                    push(&mut code, bin_opcode(ti, oi), [dst, a, c], None);
                }
                3 => push(&mut code, neg_opcode((next() % 11) as usize), [dst, a, 0], None),
                4 => push(
                    &mut code,
                    conv_opcode((next() % 11) as usize, (next() % 11) as usize),
                    [dst, a, 0],
                    None,
                ),
                5 => push(
                    &mut code,
                    [HOST1, HOST2, HOSTCTX1, HOSTCTX2][(next() % 4) as usize],
                    [dst, a, c],
                    None,
                ),
                6 => push(&mut code, MULTI2, [dst, a, c], None),
                _ => push(&mut code, MOV_IMM, [dst, rand_word(next) as i32, 0], None),
            }
        }
        // terminator: any target block, any shape — backward requests go through a guard. The
        // target draw is backward-weighted (lens B#3): half the draws are forced into 0..=b (a
        // guard request), the other half stay uniform, so loops actually run instead of most
        // paths halting before ever taking a backward edge.
        let j = if next().is_multiple_of(2) {
            (next() % (b as u32 + 1)) as usize // backward-biased half: j <= b ⇒ guard request
        } else {
            (next() % nblocks as u32) as usize
        };
        let r = (next() % CFG_DATA_REGS) as i32;
        let sym = |guards: &mut Vec<usize>| {
            if j > b {
                Sym::Block(j)
            } else {
                guards.push(j);
                Sym::Guard(guards.len() - 1)
            }
        };
        match next() % 10 {
            0 | 1 => {
                let s = sym(&mut guards);
                push(&mut code, JUMP, [0, 0, 0], Some((0, s)));
            }
            2..=4 => {
                let s = sym(&mut guards);
                push(&mut code, JUMP_IF, [r, 0, 0], Some((1, s)));
            }
            5 if j > b => {
                // forward-only dec_jnz on a DATA reg (a backward one would break the termination
                // argument — only the guards may branch backward)
                push(&mut code, DEC_JNZ, [r, 0, 0], Some((1, Sym::Block(j))));
            }
            5 => push(&mut code, HALT, [r, 0, 0], None),
            6 => push(&mut code, HALT, [r, 0, 0], None),
            7 => push(&mut code, RET, [r, 0, 0], None),
            // no-terminator fall-through (lens B#1): the block just ends and the run continues
            // into the next block — the general §3 B#3 mid-merge boundary. Safe for EVERY block,
            // the last one included: the catch-all halt below sits before the guard blocks, so a
            // falling last block lands on a halt (no FallsOffEnd; clean-by-construction holds).
            _ => {}
        }
    }
    // catch-all halt: gives the last block's conditional terminators a landing pc (⇒ no trailing
    // JumpIf, no end fall-through) and ends the function when no guards follow
    let exit_reg = (next() % CFG_DATA_REGS) as i32;
    push(&mut code, HALT, [exit_reg, 0, 0], None);
    // guard blocks: the ONLY backward branches in the final pc order (see the doc comment)
    let mut guard_start = Vec::with_capacity(guards.len());
    for &j in &guards {
        guard_start.push(code.len());
        push(&mut code, DEC_JNZ, [CFG_COUNTER, 0, 0], Some((1, Sym::Block(j))));
        push(&mut code, HALT, [exit_reg, 0, 0], None);
    }

    // layout is final (1 proto-inst = 1 pc) — patch the symbolic targets
    let insts: Vec<Inst> = code
        .iter()
        .map(|p| {
            let mut o = p.operands;
            if let Some((slot, sym)) = &p.patch {
                o[*slot] = match sym {
                    Sym::Block(j) => block_start[*j] as i32,
                    Sym::Guard(g) => guard_start[*g] as i32,
                };
            }
            Inst::new(p.op, o[0], o[1], o[2])
        })
        .collect();

    let argc = (next() % (nparams as u32 + 3)) as usize; // under- and over-supply both
    let args: Vec<Value> = (0..argc)
        .map(|_| {
            if next().is_multiple_of(2) {
                WORDS[(next() % WORDS.len() as u32) as usize]
            } else {
                rand_word(next)
            }
        })
        .collect();
    let fill = rand_word(next);
    let host_ctx = rand_word(next);
    (
        FuncDef {
            nparams,
            consts: Vec::new(),
            nlocals: CFG_NLOCALS,
            code: insts,
        },
        args,
        fill,
        host_ctx,
    )
}

/// Count the **general mid-merge fall boundaries** in `f`: pcs where a non-terminator run falls
/// into a basic-block leader of ordinal > 1. Leaders are re-derived exactly like the backend does
/// (pc 0 + every jump target + every terminator-successor pc), independently, so this stays an
/// oracle on the generator rather than an echo of the lowering. Panel #2b lens B#1: the old
/// always-terminated menu made this count 0 across the whole 300-case corpus — every fall
/// boundary was prelude→block 0 at ordinal 1, so mutating the boundary's ordinal constant to a
/// literal 1 (M3) survived 700 fuzz cases and was caught by exactly one hand-picked test.
pub fn mid_merge_fall_boundaries(isa: &Isa, f: &FuncDef) -> usize {
    let len = f.code.len();
    let mut is_leader = vec![false; len];
    let mut is_term = vec![false; len];
    is_leader[0] = true;
    for (pc, inst) in f.code.iter().enumerate() {
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::JumpTo { target } | Effect::JumpIf { target, .. } => {
                    is_term[pc] = true;
                    is_leader[inst.operand(*target) as usize] = true;
                }
                Effect::Halt { .. } | Effect::Return { .. } | Effect::TailCall { .. } => is_term[pc] = true,
                Effect::SetLocalAt { .. } | Effect::Guard { .. } | Effect::Call { .. } => {}
            }
        }
        if is_term[pc] && pc + 1 < len {
            is_leader[pc + 1] = true;
        }
    }
    let mut ord = 0usize;
    let mut boundaries = 0usize;
    for pc in 0..len {
        if is_leader[pc] {
            if ord > 1 && pc > 0 && !is_term[pc - 1] {
                boundaries += 1;
            }
            ord += 1;
        }
    }
    boundaries
}
