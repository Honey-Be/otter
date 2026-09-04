//! §8(a) native differential harness (design `otter-o13-wasm-emit-design.md`): run the emitted
//! module bytes on wasmtime against a host-built mock of the deployment environment — a
//! host-created linear memory + funcref table with the canonical helpers (`otter_binop`/
//! `otter_convert`/`otter_unop`) and the test host builtins installed at harness-chosen table
//! slots — and compare every result **bit-for-bit** against the T0 oracle
//! ([`otter::interp::run_one`]).
//!
//! The same index mapping the harness installs is what it hands the backend ([`HelperIdx`] + the
//! `resolve` closure) — exercising the §2.3 index-externalization for real: the backend never
//! learns a native address, only table slots.

mod common;

use common::{
    ALL_ARITH, ALL_TYS, COPY, DEC_JNZ, GUARD, HALT, HOST1, HOST2, HOSTCTX1, HOSTCTX2, IMM_BIG, IMM_F64, JUMP,
    JUMP_IF, MOV_IMM, MULTI2, WORDS, bin_opcode, conv_opcode, func, gen_cfg, gen_straightline, lcg,
    mid_merge_fall_boundaries, neg_opcode, oracle,
};
use otter::{FuncDef, Inst, Isa, NumTy, Value};
use otter_wasmemit::{HelperIdx, compile_dispatch_fn};
use wasmtime::{
    Config, Engine, Func, Instance, Memory, MemoryType, Module, Ref, RefType, Store, Table, TableType,
    TypedFunc,
};

// --- harness-chosen table layout (slot 0 stays null — the design's null index) -------------------
const BINOP_SLOT: u32 = 3;
const CONVERT_SLOT: u32 = 5;
const UNOP_SLOT: u32 = 7;
/// Never called in THIS suite (its corpus is call-free; the W-2 call family has its own harness
/// in `tests/calls.rs`) — deliberately left as null slots so any stray dispatch/tail call a
/// lowering bug emitted would trap loudly instead of silently succeeding.
const DISPATCH_SLOT: u32 = 11;
const TAIL_SLOT: u32 = 12;
const H_ADD7_SLOT: u32 = 13;
const H_MIX_SLOT: u32 = 17;
const HC_RET_CTX_SLOT: u32 = 19;
const HC_MIX_SLOT: u32 = 23;
const TABLE_MIN: u64 = 32;

/// Where in the mock linear memory the harness marshals arguments (8-byte aligned).
const ARGS_ADDR: usize = 1024;
/// The `scratch_addr` passed for this suite's (all call-free ⇒ `scratch_words == 0`) functions:
/// deliberately OUT OF BOUNDS of the 1-page mock memory — a call-free function never bakes the
/// address into anything (see `compile_dispatch_fn`'s doc), so any store a lowering bug emitted
/// through it would trap loudly here instead of silently scribbling on live test memory.
const SCRATCH_UNUSED: u32 = 0x00DE_AD00;
/// A junk ctx word: the emitted code must forward it opaquely at most — never dereference it.
const CTX_JUNK: i32 = 0x7EAD;

fn helper_idx() -> HelperIdx {
    HelperIdx {
        dispatch: DISPATCH_SLOT,
        tail: TAIL_SLOT,
        binop: BINOP_SLOT,
        convert: CONVERT_SLOT,
        unop: UNOP_SLOT,
    }
}

/// Host-fn pointer (as baked in the IR) → the harness's table slot for its wasm stub. The wasm
/// deployment's equivalent is the identity (fn pointer IS the table index — §2 measured).
fn resolve(p: usize) -> Option<u32> {
    let map: [(usize, u32); 4] = [
        (common::h_add7 as otter::HostFn1 as usize, H_ADD7_SLOT),
        (common::h_mix as otter::HostFn2 as usize, H_MIX_SLOT),
        (common::hc_ret_ctx as otter::HostFnCtx1 as usize, HC_RET_CTX_SLOT),
        (common::hc_mix as otter::HostFnCtx2 as usize, HC_MIX_SLOT),
    ];
    map.iter().find(|(a, _)| *a == p).map(|(_, i)| *i)
}

struct Harness {
    engine: Engine,
    store: Store<()>,
    memory: Memory,
    table: Table,
}

impl Harness {
    fn new() -> Harness {
        // Fuel metering (panel #2b lens B#2): without it, a control-flow lowering bug that turns
        // into a wasm infinite loop is a CI HANG (panel-demonstrated: a mutated backward edge span
        // 60s+ with no failure), not a red test. With fuel, the runaway execution traps on fuel
        // exhaustion and fails LOUDLY through the existing `expect("emitted f must not trap")`
        // path. The budget is far above anything a legitimate test case needs (the CFG fuzz's
        // termination argument bounds runs at (budget+1) x code_len steps — thousands, not
        // millions), so a fuel trap always means a lowering/generator bug.
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).expect("engine with fuel metering");
        let mut store = Store::new(&engine, ());
        store.set_fuel(100_000_000).expect("set_fuel");
        let memory = Memory::new(&mut store, MemoryType::new(1, None)).expect("memory");
        let table = Table::new(
            &mut store,
            TableType::new(RefType::FUNCREF, u32::try_from(TABLE_MIN).unwrap(), None),
            Ref::Func(None),
        )
        .expect("table");

        // Helper stubs: §2.3 signatures exactly; bodies delegate to the canonical otter helpers —
        // the same code T0 uses, so the oracle comparison is apples-to-apples.
        let binop = Func::wrap(&mut store, |a: i64, b: i64, ty: u32, op: u32| -> i64 {
            otter::otter_binop(a, b, ty, op)
        });
        let convert = Func::wrap(&mut store, |a: i64, from: u32, to: u32| -> i64 {
            otter::otter_convert(a, from, to)
        });
        let unop = Func::wrap(&mut store, |a: i64, ty: u32, op: u32| -> i64 {
            otter::otter_unop(a, ty, op)
        });
        // Host-builtin stubs: wrap the SAME extern "C" fns the oracle calls natively.
        let h1 = Func::wrap(&mut store, |x: i64| -> i64 { common::h_add7(x) });
        let h2 = Func::wrap(&mut store, |x: i64, y: i64| -> i64 { common::h_mix(x, y) });
        let hc1 = Func::wrap(&mut store, |c: i64, x: i64| -> i64 { common::hc_ret_ctx(c, x) });
        let hc2 = Func::wrap(&mut store, |c: i64, x: i64, y: i64| -> i64 {
            common::hc_mix(c, x, y)
        });

        for (slot, f) in [
            (BINOP_SLOT, binop),
            (CONVERT_SLOT, convert),
            (UNOP_SLOT, unop),
            (H_ADD7_SLOT, h1),
            (H_MIX_SLOT, h2),
            (HC_RET_CTX_SLOT, hc1),
            (HC_MIX_SLOT, hc2),
        ] {
            table
                .set(&mut store, u64::from(slot), Ref::Func(Some(f)))
                .expect("table.set");
        }
        Harness {
            engine,
            store,
            memory,
            table,
        }
    }

    fn compile(&self, isa: &Isa, f: &FuncDef, fill: Value, host_ctx: Value) -> Vec<u8> {
        compile_dispatch_fn(isa, 0, f, fill, host_ctx, helper_idx(), &resolve, SCRATCH_UNUSED)
            .expect("lowerable (call-free, §4.6-clean) function must compile")
    }

    fn instantiate(&mut self, bytes: &[u8]) -> TypedFunc<(i32, i32, i64), i64> {
        let module = Module::new(&self.engine, bytes).expect("emitted module must validate");
        let instance = Instance::new(&mut self.store, &module, &[self.memory.into(), self.table.into()])
            .expect("instantiate against otter.memory/otter.table");
        instance
            .get_typed_func::<(i32, i32, i64), i64>(&mut self.store, "f")
            .expect("export \"f\" of the CompiledFn type")
    }

    /// Marshal `args` into the mock memory and call `f(ctx, args, argc)`. `argc` may be smaller
    /// than `args.len()` (the extra words stay in memory, unread if the argc-guard works).
    fn call(&mut self, f: &TypedFunc<(i32, i32, i64), i64>, args: &[Value], argc: usize) -> Value {
        assert!(argc <= args.len(), "harness misuse: argc beyond marshalled args");
        // Re-arm the fuel budget per call: fuel is consumed cumulatively per Store, and one
        // Harness runs thousands of calls (the fuzz corpora) — re-arming keeps every call's
        // budget identical and independent of how many cases ran before it.
        self.store.set_fuel(100_000_000).expect("set_fuel");
        let mut buf = Vec::with_capacity(args.len() * 8);
        for a in args {
            buf.extend_from_slice(&a.to_le_bytes());
        }
        self.memory
            .write(&mut self.store, ARGS_ADDR, &buf)
            .expect("write args");
        f.call(
            &mut self.store,
            (CTX_JUNK, i32::try_from(ARGS_ADDR).unwrap(), argc as i64),
        )
        .expect("emitted f must not trap")
    }

    /// Compile once, instantiate once, then differentially check every arg set (argc = len).
    fn check_all(&mut self, isa: &Isa, f: &FuncDef, fill: Value, host_ctx: Value, argsets: &[&[Value]]) {
        let bytes = self.compile(isa, f, fill, host_ctx);
        let tf = self.instantiate(&bytes);
        for args in argsets {
            let got = self.call(&tf, args, args.len());
            let want = oracle(isa, f, args, fill, host_ctx);
            assert_eq!(
                got, want,
                "wasm≠oracle: args={args:?} fill={fill:#x} host_ctx={host_ctx:#x}\nfunc={f:?}"
            );
        }
    }

    /// Differential check with `argc` decoupled from the marshalled words (argc-boundary probe).
    /// Returns the (oracle-equal) result so callers can additionally pin expected literals.
    fn check_argc(
        &mut self,
        isa: &Isa,
        f: &FuncDef,
        fill: Value,
        host_ctx: Value,
        args: &[Value],
        argc: usize,
    ) -> Value {
        let bytes = self.compile(isa, f, fill, host_ctx);
        let tf = self.instantiate(&bytes);
        let got = self.call(&tf, args, argc);
        let want = oracle(isa, f, &args[..argc], fill, host_ctx);
        assert_eq!(got, want, "wasm≠oracle: args={args:?} argc={argc}\nfunc={f:?}");
        got
    }
}

const FILL: Value = 0x5EED_F00D_5EED_F00D_u64 as Value;
const HOST_CTX: Value = 0x7E57_C0DE_7E57_C0DE_u64 as Value;

// --- explicit cases ------------------------------------------------------------------------------

/// Every Bin op at every NumTy (minus refused integer Div) over the diverse word set — the emitted
/// `call_indirect otter_binop` path must be bit-identical to T0's `BinOp::eval`.
#[test]
fn bin_all_numtys_match_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    for (ti, ty) in ALL_TYS.iter().enumerate() {
        for (oi, op) in ALL_ARITH.iter().enumerate() {
            if !ty.is_float() && *op == otter::ArithOp::Div {
                continue; // §4.6-3: refused — covered by the refusal tests
            }
            let f = func(
                2,
                3,
                vec![Inst::new(bin_opcode(ti, oi), 2, 0, 1), Inst::new(HALT, 2, 0, 0)],
            );
            let bytes = h.compile(&isa, &f, FILL, HOST_CTX);
            let tf = h.instantiate(&bytes);
            for &a in &WORDS {
                for &b in &WORDS {
                    let got = h.call(&tf, &[a, b], 2);
                    let want = oracle(&isa, &f, &[a, b], FILL, HOST_CTX);
                    assert_eq!(got, want, "{ty:?} {op:?} a={a:#x} b={b:#x}");
                }
            }
        }
    }
}

/// `Un(Neg)` at every NumTy through `otter_unop`, and ALL 121 Convert pairs through
/// `otter_convert`, over the diverse word set.
#[test]
fn neg_and_convert_all_numtys_match_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    for (ti, ty) in ALL_TYS.iter().enumerate() {
        let f = func(
            1,
            2,
            vec![Inst::new(neg_opcode(ti), 1, 0, 0), Inst::new(HALT, 1, 0, 0)],
        );
        let bytes = h.compile(&isa, &f, FILL, HOST_CTX);
        let tf = h.instantiate(&bytes);
        for &a in &WORDS {
            let got = h.call(&tf, &[a], 1);
            let want = oracle(&isa, &f, &[a], FILL, HOST_CTX);
            assert_eq!(got, want, "neg {ty:?} a={a:#x}");
        }
    }
    for (fi, from) in ALL_TYS.iter().enumerate() {
        for (ti, to) in ALL_TYS.iter().enumerate() {
            let f = func(
                1,
                2,
                vec![Inst::new(conv_opcode(fi, ti), 1, 0, 0), Inst::new(HALT, 1, 0, 0)],
            );
            let bytes = h.compile(&isa, &f, FILL, HOST_CTX);
            let tf = h.instantiate(&bytes);
            for &a in &WORDS {
                let got = h.call(&tf, &[a], 1);
                let want = oracle(&isa, &f, &[a], FILL, HOST_CTX);
                assert_eq!(got, want, "conv {from:?}->{to:?} a={a:#x}");
            }
        }
    }
}

/// The W-0.5 IEEE binary16 anchor bits, end-to-end through the emitted module: sign flip (NaN
/// payload preserved, non-normalized words truncated) and helper-rounded f16 arithmetic.
#[test]
fn f16_ieee_anchor_cases() {
    let isa = common::isa();
    let mut h = Harness::new();
    let f16_ti = 8; // ALL_TYS[8] == F16
    assert_eq!(ALL_TYS[f16_ti], NumTy::F16);

    let neg = func(
        1,
        2,
        vec![Inst::new(neg_opcode(f16_ti), 1, 0, 0), Inst::new(HALT, 1, 0, 0)],
    );
    let bytes = h.compile(&isa, &neg, FILL, HOST_CTX);
    let tf = h.instantiate(&bytes);
    let anchors: [(i64, i64, &str); 5] = [
        (0x3C00, 0xBC00, "1.0 -> -1.0"),
        (0x0000, 0x8000, "+0.0 -> -0.0"),
        (0x7E01, 0xFE01, "NaN payload preserved"),
        (0xFC00, 0x7C00, "-inf -> +inf"),
        (
            0x7FFF_FFFF_FFFF_3C00,
            0xBC00,
            "non-normalized word truncates first",
        ),
    ];
    for (a, want_bits, what) in anchors {
        let got = h.call(&tf, &[a], 1);
        assert_eq!(got, want_bits, "{what}: neg_f16({a:#x})");
        assert_eq!(got, oracle(&isa, &neg, &[a], FILL, HOST_CTX), "{what}: oracle");
    }

    // f16 add: 1.0 + 1.0 = 2.0 (0x4000); overflow rounds to +inf per the canonical helper.
    let add = func(
        2,
        3,
        vec![
            Inst::new(bin_opcode(f16_ti, 0), 2, 0, 1),
            Inst::new(HALT, 2, 0, 0),
        ],
    );
    let bytes = h.compile(&isa, &add, FILL, HOST_CTX);
    let tf = h.instantiate(&bytes);
    assert_eq!(h.call(&tf, &[0x3C00, 0x3C00], 2), 0x4000, "1.0f16 + 1.0f16");
    assert_eq!(
        h.call(&tf, &[0x7BFF, 0x7BFF], 2),
        oracle(&isa, &add, &[0x7BFF, 0x7BFF], FILL, HOST_CTX),
        "f16 max + max"
    );

    // convert f64 -> f16: 1.5 is exact (0x3E00).
    let conv = func(
        1,
        2,
        vec![
            Inst::new(conv_opcode(10, f16_ti), 1, 0, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    );
    let bytes = h.compile(&isa, &conv, FILL, HOST_CTX);
    let tf = h.instantiate(&bytes);
    assert_eq!(
        h.call(&tf, &[1.5f64.to_bits() as i64], 1),
        0x3E00,
        "f64 1.5 -> f16"
    );
}

/// `OperandImm` is an i32 operand SIGN-extended to the i64 slot (cranelift `operand as i64` mirror).
#[test]
fn operand_imm_sign_extends() {
    let isa = common::isa();
    let mut h = Harness::new();
    for imm in [-5, i32::MIN, i32::MAX, 0, -1] {
        let f = func(
            0,
            1,
            vec![Inst::new(MOV_IMM, 0, imm, 0), Inst::new(HALT, 0, 0, 0)],
        );
        let got = h.check_argc(&isa, &f, FILL, HOST_CTX, &[], 0);
        assert_eq!(got, i64::from(imm), "sign-extended immediate {imm}");
    }
}

/// argc boundaries: absent params stay `fill` (and their memory is never read); extra args are
/// ignored. `Imm`/`ImmF64` and consts baking ride along.
#[test]
fn argc_boundaries_and_fill() {
    let isa = common::isa();
    let mut h = Harness::new();

    // return param 2 of nparams=3
    let f = func(3, 3, vec![Inst::new(HALT, 2, 0, 0)]);
    let args = [11, 22, 33, 44, 55];
    for argc in [0usize, 1, 2] {
        let got = h.check_argc(&isa, &f, FILL, HOST_CTX, &args, argc);
        assert_eq!(got, FILL, "param 2 must stay fill at argc={argc}");
    }
    assert_eq!(h.check_argc(&isa, &f, FILL, HOST_CTX, &args, 3), 33);
    assert_eq!(
        h.check_argc(&isa, &f, FILL, HOST_CTX, &args, 5),
        33,
        "extra args ignored"
    );

    // zero-arg function returning an untouched local: always fill.
    let f0 = func(0, 2, vec![Inst::new(HALT, 1, 0, 0)]);
    assert_eq!(h.check_argc(&isa, &f0, FILL, HOST_CTX, &[], 0), FILL);

    // consts land right after the params and can shadow neither params nor fill slots.
    let fc = FuncDef {
        nparams: 1,
        consts: vec![10, -20, 0x0000_7FFF_FFFF_FFFF],
        nlocals: 6,
        code: vec![
            Inst::new(bin_opcode(3, 0), 4, 1, 2), // r4 = c0 + c1 (I64 add)
            Inst::new(bin_opcode(3, 0), 4, 4, 3), // r4 += c2
            Inst::new(bin_opcode(3, 0), 4, 4, 0), // r4 += arg0
            Inst::new(HALT, 4, 0, 0),
        ],
    };
    let got = h.check_argc(&isa, &fc, FILL, HOST_CTX, &[5], 1);
    assert_eq!(got, 10 - 20 + 0x0000_7FFF_FFFF_FFFF + 5);
    // r5 untouched stays fill even with consts present
    let fc2 = FuncDef {
        code: vec![Inst::new(HALT, 5, 0, 0)],
        ..fc
    };
    assert_eq!(h.check_argc(&isa, &fc2, FILL, HOST_CTX, &[5], 1), FILL);

    // Imm / ImmF64 literals
    let fi = func(
        0,
        2,
        vec![
            Inst::new(IMM_F64, 0, 0, 0),
            Inst::new(IMM_BIG, 1, 0, 0),
            Inst::new(bin_opcode(3, 1), 0, 1, 0), // r0 = r1 - r0 (i64)
            Inst::new(HALT, 0, 0, 0),
        ],
    );
    let got = h.check_argc(&isa, &fi, FILL, HOST_CTX, &[], 0);
    assert_eq!(
        got,
        0x0123_4567_89AB_CDEF_i64.wrapping_sub(1.5f64.to_bits() as i64)
    );
}

/// Host builtins: `Host1/2` via the resolver-mapped table slots; `HostCtx1/2` get the module's
/// `host_ctx` word prepended — and it must be `host_ctx`, not `fill`, not the opaque ctx param.
#[test]
fn host_calls_and_host_ctx_fill_independence() {
    let isa = common::isa();
    let mut h = Harness::new();

    // hc_ret_ctx returns its first argument: end-to-end this IS the module's host_ctx word.
    let f = func(1, 2, vec![Inst::new(HOSTCTX1, 1, 0, 0), Inst::new(HALT, 1, 0, 0)]);
    let got = h.check_argc(&isa, &f, FILL, HOST_CTX, &[9], 1);
    assert_eq!(got, HOST_CTX, "HostCtx1 must receive Module::host_ctx");
    assert_ne!(got, FILL, "host_ctx must be independent of fill");
    assert_ne!(
        got,
        i64::from(CTX_JUNK),
        "host_ctx must not be the opaque ctx param"
    );

    // Distinct fill/host_ctx words flow to their own consumers within one function.
    let f2 = func(
        2,
        5,
        vec![
            Inst::new(HOST1, 2, 0, 0),    // r2 = h_add7(arg0)
            Inst::new(HOST2, 3, 2, 1),    // r3 = h_mix(r2, arg1)
            Inst::new(HOSTCTX2, 4, 3, 1), // r4 = hc_mix(host_ctx, r3, arg1)
            Inst::new(HALT, 4, 0, 0),
        ],
    );
    h.check_all(
        &isa,
        &f2,
        FILL,
        HOST_CTX,
        &[&[1, 2], &[-7, 1 << 40], &[i64::MIN, i64::MAX]],
    );
    // ... and swapping fill/host_ctx changes the result exactly like the oracle says it should.
    h.check_all(&isa, &f2, HOST_CTX, FILL, &[&[1, 2]]);

    // Guard is a transparent no-op on this tier.
    let fg = func(
        2,
        3,
        vec![
            Inst::new(GUARD, 0, 1, 0),
            Inst::new(bin_opcode(3, 2), 2, 0, 1), // r2 = arg0 * arg1
            Inst::new(HALT, 2, 0, 0),
        ],
    );
    h.check_all(&isa, &fg, FILL, HOST_CTX, &[&[3, 4], &[-1, i64::MAX]]);
}

// --- LCG fuzz ------------------------------------------------------------------------------------
// The generators (`lcg`/`rand_word`/`gen_straightline`) moved to `common` at O-14 A-1 so the
// N-in-1 gate (tests/aot.rs) can run the IDENTICAL corpus — same generators, same seeds.

/// Differential fuzz: several hundred random straight-line functions, each compiled to a fresh
/// wasm module, instantiated, and compared bit-for-bit against the T0 oracle.
#[test]
fn fuzz_straightline_matches_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let mut next = lcg(0x9E37_79B9_7F4A_7C15);
    for i in 0..400 {
        let (f, args, fill, host_ctx) = gen_straightline(&mut next);
        let bytes = h.compile(&isa, &f, fill, host_ctx);
        let tf = h.instantiate(&bytes);
        let got = h.call(&tf, &args, args.len());
        let want = oracle(&isa, &f, &args, fill, host_ctx);
        assert_eq!(
            got, want,
            "fuzz #{i}: wasm {got:#x} ≠ oracle {want:#x}\nargs={args:?} fill={fill:#x} \
             host_ctx={host_ctx:#x}\nfunc={f:?}"
        );
    }
}

// --- explicit control-flow samples (W-1b, design §3 dispatcher loop) ------------------------------

/// Backward `JumpIf` loop (sum 1..=n): the canonical hot-loop shape, pinned against the closed
/// form on top of the oracle comparison.
#[test]
fn loop_sum_matches_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let sum = func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 1, 0, 0),          // 0: acc = 0
            Inst::new(MOV_IMM, 2, 1, 0),          // 1: one = 1
            Inst::new(bin_opcode(3, 0), 1, 1, 0), // 2: acc += n      <- loop header (backward target)
            Inst::new(bin_opcode(3, 1), 0, 0, 2), // 3: n -= 1
            Inst::new(JUMP_IF, 0, 2, 0),          // 4: if n != 0 goto 2 (backward edge)
            Inst::new(HALT, 1, 0, 0),             // 5: return acc
        ],
    );
    for n in [1i64, 2, 7, 50] {
        let got = h.check_argc(&isa, &sum, FILL, HOST_CTX, &[n], 1);
        assert_eq!(got, n * (n + 1) / 2, "sum 1..={n}");
    }
}

/// Backward `JumpTo` loop with a forward `JumpIf` exit (factorial) — the loop-header test at the
/// top, the back edge at the bottom.
#[test]
fn loop_factorial_matches_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let fact = func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 1, 1, 0),          // 0: acc = 1
            Inst::new(MOV_IMM, 2, 1, 0),          // 1: one = 1
            Inst::new(JUMP_IF, 0, 4, 0),          // 2: if n != 0 goto 4    <- header (forward JumpIf)
            Inst::new(HALT, 1, 0, 0),             // 3: return acc
            Inst::new(bin_opcode(3, 2), 1, 1, 0), // 4: acc *= n
            Inst::new(bin_opcode(3, 1), 0, 0, 2), // 5: n -= 1
            Inst::new(JUMP, 2, 0, 0),             // 6: goto 2 (backward JumpTo, last instruction)
        ],
    );
    for (n, want) in [(0i64, 1i64), (1, 1), (5, 120), (10, 3_628_800)] {
        let got = h.check_argc(&isa, &fact, FILL, HOST_CTX, &[n], 1);
        assert_eq!(got, want, "{n}!");
    }
}

/// Nested loops (acc += 1, inner_n times, outer_n times): two backward edges, an inner-loop
/// re-entry from below, and a `JumpTo` over the inner body.
#[test]
fn nested_loops_match_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let nested = func(
        2,
        5,
        vec![
            Inst::new(MOV_IMM, 2, 0, 0),          // 0:  acc = 0
            Inst::new(MOV_IMM, 3, 1, 0),          // 1:  one = 1
            Inst::new(JUMP_IF, 0, 4, 0),          // 2:  outer header: if i != 0 goto 4
            Inst::new(HALT, 2, 0, 0),             // 3:  return acc
            Inst::new(COPY, 4, 1, 0),             // 4:  j = inner_n
            Inst::new(JUMP_IF, 4, 7, 0),          // 5:  inner header: if j != 0 goto 7
            Inst::new(JUMP, 10, 0, 0),            // 6:  inner done → outer tail
            Inst::new(bin_opcode(3, 0), 2, 2, 3), // 7:  acc += 1
            Inst::new(bin_opcode(3, 1), 4, 4, 3), // 8:  j -= 1
            Inst::new(JUMP, 5, 0, 0),             // 9:  goto inner header (backward)
            Inst::new(bin_opcode(3, 1), 0, 0, 3), // 10: i -= 1
            Inst::new(JUMP, 2, 0, 0),             // 11: goto outer header (backward)
        ],
    );
    for (i, j) in [(0i64, 5i64), (3, 4), (5, 0), (2, 7), (1, 1)] {
        let got = h.check_argc(&isa, &nested, FILL, HOST_CTX, &[i, j], 2);
        assert_eq!(got, i * j, "nested {i}x{j}");
    }
}

/// Diamond branch with a SHARED fall-through target: pc 4 is simultaneously a `JumpTo` target
/// (from the not-taken arm) and the fall-through leader of the taken arm's non-terminator run —
/// the §3 B#3 boundary-emission rule on a merge point.
#[test]
fn diamond_and_shared_fall_through_match_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let diamond = func(
        2,
        3,
        vec![
            Inst::new(JUMP_IF, 0, 3, 0),          // 0: if c goto 3
            Inst::new(bin_opcode(3, 2), 1, 1, 1), // 1: x *= x   (not-taken arm)
            Inst::new(JUMP, 4, 0, 0),             // 2: goto merge
            Inst::new(bin_opcode(3, 0), 1, 1, 1), // 3: x += x   (taken arm — FALLS into the merge)
            Inst::new(HOST1, 1, 1, 0),            // 4: merge: x = h_add7(x)
            Inst::new(HALT, 1, 0, 0),             // 5
        ],
    );
    h.check_all(
        &isa,
        &diamond,
        FILL,
        HOST_CTX,
        &[&[0, 5], &[1, 5], &[-1, 7], &[0, -3], &[1 << 62, 9]],
    );

    // Two JumpIfs sharing one target leader (plus the all-fall-through path).
    let shared = func(
        2,
        3,
        vec![
            Inst::new(JUMP_IF, 0, 4, 0),   // 0: if a goto 4
            Inst::new(JUMP_IF, 1, 4, 0),   // 1: if b goto 4 (same target)
            Inst::new(MOV_IMM, 2, 111, 0), // 2
            Inst::new(HALT, 2, 0, 0),      // 3
            Inst::new(MOV_IMM, 2, 222, 0), // 4: shared target
            Inst::new(HALT, 2, 0, 0),      // 5
        ],
    );
    h.check_all(
        &isa,
        &shared,
        FILL,
        HOST_CTX,
        &[&[0, 0], &[0, 1], &[1, 0], &[1, 1], &[-1, 0], &[0, i64::MIN]],
    );
}

/// **Irreducible CFG**: H1 (pc 3) and H2 (pc 7) form a cycle entered at BOTH headers from
/// outside (pc 2 jumps to H2, or falls into H1), so neither dominates the other — no reducible
/// loop nest exists. Exactly the shape a relooper cannot express without duplication and the §3
/// dispatcher handles for free.
#[test]
fn irreducible_cfg_matches_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let irr = func(
        2,
        4,
        vec![
            Inst::new(MOV_IMM, 2, 1, 0),          // 0:  acc = 1
            Inst::new(MOV_IMM, 3, 1, 0),          // 1:  one = 1
            Inst::new(JUMP_IF, 0, 7, 0),          // 2:  if c goto H2 (else fall into H1)
            Inst::new(bin_opcode(3, 0), 2, 2, 3), // 3:  H1: acc += 1
            Inst::new(bin_opcode(3, 1), 1, 1, 3), // 4:      n -= 1
            Inst::new(JUMP_IF, 1, 7, 0),          // 5:      if n != 0 goto H2
            Inst::new(HALT, 2, 0, 0),             // 6
            Inst::new(bin_opcode(3, 0), 2, 2, 2), // 7:  H2: acc += acc
            Inst::new(bin_opcode(3, 1), 1, 1, 3), // 8:      n -= 1
            Inst::new(JUMP_IF, 1, 3, 0),          // 9:      if n != 0 goto H1
            Inst::new(HALT, 2, 0, 0),             // 10
        ],
    );
    for c in [0i64, 1] {
        for n in [1i64, 2, 3, 4, 6] {
            let got = h.check_argc(&isa, &irr, FILL, HOST_CTX, &[c, n], 2);
            let want = oracle(&isa, &irr, &[c, n], FILL, HOST_CTX);
            assert_eq!(got, want, "irreducible c={c} n={n}");
        }
    }
}

/// Multi-effect opcodes (W-1a panel A#3 carry-over): `multi2` (two `SetLocalAt`s with a
/// read-after-write between them) on the straight-line path, and `dec_jnz` (`SetLocalAt` then a
/// `JumpIf` whose cond reads the fresh write) as a real loop on the dispatcher path.
#[test]
fn multi_effect_opcodes_match_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();

    // multi2, straight-line; halt each written register in turn so both effects' results surface.
    let pairs: &[&[Value]] = &[
        &[1, 2],
        &[-7, 1 << 40],
        &[i64::MIN, i64::MAX],
        &[0x3C00, 0x7E01],
        &[0, 0],
    ];
    let m_dst = func(2, 3, vec![Inst::new(MULTI2, 2, 0, 1), Inst::new(HALT, 2, 0, 0)]);
    h.check_all(&isa, &m_dst, FILL, HOST_CTX, pairs);
    let m_src = func(2, 3, vec![Inst::new(MULTI2, 2, 0, 1), Inst::new(HALT, 0, 0, 0)]);
    h.check_all(&isa, &m_src, FILL, HOST_CTX, pairs);

    // dec_jnz countdown: acc += n each pass, n decremented INSIDE the same opcode whose JumpIf
    // then reads the decremented value — sum n + (n-1) + … + 1.
    let dj = func(
        1,
        2,
        vec![
            Inst::new(MOV_IMM, 1, 0, 0),          // 0: acc = 0
            Inst::new(bin_opcode(3, 0), 1, 1, 0), // 1: acc += n     <- loop
            Inst::new(DEC_JNZ, 0, 1, 0),          // 2: if --n != 0 goto 1
            Inst::new(HALT, 1, 0, 0),             // 3
        ],
    );
    for n in [1i64, 4, 9] {
        let got = h.check_argc(&isa, &dj, FILL, HOST_CTX, &[n], 1);
        assert_eq!(got, n * (n + 1) / 2, "dec_jnz sum {n}");
    }

    // multi2 inside a dispatcher-path loop (multi-effect × control flow interaction).
    let m_loop = func(
        2,
        4,
        vec![
            Inst::new(MULTI2, 3, 0, 1),  // 0: r3 = a + b; a = r3 * b   <- loop
            Inst::new(DEC_JNZ, 1, 0, 0), // 1: if --b != 0 goto 0
            Inst::new(HALT, 0, 0, 0),    // 2: return the accumulated a
        ],
    );
    for b in [1i64, 2, 5] {
        for a in [0i64, 3, -11] {
            let got = h.check_argc(&isa, &m_loop, FILL, HOST_CTX, &[a, b], 2);
            let want = oracle(&isa, &m_loop, &[a, b], FILL, HOST_CTX);
            assert_eq!(got, want, "multi2 loop a={a} b={b}");
        }
    }
}

// --- random CFG fuzz (design §8a C#5 — the W-1b gate's mandatory element) -------------------------

// The CFG generator (`gen_cfg`) and its `mid_merge_fall_boundaries` oracle moved to `common`
// at O-14 A-1 (same-corpus rationale as the straight-line generator above).

/// Random-CFG differential fuzz (§8a C#5 — mandatory W-1b gate element): several hundred
/// arbitrary-shape (irreducible included), termination-guaranteed CFGs, each compiled,
/// instantiated, run, and compared bit-for-bit against the T0 oracle. The corpus-shape assertion
/// at the end pins the lens B#1 fix structurally: the corpus must actually contain mid-merge
/// fall boundaries (ordinal > 1), or the generator has regressed to the always-terminated form
/// this fuzz's boundary coverage silently degenerated to before.
#[test]
fn fuzz_cfg_matches_oracle() {
    let isa = common::isa();
    let mut h = Harness::new();
    let mut next = lcg(0xA5F1_5EED_0D15_EA5E);
    let mut mid_merges = 0usize;
    for i in 0..300 {
        let (f, args, fill, host_ctx) = gen_cfg(&mut next);
        mid_merges += mid_merge_fall_boundaries(&isa, &f);
        // gen_cfg is check_func- and §4.6-clean by construction; `compile` panics on a Refusal,
        // which is exactly right (a refusal here is a generator bug, not a case to skip).
        let bytes = h.compile(&isa, &f, fill, host_ctx);
        let tf = h.instantiate(&bytes);
        let got = h.call(&tf, &args, args.len());
        let want = oracle(&isa, &f, &args, fill, host_ctx);
        assert_eq!(
            got, want,
            "cfg fuzz #{i}: wasm {got:#x} ≠ oracle {want:#x}\nargs={args:?} fill={fill:#x} \
             host_ctx={host_ctx:#x}\nfunc={f:?}"
        );
    }
    // 60 with the pinned seed at the time of writing; > 0 is the structural floor (lens B#1).
    assert!(
        mid_merges > 0,
        "corpus lost its mid-merge (ordinal > 1) fall boundaries — generator regression"
    );
}
