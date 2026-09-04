//! otter-wasm-demo — the O-13 §8(b)/(c) in-guest acceptance suite (design §9 row
//! `otter-wasm-demo`, §10 W-5). W-4 shipped this as a link-proof skeleton; as of W-5 it IS the
//! E2E deliverable: run under the reference host (`hosts/otter-host-wasmtime`) it executes the
//! §8(b) in-guest cross-tier differential suite and prints one greppable marker per §8(c)
//! acceptance item. `just wasm-e2e` (repo root) builds and runs all three modes.
//!
//! ```text
//! otter-wasm-demo <tierup|reject|t0>       (the mode arrives as WASI argv[1], host-forwarded)
//! ```
//!
//! | mode | build | host flags | markers |
//! |---|---|---|---|
//! | `tierup` | `--features wasm-emit` | (none) | `E2E-1-BUILD-OK` `E2E-2-TIERUP-OK` `E2E-3-ORACLE-OK` `E2E-4-TRAP-OK` `E2E-7-GROW-OK` `E2E-8B-CROSSTIER-OK` |
//! | `reject` | `--features wasm-emit` | `--reject-compiles` | `E2E-1-BUILD-OK` `E2E-5-REJECT-OK` |
//! | `t0` | (feature off) | `--no-otter-jit` | `E2E-1-BUILD-OK` `E2E-6-FEATUREOFF-OK` |
//! | `p3bridge` | `--features wasm-emit,p3-bridge` | `--p3-bridge` | `E2E-1-BUILD-OK` `E2E-9-P3BRIDGE-OK` (O-16f — `just wasm-e2e-p3`) |
//!
//! Every successful run ends with `E2E-ALL-OK`; any failed assertion prints `E2E-FAIL: <what>`
//! and exits non-zero (the host propagates the guest exit code). The guest also prints its own
//! compile-accounting expectation (`E2E-EXPECT-COMPILES=` / `E2E-EXPECT-COMPILE-REQUESTS=`),
//! which `just wasm-e2e` cross-checks against the host's `HOST-COMPILE-REQUESTS=`/
//! `HOST-COMPILE-COUNT=` counters — the §8(c)-5 "exactly one `otter_jit.compile` call per hot
//! function, no retry loop" evidence.
//!
//! Source stays stable-compatible; only the BUILD PIPELINE is nightly (§7 / owner decision
//! §12-1) — see README.md for the canonical build commands.

use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, Value};
use otter_engine::ModuleEngine;

// ---------------------------------------------------------------------------------------------
// Demo ISA + module (shared by every mode; the T0 oracle interprets the same objects)
// ---------------------------------------------------------------------------------------------

const EQ0: u16 = 0; // l[op0] = (l[op1] == 0)
const JUMP_IF: u16 = 1; // if l[op0] != 0 { pc = op1 }
const SUB1: u16 = 2; // l[op0] = l[op1] - 1
const SET_IMM: u16 = 3; // l[op0] = op1 (immediate)
const COPY: u16 = 4; // l[op0] = l[op1]
const TAILCALL: u16 = 5; // tailcall l[op0..]([callee, args...]), argc = op1
const CALL: u16 = 6; // l[op0] = call l[op1..]([callee, args...]), argc = op2
const RET: u16 = 7; // return l[op0]
const MUL: u16 = 8; // l[op0] = l[op1] * l[op2]   (i64)
const ADD: u16 = 9; // l[op0] = l[op1] + l[op2]   (i64)
const DIV: u16 = 10; // l[op0] = l[op1] / l[op2]  (INTEGER Div — §4.6-3 wasm-Refused + T0 panic)

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
fn bin(op: ArithOp, a: Expr, b: Expr) -> Expr {
    Expr::Bin(BinOp::new(NumTy::I64, op), Box::new(a), Box::new(b))
}

fn demo_isa() -> Isa {
    Isa {
        ops: vec![
            set("eq0", bin(ArithOp::Eq, Expr::LocalAt(1), Expr::Imm(0))),
            ctrl(
                "jump_if",
                Effect::JumpIf {
                    cond: Expr::LocalAt(0),
                    target: 1,
                },
            ),
            set("sub1", bin(ArithOp::Sub, Expr::LocalAt(1), Expr::Imm(1))),
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
            set("mul", bin(ArithOp::Mul, Expr::LocalAt(1), Expr::LocalAt(2))),
            set("add", bin(ArithOp::Add, Expr::LocalAt(1), Expr::LocalAt(2))),
            // Integer Div: BinOp::eval panics on it (T0: unreachable!), and the wasm-emit
            // precheck refuses any function containing it (§4.6-3) BEFORE any host compile call
            // — the deterministic acceptance-4 trap source that stays T0 by design.
            set("divi", bin(ArithOp::Div, Expr::LocalAt(1), Expr::LocalAt(2))),
        ],
    }
}

const ENTRY: usize = 0;
const SQ: usize = 1;
const SUMDOWN: usize = 2;
const CRASH: usize = 3;

/// entry(x, n) = sq(x) + sumdown(0, n) — `Call`s across tiers (drives SQ/SUMDOWN hot too).
/// sq(x) = x * x — the minimal hot leaf (acceptance 2).
/// sumdown(acc, n) = n == 0 ? acc : tailcall sumdown(acc + n, n - 1) — the O-8 tail-recursion
///   gate shape (acceptance 3's 1M-deep run).
/// crash(a, b) = a / b at i64 — integer Div: wasm tier Refuses (§4.6-3), T0 execution panics
///   deterministically (acceptance 4's trap source; never crosses the tier-up threshold here).
fn demo_module() -> Module {
    let entry = FuncDef {
        nparams: 2, // l0=x l1=n; l2=sq(x) l3=sumdown l4=callee l5=a0 l6=a1 l7=sum
        consts: vec![],
        nlocals: 8,
        code: vec![
            Inst::new(SET_IMM, 4, SQ as i32, 0),
            Inst::new(COPY, 5, 0, 0),
            Inst::new(CALL, 2, 4, 1),
            Inst::new(SET_IMM, 4, SUMDOWN as i32, 0),
            Inst::new(SET_IMM, 5, 0, 0),
            Inst::new(COPY, 6, 1, 0),
            Inst::new(CALL, 3, 4, 2),
            Inst::new(ADD, 7, 2, 3),
            Inst::new(RET, 7, 0, 0),
        ],
    };
    let sq = FuncDef {
        nparams: 1, // l0=x l1=x*x
        consts: vec![],
        nlocals: 2,
        code: vec![Inst::new(MUL, 1, 0, 0), Inst::new(RET, 1, 0, 0)],
    };
    let sumdown = FuncDef {
        nparams: 2, // l0=acc l1=n l2=cond l3=callee l4=acc+n l5=n-1
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(EQ0, 2, 1, 0),
            Inst::new(JUMP_IF, 2, 6, 0),
            Inst::new(ADD, 4, 0, 1),
            Inst::new(SUB1, 5, 1, 0),
            Inst::new(SET_IMM, 3, SUMDOWN as i32, 0),
            Inst::new(TAILCALL, 3, 2, 0),
            Inst::new(RET, 0, 0, 0),
        ],
    };
    let crash = FuncDef {
        nparams: 2, // l0=a l1=b l2=a/b
        consts: vec![],
        nlocals: 3,
        code: vec![Inst::new(DIV, 2, 0, 1), Inst::new(RET, 2, 0, 0)],
    };
    Module {
        funcs: vec![entry, sq, sumdown, crash],
        fill: 0,
        host_ctx: 0,
        entry: ENTRY,
    }
}

// ---------------------------------------------------------------------------------------------
// Suite scaffolding
// ---------------------------------------------------------------------------------------------

/// Tier-up threshold for the hot modes (reused for t1+t2; t1 is unused on wasm builds). Small on
/// purpose: acceptance 2 wants the promotion observable after a handful of calls.
const T2: u32 = 3;

/// (x, n) inputs for entry(x, n) — also drive SQ/SUMDOWN via nested `Call`s.
const PAIRS: [(Value, Value); 6] = [(3, 10), (5, 7), (0, 0), (9, 3), (2, 25), (7, 1)];

/// 1M-deep tail recursion (acceptance 3 — the O-8 decisive gate, wasm edition).
const DEEP_N: Value = 1_000_000;
const DEEP_SUM: Value = 500_000_500_000; // sum(1..=1_000_000)

fn fail(what: &str) -> ! {
    eprintln!("E2E-FAIL: {what}");
    std::process::exit(1);
}

fn check(cond: bool, what: &str) {
    if !cond {
        fail(what);
    }
}

/// The in-guest bit-exactness oracle (§8(b)-i): `otter::interp::run_module` on a clone of the
/// SAME module with `entry` retargeted at `func`. No engine, no tiers — the canonical T0
/// reference semantics, running inside the same wasm instance.
fn oracle(isa: &Isa, module: &Module, func: usize, args: &[Value]) -> Value {
    let mut m = module.clone();
    m.entry = func;
    otter::interp::run_module(isa, &m, args)
}

/// `try_dispatch` that must NOT trap.
fn dispatch_ok(eng: &ModuleEngine, idx: usize, args: &[Value], what: &str) -> Value {
    match eng.try_dispatch(idx, args) {
        Ok(v) => v,
        Err(t) => fail(&format!("{what}: unexpected trap {t:?}")),
    }
}

/// Acceptance 7's guest-side `memory.grow`: grow the linear memory by 16 pages (1 MiB) while
/// emitted instances (which import this very memory) are live, then keep calling emitted code.
#[cfg(all(feature = "wasm-emit", target_arch = "wasm32"))]
fn grow_linear_memory() -> bool {
    core::arch::wasm32::memory_grow(0, 16) != usize::MAX
}
#[cfg(all(feature = "wasm-emit", not(target_arch = "wasm32")))]
fn grow_linear_memory() -> bool {
    true // native type-check builds only; the wasm-e2e gate always runs the wasm32 artifact
}

#[cfg(all(feature = "p3-bridge", target_arch = "wasm32"))]
#[link(wasm_import_module = "otter_p3")]
unsafe extern "C" {
    /// O-16f bridge import (mode `p3bridge` only): the reference host's `--p3-bridge` mode
    /// serves this by calling a real component's `stamp: func() -> u64` export (which itself
    /// consumes `wasi:clocks/monotonic-clock@0.3.0` + `wasi:random/random@0.3.0` through the p3
    /// host wiring) in the SAME synchronous Store, then writing the u64 LE at `dst`. Returns 0
    /// on success, negative host-side error class otherwise (sign-normative, module doc of the
    /// host's `p3_bridge`). Kept OUT of every other build: the frozen import surface of the
    /// stock guest ({wasi_snapshot_preview1, otter_jit}) must not change (§5.1 freeze).
    fn fetch_stamp(dst: *mut u64) -> i32;
}
/// Native type-check stub (never a gated artifact — the wasm-e2e-p3 gate runs wasm32 only):
/// fails loudly so a native run can never fake a bridge round trip.
#[cfg(all(feature = "p3-bridge", not(target_arch = "wasm32")))]
#[allow(clippy::missing_safety_doc)]
unsafe fn fetch_stamp(_dst: *mut u64) -> i32 {
    -99
}

/// One bridge round trip: host writes the stamp into `slot` AND returns status; both the status
/// and the memory round trip (nonzero value actually landed in guest linear memory) are checked.
#[cfg(feature = "p3-bridge")]
fn bridge_stamp(what: &str) -> u64 {
    let mut slot: u64 = 0;
    let rc = unsafe { fetch_stamp(&mut slot) };
    check(rc == 0, &format!("{what}: fetch_stamp status {rc} != 0"));
    check(slot != 0, &format!("{what}: stamp not written to guest memory"));
    slot
}

// ---------------------------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------------------------

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: otter-wasm-demo <tierup|reject|t0|p3bridge>  (host forwards the mode as argv[1])");
        std::process::exit(2);
    });
    // Acceptance 1: this §7-built (pinned nightly + build-std + -Cpanic=unwind + exnref +
    // --export-table/--growable-table) wasip1 artifact instantiated and is executing.
    println!(
        "E2E-1-BUILD-OK §7-config wasip1 artifact running (mode {mode}, wasm-emit feature {})",
        cfg!(feature = "wasm-emit"),
    );
    match mode.as_str() {
        "tierup" => run_tierup(),
        "reject" => run_reject(),
        "t0" => run_t0(),
        "p3bridge" => run_p3bridge(),
        other => {
            eprintln!("otter-wasm-demo: unknown mode '{other}' (want tierup|reject|t0|p3bridge)");
            std::process::exit(2);
        }
    }
    println!("E2E-ALL-OK");
}

/// Normal mode under the fully wired reference host: acceptance items 2, 3, 4, 7 and the §8(b)
/// cross-tier differential (feature-on build required).
fn run_tierup() {
    check(
        cfg!(feature = "wasm-emit"),
        "mode tierup needs a --features wasm-emit build",
    );
    #[cfg(feature = "wasm-emit")]
    {
        let isa = demo_isa();
        let module = demo_module();
        let eng = ModuleEngine::new(isa.clone(), module.clone(), T2, T2);

        // ---- acceptance 2: hot function crosses t2 and lands on the wasm-emit tier ----------
        for x in 0..6 {
            let got = dispatch_ok(&eng, SQ, &[x], "sq");
            check(got == x * x, "sq(x) != x*x");
            check(got == oracle(&isa, &module, SQ, &[x]), "sq vs oracle");
        }
        check(
            eng.tier_of(SQ) == "wasm-emit",
            "sq must be on the wasm-emit tier after crossing t2",
        );
        println!(
            "E2E-2-TIERUP-OK sq tier={} after {} calls (t2={T2})",
            eng.tier_of(SQ),
            eng.calls(SQ),
        );

        // ---- acceptance 3: every result bit-identical to the in-guest oracle, and the O-8
        // 1M-deep tail-recursion gate completes ON THE PROMOTED TIER --------------------------
        let warm = dispatch_ok(&eng, SUMDOWN, &[0, 10], "sumdown warm");
        check(warm == 55, "sumdown(0,10) != 55");
        check(
            warm == oracle(&isa, &module, SUMDOWN, &[0, 10]),
            "sumdown vs oracle",
        );
        check(
            eng.tier_of(SUMDOWN) == "wasm-emit",
            "sumdown must be promoted (self-tail-calls cross t2 within the first outer call)",
        );
        let deep = dispatch_ok(&eng, SUMDOWN, &[0, DEEP_N], "sumdown 1M");
        check(deep == DEEP_SUM, "sumdown(0,1M) != 500000500000");
        check(
            deep == oracle(&isa, &module, SUMDOWN, &[0, DEEP_N]),
            "sumdown 1M vs oracle",
        );
        for &(x, n) in &PAIRS {
            let got = dispatch_ok(&eng, ENTRY, &[x, n], "entry");
            check(got == oracle(&isa, &module, ENTRY, &[x, n]), "entry vs oracle");
        }
        check(
            eng.tier_of(ENTRY) == "wasm-emit",
            "entry must be promoted after the PAIRS sweep",
        );
        println!(
            "E2E-3-ORACLE-OK all results bit-identical to in-guest oracle; 1M-deep tail \
             recursion on tier={} returned {deep}",
            eng.tier_of(SUMDOWN),
        );

        // ---- acceptance 4: deterministic T0 trap (integer Div — wasm tier Refuses the shape,
        // §4.6-3, so it STAYS T0) → Err(Trap), then clear_trap and everything still works ------
        // Calls 3+ of this loop also prove the §4.6 Refusal is sticky T0 (the threshold crossing
        // on call 3 refuses ENGINE-side, before any host request — the E2E recipe's request
        // accounting would catch one) and that the trap reproduces identically every time.
        for _ in 0..4 {
            match eng.try_dispatch(CRASH, &[7, 2]) {
                Ok(v) => fail(&format!("crash() returned Ok({v}) instead of trapping")),
                Err(t) => check(t.idx == CRASH, "Trap.idx must name the crashing function"),
            }
            check(eng.trapped(), "engine must be poisoned after the trap");
            eng.clear_trap();
            check(!eng.trapped(), "clear_trap must un-poison");
        }
        check(
            eng.tier_of(CRASH) == "interpreter",
            "crash must stay T0 (wasm tier refuses integer Div)",
        );
        // Post-recovery: a T0-shaped call tree AND the already-promoted wasm functions all work.
        let post_entry = dispatch_ok(&eng, ENTRY, &[4, 5], "entry after clear_trap");
        check(
            post_entry == oracle(&isa, &module, ENTRY, &[4, 5]),
            "entry after clear_trap vs oracle",
        );
        check(
            dispatch_ok(&eng, SQ, &[12], "sq after clear_trap") == 144,
            "sq(12) != 144 after clear_trap",
        );
        check(
            eng.tier_of(SQ) == "wasm-emit",
            "sq must still be on the wasm-emit tier after trap recovery",
        );
        println!("E2E-4-TRAP-OK integer-Div T0 trap caught in-wasm; clear_trap + wasm re-run OK");

        // ---- acceptance 7: guest memory.grow AFTER promotion, emitted code keeps working -----
        check(grow_linear_memory(), "memory.grow(16 pages) failed");
        check(
            dispatch_ok(&eng, SQ, &[31], "sq after grow") == 961,
            "sq(31) != 961 after memory.grow",
        );
        check(
            dispatch_ok(&eng, SUMDOWN, &[0, 50], "sumdown after grow") == 1275,
            "sumdown(0,50) != 1275 after memory.grow",
        );
        let post_grow = dispatch_ok(&eng, ENTRY, &[6, 6], "entry after grow");
        check(
            post_grow == oracle(&isa, &module, ENTRY, &[6, 6]),
            "entry after memory.grow vs oracle",
        );
        println!("E2E-7-GROW-OK emitted code correct across a 16-page guest memory.grow");

        // ---- §8(b): (i) oracle vs (ii) T0-pinned vs (iii) wasm-promoted, bit-identical -------
        let pinned = ModuleEngine::new(isa.clone(), module.clone(), u32::MAX, u32::MAX);
        let promoted = ModuleEngine::new(isa.clone(), module.clone(), T2, T2);
        for _ in 0..2 {
            for &(x, n) in &PAIRS {
                let _ = dispatch_ok(&promoted, ENTRY, &[x, n], "8b warm"); // drive hot
            }
        }
        for f in [ENTRY, SQ, SUMDOWN] {
            check(
                promoted.tier_of(f) == "wasm-emit",
                "8b: entry/sq/sumdown must all be promoted in run (iii)",
            );
        }
        for &(x, n) in &PAIRS {
            let o = oracle(&isa, &module, ENTRY, &[x, n]);
            let p = dispatch_ok(&pinned, ENTRY, &[x, n], "8b T0-pinned");
            let w = dispatch_ok(&promoted, ENTRY, &[x, n], "8b promoted");
            check(
                o == p && p == w,
                "8b: oracle / T0-pinned / wasm-promoted diverged",
            );
        }
        for f in [ENTRY, SQ, SUMDOWN, CRASH] {
            check(
                pinned.tier_of(f) == "interpreter",
                "8b: the T0-pinned engine must never promote",
            );
        }
        println!("E2E-8B-CROSSTIER-OK oracle == T0-pinned == wasm-promoted on all PAIRS");

        // ---- §8(c)-5 accounting (normal side): how many otter_jit.compile round trips this
        // run legitimately performed = promoted functions across both promoting engines
        // (crash contributes none: its Refusal fires engine-side, before any host call).
        let expect = (0..module.funcs.len())
            .filter(|&f| eng.tier_of(f) == "wasm-emit")
            .count()
            + (0..module.funcs.len())
                .filter(|&f| promoted.tier_of(f) == "wasm-emit")
                .count();
        println!("E2E-EXPECT-COMPILES={expect}");
    }
}

/// O-16f `--p3-bridge` mode (`just wasm-e2e-p3`): the guest consumes a p3-world service through
/// the host's single-sync-Store component bridge, interleaved with REAL otter_jit tier-ups —
/// bridge round trips before / between / after two promotions, every promoted result checked
/// against the in-guest T0 oracle. Proves, from inside the guest, that path 12's two halves
/// coexist in one Store and one run: the frozen §5.1 JIT contract (unchanged) and the p3
/// component world (reached via `otter_p3.fetch_stamp`).
fn run_p3bridge() {
    check(
        cfg!(all(feature = "wasm-emit", feature = "p3-bridge")),
        "mode p3bridge needs a --features wasm-emit,p3-bridge build",
    );
    #[cfg(all(feature = "wasm-emit", feature = "p3-bridge"))]
    {
        let isa = demo_isa();
        let module = demo_module();
        let eng = ModuleEngine::new(isa.clone(), module.clone(), T2, T2);

        // ---- bridge round trip #1: BEFORE any tier-up (pure-T0 engine state) ----------------
        let s1 = bridge_stamp("bridge #1 (pre-tier-up)");

        // ---- tier-up #1: drive sq hot through the SAME host the component lives in ----------
        for x in 0..6 {
            let got = dispatch_ok(&eng, SQ, &[x], "p3: sq");
            check(got == x * x, "p3: sq(x) != x*x");
            check(got == oracle(&isa, &module, SQ, &[x]), "p3: sq vs oracle");
        }
        check(
            eng.tier_of(SQ) == "wasm-emit",
            "p3: sq must be on the wasm-emit tier after crossing t2",
        );

        // ---- bridge round trip #2: BETWEEN tier-ups (emitted instance now shares the Store) --
        let s2 = bridge_stamp("bridge #2 (between tier-ups)");

        // ---- tier-up #2: sumdown promotes (self-tail-calls cross t2), then runs 100k deep
        // ON THE PROMOTED TIER — post-bridge emitted code is bit-identical to the oracle -------
        let warm = dispatch_ok(&eng, SUMDOWN, &[0, 10], "p3: sumdown warm");
        check(warm == 55, "p3: sumdown(0,10) != 55");
        check(
            eng.tier_of(SUMDOWN) == "wasm-emit",
            "p3: sumdown must be promoted after the warm call",
        );
        let deep = dispatch_ok(&eng, SUMDOWN, &[0, 100_000], "p3: sumdown 100k");
        check(deep == 5_000_050_000, "p3: sumdown(0,100k) != 5000050000");
        check(
            deep == oracle(&isa, &module, SUMDOWN, &[0, 100_000]),
            "p3: sumdown 100k vs oracle",
        );

        // ---- bridge round trip #3: AFTER all tier-ups ---------------------------------------
        let s3 = bridge_stamp("bridge #3 (post-tier-up)");

        // Three stamps from (monotonic-clock << 1) ^ random-u64 must be pairwise distinct —
        // a host stub returning a constant (or a stale memory value) can never pass.
        check(
            s1 != s2 && s2 != s3 && s1 != s3,
            "p3: bridge stamps not pairwise distinct",
        );

        // §8(c)-5-style accounting, both channels: exactly one otter_jit.compile per promoted
        // function, and exactly one host bridge call per bridge_stamp() above.
        let expect = (0..module.funcs.len())
            .filter(|&f| eng.tier_of(f) == "wasm-emit")
            .count();
        println!("E2E-EXPECT-COMPILES={expect}");
        println!("E2E-EXPECT-BRIDGE-CALLS=3");
        println!(
            "E2E-9-P3BRIDGE-OK stamps=({s1:#x},{s2:#x},{s3:#x}) pairwise-distinct; \
             sq/sumdown tier=wasm-emit; bridge round trips pre/between/post tier-up"
        );
    }
}

/// `--reject-compiles` mode (§8(c)-5): the host answers every compile with -1 → sticky Refused,
/// permanent T0, results stay correct, and the host-side counters prove exactly-once requests.
fn run_reject() {
    check(
        cfg!(feature = "wasm-emit"),
        "mode reject needs a --features wasm-emit build",
    );
    #[cfg(feature = "wasm-emit")]
    {
        let isa = demo_isa();
        let module = demo_module();
        let eng = ModuleEngine::new(isa.clone(), module.clone(), T2, T2);

        // Drive the same three functions hot; every threshold crossing asks the host ONCE, gets
        // -1, installs sticky Refused, and never asks again.
        for x in 0..6 {
            check(dispatch_ok(&eng, SQ, &[x], "sq") == x * x, "reject: sq(x) != x*x");
        }
        for _ in 0..3 {
            check(
                dispatch_ok(&eng, SUMDOWN, &[0, 10], "sumdown") == 55,
                "reject: sumdown(0,10) != 55",
            );
        }
        for &(x, n) in &PAIRS {
            check(
                dispatch_ok(&eng, ENTRY, &[x, n], "entry") == oracle(&isa, &module, ENTRY, &[x, n]),
                "reject: entry vs oracle",
            );
        }
        for f in [ENTRY, SQ, SUMDOWN] {
            check(
                eng.tier_of(f) == "interpreter",
                "reject: every hot function must stay permanent T0 (sticky Refused)",
            );
            check(
                eng.calls(f) > T2,
                "reject: hot functions must actually have crossed the threshold",
            );
        }
        // Stickiness under continued load: MANY more calls — correctness holds, tier stays T0,
        // and the host-side HOST-COMPILE-REQUESTS counter (checked by the recipe) proves no
        // retry loop ever re-asked.
        for x in 0..20 {
            check(
                dispatch_ok(&eng, SQ, &[x], "sq sticky") == x * x,
                "reject: sq wrong under continued load",
            );
        }
        check(
            eng.tier_of(SQ) == "interpreter",
            "reject: sq must still be T0 after continued load",
        );
        // Exactly the three driven-hot functions requested a compile, once each (crash was never
        // called; §4.6-refused shapes never reach the host anyway).
        println!("E2E-EXPECT-COMPILE-REQUESTS=3");
        println!("E2E-5-REJECT-OK permanent T0 + correct results under all-compiles-rejected host");
    }
}

/// Feature-off mode (§8(c)-6, O-7 preservation): built WITHOUT `wasm-emit`, run under
/// `--no-otter-jit` (no `otter_jit` wired at all) — pure T0, green, including trap containment.
fn run_t0() {
    check(
        !cfg!(feature = "wasm-emit"),
        "mode t0 is for the feature-OFF build (this binary was built with wasm-emit)",
    );
    let isa = demo_isa();
    let module = demo_module();
    let eng = ModuleEngine::new(isa.clone(), module.clone(), T2, T2);

    for x in 0..6 {
        check(dispatch_ok(&eng, SQ, &[x], "sq") == x * x, "t0: sq(x) != x*x");
    }
    check(
        dispatch_ok(&eng, SUMDOWN, &[0, 10], "sumdown") == 55,
        "t0: sumdown(0,10) != 55",
    );
    // O-8's boundedness gate on plain T0-in-wasm: 1M-deep tail recursion through the dispatcher
    // trampoline, no compiled tier anywhere.
    check(
        dispatch_ok(&eng, SUMDOWN, &[0, DEEP_N], "sumdown 1M") == DEEP_SUM,
        "t0: sumdown(0,1M) != 500000500000",
    );
    for &(x, n) in &PAIRS {
        check(
            dispatch_ok(&eng, ENTRY, &[x, n], "entry") == oracle(&isa, &module, ENTRY, &[x, n]),
            "t0: entry vs oracle",
        );
    }
    for f in [ENTRY, SQ, SUMDOWN, CRASH] {
        check(
            eng.tier_of(f) == "interpreter",
            "t0: every function must report the interpreter tier",
        );
    }
    // O-12 fail-closed works feature-off too: in-wasm catch_unwind contains the Div panic.
    match eng.try_dispatch(CRASH, &[9, 3]) {
        Ok(v) => fail(&format!("t0: crash() returned Ok({v}) instead of trapping")),
        Err(t) => check(t.idx == CRASH, "t0: Trap.idx must name the crashing function"),
    }
    eng.clear_trap();
    check(
        dispatch_ok(&eng, SQ, &[8], "sq after clear_trap") == 64,
        "t0: sq(8) != 64 after clear_trap",
    );
    println!("E2E-6-FEATUREOFF-OK pure-T0 green on an otter_jit-less host (feature-off build)");
}
