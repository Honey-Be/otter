//! O-14 A-5 host-train E2E (design `.claude-research-library/otter-o14-aot-core-design.md`
//! §8.2 / §9 A-5): run `compile_module` AOT artifacts on the DEPLOYMENT host shape — wasmtime 48,
//! reference-host features (`runtime,cranelift,gc,gc-drc,std,threads`), reference-host `Config`
//! (`wasm_exceptions(true)` + `shared_memory(true)` — hosts/otter-host-wasmtime/src/main.rs
//! verbatim). The artifact itself is EH-free (§3.3), so `wasm_exceptions` is not needed BY the
//! artifact — it is set because the gate's definition is "the real deployment shape, as-is"
//! (§8.2: 실배포 형상 정합).
//!
//! Phases (each prints a grep-gated marker; `just wasm-aot-e2e` asserts all of them + exit 0):
//!
//! 1. `AOT48-E2E-1-ENTRY-OK` — non-shared artifact, `otter_entry` called with the §2.4/§12.4 v1
//!    canonical shape `(ctx=0, args=0, argc=0)`, result bit-identical to the native T0 oracle
//!    (`otter::interp::run_module`). The program exercises every import row the manifest can
//!    carry: `otter.memory`, all 3 `otter_helpers.*` (Bin/Un/Convert), 2 `otter_host.*` names
//!    (`Host1` + `HostCtx1` with the baked `host_ctx` word), plus a marshaled non-tail `Call`.
//! 2. `AOT48-E2E-2-SHARED-LINK-OK` — the SAME program compiled `shared_memory=true`
//!    (`(memory 0 65536 shared)` import shape) instantiated against a real
//!    [`wasmtime::SharedMemory`], same oracle-identical result — the §8.2 "shared_memory=true
//!    형상 링크" leg.
//! 3. `AOT48-E2E-3-TAILBOUND-OK` — [스파이크] M2's figures re-confirmed on the 48 train:
//!    2,000,000/2,000,001-deep mutual `TailCall` recursion (`return_call_indirect` through the
//!    declared table) completes in a 64 KiB wasm stack, opposite parities.
//! 4. `AOT48-E2E-4-SHARED-TAILBOUND-OK` — the same 2M-deep bounded run on the SHARED-memory
//!    artifact: the shared-import × declared-table × `return_call_indirect` combination §2.7
//!    left as [추정→A-5 E2E에서 실측화], measured here as one artifact.
//! 5. `AOT48-E2E-5-NEGCTRL-OK` — negative control on the same shared deployment shape: identical
//!    guarded-callee module with ordinary `Call` in place of `TailCall` must die with
//!    [`Trap::StackOverflow`] — isolating phase 3/4's boundedness to `return_call_indirect`
//!    itself (the aot.rs gate-② discipline, 48 edition).
//!
//! `AOT48-E2E-ALL-OK` seals the run.
//!
//! The argc>0 legs (phases 3-5) drive their depth through a 0-param `boot` entry function that
//! marshals the argument GUEST-SIDE (`MOV_IMM` + `TailCall`/`Call`), so the host never writes
//! guest memory: the §12.4 embedder args-block duty is deliberately NOT exercised from the host
//! here (the v1 canonical entry call is argc=0 — this binary calls `otter_entry` with argc=0 in
//! every phase).
//!
//! **`bprime-diff <lib.wasm>` mode (O-16c′ L-2, design §7 L-2 row "러너 로직은 e2e 크레이트에
//! 흡수" verbatim)**: the spike's b′ differential runner absorbed as a subcommand, re-run
//! against the PRODUCT b′ artifact (`just wasm-bprime`). Gates, not prints — B2 (import 0 +
//! export 4종 실물: `memory`/`otter_binop`/`otter_convert`/`otter_unop`, §12.3), B3 (the
//! 40,000-case differential vs the SAME native canonical helpers the oracle runs — exact i64
//! bit equality, 0 mismatches or exit 1), B4 (all 3 documented panic paths →
//! [`Trap::UnreachableCodeReached`], instance survives). Markers: `BPRIME-B2-SURFACE-OK`,
//! `BPRIME-B3-DIFF-OK`, `BPRIME-B4-PANIC-OK`, `BPRIME-DIFF-ALL-OK`.
//!
//! **O-16c′ L-4 modes (design §7 L-4 row / §5.5 — the e2e binary does EXECUTION AND DIFFING
//! ONLY; external tool calls (`wasm-merge`, pinned `wasm-tools`, `otter-aot-link`) are the
//! `just wasm-aot-link-e2e` recipe's, never this crate's)**:
//! - `emit-aot <dir>` — write the three real `compile_module` products (the single-lowering-
//!   source rule: generated NOW by the live emitter, never pre-emitted bytes that could drift)
//!   plus their JSON manifest sidecars, serialized through otter-aot-link's `schema` module
//!   (the §3.1/§0-7 single schema owner — this crate path-deps it as the PRODUCER side):
//!   `calc` (host-carrying, non-shared), `deep` (self-contained, 2M-deep tail recursion,
//!   non-shared), `calc_shared` (host-carrying, shared-memory import shape).
//! - `run-linked calc|deep|calc-shared <linked.wasm>` — instantiate a LINKED artifact (its
//!   memory is now the b′-owned definition — no memory extern) resolving ONLY residual
//!   `otter_host.*` imports by name, call `otter_entry(0,0,0)`, and compare against the
//!   native T0 oracle (`calc`/`calc-shared`) or the parity contract (`deep`, 2M in the
//!   64 KiB wasm stack — the bounded-tail claim re-proven THROUGH the linked artifact).
//!   Markers: `AOT48-E2E-6-LINKED-{CALC,DEEP,SHARED}-OK`.
//! - `linked-diff <linked.wasm>` — the §5.3 corpus reinforcement (panel G-N-3): the L-2
//!   40,000-case helper differential re-run ONCE through a linked artifact's (pre-shrink,
//!   export-union) helper exports, plus the B4 panic-path probes. Import-0 asserted (run on
//!   the self-contained deep product). Markers: `LINKED-DIFF-B3-OK`, `LINKED-DIFF-B4-OK`,
//!   `LINKED-DIFF-ALL-OK`.
//! - `assert-instantiate <wasm>` — the wasmtime instantiation assertion the design moved out
//!   of L-1 (§0-5, panel MF-4: otter-aot-link stays wasmtime-FREE, so the assertion lives
//!   here, wasmtime's existing home). Used by the recipe on the per-product marker-injected
//!   b′ gate copies (both profiles — the shared run is [추정 ④]'s engine-acceptance leg).
//!   Marker: `INSTANTIATE-OK`.
//!
//! **O-16c′ L-5 mode (design §7 L-5 row / §5.5)**: `run-component calc|deep <component.wasm>`
//! — the AOT48-E2E-7 component execution differential via the wasmtime component API
//! (`component-model` feature): `calc` wires the LIFTED host imports (`h-add7`/`hc-xor`,
//! §2-4-3 — the [추정]② execution leg) through the component `Linker` and asserts oracle
//! bit-identity; `deep` re-proves the 2M bounded-tail claim INSIDE the component wrapper on
//! the 64 KiB wasm stack. The embed/new chain itself is `just wasm-aot-component`'s (external
//! tool calls never live in this crate).
//!
//! **O-19 C3 mode (design `otter-o19-wasm64-design.md` §4/§7 C3)**: `a64-diff` — the A64
//! (memory64) L1 execution gate, the (c) leg of the O-19 gate triple. The SAME IR compiled
//! A32 and A64 (`AotOpts::with_memory64(true)`) must round-trip to the SAME value as the
//! native T0 oracle, on wasmtime 48 with **explicit** `wasm_memory64(true)` (48/45 measured
//! default-ON, S2-M1-6 — the explicit opt-in is the §8-1 default-reversal defense, wired
//! through the ONE `reference_config` builder as a parameter, never a fork — tech-S1) and a
//! host-side [`MemoryType::new64`] import memory (the S2-M4 shape, premise ⑥). Negatives:
//! a 32-bit host memory must REFUSE to link against `(memory i64 0)` (S2-M4 measured
//! `incompatible import type`), and an explicit `wasm_memory64(false)` Config must REJECT
//! the A64 product at `Module::new` (Err asserted, NO message-string pin — §4 tech-S2 귀속
//! 정정) while still accepting the A32 product (non-vacuousness control). Plus the **table64
//! probe vector** (panel MF-1): the S2 probe shape verbatim (`table64` funcref table,
//! `i64`-indexed `call_indirect`, `go() → 7`), hand-encoded here — the L1 definition's
//! "memory64+**table64** 실행" second half as a product-level lane instead of spike-only
//! evidence; the EMITTED artifact itself stays the mixed shape (§2.1 [결정]).
//! Markers: `AOT48-E2E-8-A64-DIFF-OK`, `-A64-MIXED-TAIL-OK`, `-A64-NEG-MEM32-OK`,
//! `-A64-NEG-CONFIG-OK`, `-TABLE64-OK`, sealed by `AOT48-E2E-8-ALL-OK`.

use std::cell::RefCell;

use otter::interp::run_module;
use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, NumTy, OpDef, OpHints, UnOp, Value};
use otter_wasmemit::{
    AotArtifact, AotImportKind, AotOpts, HelperKind, HostImportName, compile_module, validate_aot_artifact,
};
use wasmtime::{
    Config, Engine, Extern, Func, Instance, Memory, MemoryType, Module, Result, SharedMemory, Store, Trap,
    bail,
};

// --- fixture constants (aot.rs suite conventions) -------------------------------------------------

/// [`AotOpts::scratch_base`] — O-16c′ L-4 raise (design §2-2 파급 정정 (b) verbatim): the old
/// page-1 base (0x10000) was harmless against host-glue stubs but VIOLATES the real-b′ link
/// shape (it sits inside the b′ shadow stack, SPK §5-2); 0x110000 equals the +atomics
/// heap_floor (≥ satisfied) and clears the non-shared floor — one base for both profiles.
const SCRATCH_BASE: u64 = 0x11_0000; // u64 since O-19 C1 (AotOpts::scratch_base widening)
/// O-16c′ L-4 companion raise (design §2-2 파급 정정 (c), panel G-SF-1/T-N5): the scratch
/// region now ends above page 17, so §5.2-(ii) coverage requires min ≥ 18 pages — raised
/// together with SCRATCH_BASE (the check-completeness discipline: one construction pass).
const MEMORY_PAGES: u32 = 18;
/// Per-call fuel for the shallow phases (runaway loops must trap loudly, not hang CI).
const FUEL: u64 = 100_000_000;
/// Fuel for the 2M-deep phases (aot.rs `DEEP_FUEL` — orders of magnitude of headroom).
const DEEP_FUEL: u64 = 50_000_000_000;
/// [스파이크] M2's exact wasm stack size ("64 KiB 스택") — decisive for both the bounded claim
/// and the negative control (wasmtime's default is 512 KiB).
const TINY_WASM_STACK: usize = 65_536;
/// The gate-①/② depth ([스파이크] M2 / aot.rs).
const DEPTH: i32 = 2_000_000;

const FILL: Value = 0x5EED_F00D_5EED_F00D_u64 as Value;
const HOST_CTX: Value = 0x7E57_C0DE_7E57_C0DE_u64 as Value;

// --- host builtins (baked into the IR; the SAME fns run natively in the oracle) -------------------

extern "C" fn h_add7(x: Value) -> Value {
    x.wrapping_add(7)
}
extern "C" fn hc_xor(host_ctx: Value, x: Value) -> Value {
    host_ctx ^ x
}

// --- minimal test ISA (the aot.rs fixture conventions, only the opcodes these phases need) --------

const MOV_IMM: u16 = 0; // local[op0] = sign-extended i32 immediate op1
const RET: u16 = 1; // return local[op0]
const JUMP_IF: u16 = 2; // if local[op0] != 0 goto op1
const CALL: u16 = 3; // Call dest=op0 base=op1 argc=op2 (callee at l[base], args at l[base+1..])
const TAILCALL: u16 = 4; // TailCall base=op0 argc=op1
const ADD_I64: u16 = 5;
const SUB_I64: u16 = 6;
const MUL_I64: u16 = 7;
const EQ_I64: u16 = 8;
const HOST_ADD7: u16 = 9; // local[op0] = h_add7(local[op1])            — otter_host leg (Host1)
const HOSTCTX_XOR: u16 = 10; // local[op0] = hc_xor(host_ctx, local[op1]) — otter_host leg (HostCtx1)
const NEG_I64: u16 = 11; // local[op0] = -local[op1]                     — otter_unop leg
const CONV_I64_F64: u16 = 12; // local[op0] = f64(local[op1])            — otter_convert leg
const CONV_F64_I64: u16 = 13; // local[op0] = i64(local[op1] as f64)
const COPY: u16 = 14; // local[op0] = local[op1]

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

fn bin(name: &'static str, op: ArithOp) -> OpDef {
    set0(
        name,
        Expr::Bin(
            BinOp::new(NumTy::I64, op),
            Box::new(Expr::LocalAt(1)),
            Box::new(Expr::LocalAt(2)),
        ),
    )
}

/// Build the fixture ISA. Opcode layout MUST match the constants above (asserted).
fn isa() -> Isa {
    let ops = vec![
        set0("mov_imm", Expr::OperandImm(1)),
        terminator(
            "ret",
            Effect::Return {
                value: Expr::LocalAt(0),
            },
        ),
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
        bin("add_i64", ArithOp::Add),
        bin("sub_i64", ArithOp::Sub),
        bin("mul_i64", ArithOp::Mul),
        bin("eq_i64", ArithOp::Eq),
        set0("host_add7", Expr::Host1(h_add7, Box::new(Expr::LocalAt(1)))),
        set0("hostctx_xor", Expr::HostCtx1(hc_xor, Box::new(Expr::LocalAt(1)))),
        set0(
            "neg_i64",
            Expr::Un(UnOp::Neg(NumTy::I64), Box::new(Expr::LocalAt(1))),
        ),
        set0(
            "conv_i64_f64",
            Expr::Un(
                UnOp::Convert {
                    from: NumTy::I64,
                    to: NumTy::F64,
                },
                Box::new(Expr::LocalAt(1)),
            ),
        ),
        set0(
            "conv_f64_i64",
            Expr::Un(
                UnOp::Convert {
                    from: NumTy::F64,
                    to: NumTy::I64,
                },
                Box::new(Expr::LocalAt(1)),
            ),
        ),
        set0("copy", Expr::LocalAt(1)),
    ];
    assert_eq!(ops.len(), COPY as usize + 1, "opcode constants out of sync");
    Isa { ops }
}

// --- IR programs ----------------------------------------------------------------------------------

/// Phase-1/2 program: an argc=0 entry that walks EVERY import kind — all 3 canonical helpers
/// (`Bin`/`Un`/`Convert`), both host namespaces' shapes (`Host1` + baked-ctx `HostCtx1`), and a
/// marshaled non-tail `Call` into a second function.
fn calc_module() -> otter::Module {
    let f0 = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 7,
        code: vec![
            Inst::new(MOV_IMM, 0, 41, 0),     // l0 = 41
            Inst::new(MOV_IMM, 1, 7, 0),      // l1 = 7
            Inst::new(ADD_I64, 2, 0, 1),      // l2 = 48            (otter_binop)
            Inst::new(HOST_ADD7, 3, 2, 0),    // l3 = h_add7(48)    (otter_host, Host1)
            Inst::new(HOSTCTX_XOR, 3, 3, 0),  // l3 = HOST_CTX ^ l3 (otter_host, HostCtx1)
            Inst::new(NEG_I64, 3, 3, 0),      // l3 = -l3           (otter_unop)
            Inst::new(CONV_I64_F64, 4, 3, 0), // l4 = f64(l3)       (otter_convert)
            Inst::new(CONV_F64_I64, 4, 4, 0), // l4 = i64(l4)       (otter_convert)
            Inst::new(MOV_IMM, 5, 1, 0),      // callee = f1
            Inst::new(COPY, 6, 4, 0),         // arg block: l6 = l4
            Inst::new(CALL, 4, 5, 1),         // l4 = f1(l6) — marshaled through §2.5 scratch
            Inst::new(RET, 4, 0, 0),
        ],
    };
    let f1 = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(MOV_IMM, 1, 3, 0),
            Inst::new(MUL_I64, 2, 0, 1), // x*3
            Inst::new(MOV_IMM, 1, 1, 0),
            Inst::new(ADD_I64, 2, 2, 1), // +1
            Inst::new(RET, 2, 0, 0),
        ],
    };
    otter::Module {
        funcs: vec![f0, f1],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    }
}

/// aot.rs `tail_fn`, callee indices shifted for the 3-function boot layout: if n == 0 return
/// `base_val` else `TailCall callee(n-1)`.
fn tail_shape(callee_idx: i32, base_val: i32) -> FuncDef {
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(MOV_IMM, 4, 0, 0),
            Inst::new(EQ_I64, 1, 0, 4),
            Inst::new(JUMP_IF, 1, 7, 0),
            Inst::new(MOV_IMM, 4, 1, 0),
            Inst::new(SUB_I64, 3, 0, 4),
            Inst::new(MOV_IMM, 2, callee_idx, 0),
            Inst::new(TAILCALL, 2, 1, 0), // callee at l2, arg at l3
            Inst::new(MOV_IMM, 4, base_val, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    }
}

/// Phase-3/4 module: 0-param `boot` (the argc=0 `otter_entry`) marshals `depth` GUEST-SIDE and
/// `TailCall`s into the even/odd mutual-recursion pair — so the very first act of the entry call
/// is already a `return_call_indirect` through the declared table.
fn deep_module(depth: i32) -> otter::Module {
    let boot = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 2,
        code: vec![
            Inst::new(MOV_IMM, 0, 1, 0), // callee = f1 (even)
            Inst::new(MOV_IMM, 1, depth, 0),
            Inst::new(TAILCALL, 0, 1, 0),
        ],
    };
    otter::Module {
        funcs: vec![boot, tail_shape(2, 1), tail_shape(1, 0)], // even → odd → even → …
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    }
}

/// aot.rs `nontail_call_fn`: the IDENTICAL guarded shape but ordinary `Call` — every recursive
/// step grows a real wasm frame (the negative control's isolation of `return_call_indirect`).
fn nontail_shape(callee_idx: i32, base_val: i32) -> FuncDef {
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 5, 0, 0),
            Inst::new(EQ_I64, 1, 0, 5),
            Inst::new(JUMP_IF, 1, 8, 0),
            Inst::new(MOV_IMM, 5, 1, 0),
            Inst::new(SUB_I64, 3, 0, 5),
            Inst::new(MOV_IMM, 2, callee_idx, 0),
            Inst::new(CALL, 4, 2, 1), // the ONE structural difference from tail_shape
            Inst::new(RET, 4, 0, 0),
            Inst::new(MOV_IMM, 4, base_val, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    }
}

/// Phase-5 module: `deep_module` with `Call` in place of `TailCall` everywhere.
fn nontail_module(depth: i32) -> otter::Module {
    let boot = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(MOV_IMM, 0, 1, 0),
            Inst::new(MOV_IMM, 1, depth, 0),
            Inst::new(CALL, 2, 0, 1),
            Inst::new(RET, 2, 0, 0),
        ],
    };
    otter::Module {
        funcs: vec![boot, nontail_shape(2, 1), nontail_shape(1, 0)],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    }
}

// --- compile + instantiate ------------------------------------------------------------------------

/// The embedder's name map (§2.2) — what a real `resolve_host` knows.
fn host_name(p: usize) -> Option<&'static str> {
    if p == h_add7 as otter::HostFn1 as usize {
        Some("h_add7")
    } else if p == hc_xor as otter::HostFnCtx1 as usize {
        Some("hc_xor")
    } else {
        None
    }
}

/// `compile_module` with a resolution-order-logging resolver (aot.rs `compile_with_log`): the
/// §2.3/§4.1 norms make the log order the `manifest.host_slots`/import order, so it doubles as
/// the harness's ptr map when building externs.
fn compile_logged(isa: &Isa, m: &otter::Module, opts: &AotOpts) -> Result<(AotArtifact, Vec<usize>)> {
    let order: RefCell<Vec<usize>> = RefCell::new(Vec::new());
    let resolve = |p: usize| -> Option<HostImportName> {
        let name = host_name(p)?;
        order.borrow_mut().push(p);
        Some(HostImportName(name.to_string()))
    };
    let artifact = match compile_module(isa, m, &resolve, opts) {
        Ok(a) => a,
        Err(e) => bail!("compile_module failed: {e:?}"),
    };
    if let Err(e) = validate_aot_artifact(&artifact.bytes) {
        bail!("pre-link artifact validator rejected compile_module's own output: {e:?}");
    }
    Ok((artifact, order.into_inner()))
}

/// The one memory extern, in the artifact's declared shape (§2.7): plain for `(memory 0)`, a real
/// [`SharedMemory`] for `(memory 0 65536 shared)` — sharedness must match EXACTLY (no coercion),
/// so a successful instantiation is itself the shape proof in both directions.
enum GuestMem {
    Plain(Memory),
    Shared(SharedMemory),
}

/// Build the externs in `manifest.imports` order (the b′ export-requirement spec order — §2.2),
/// delegating every helper to the SAME native fn the oracle runs (single canonical semantics; in
/// the real M2a deployment these are the b′ engine-library exports, §12.3).
fn build_externs(
    store: &mut Store<()>,
    man: &otter_wasmemit::AotManifest,
    mem: &GuestMem,
    order: &[usize],
) -> Result<Vec<Extern>> {
    let mut host_order = order.iter();
    let mut externs = Vec::with_capacity(man.imports.len());
    for imp in &man.imports {
        externs.push(match imp.kind {
            AotImportKind::Memory => match mem {
                GuestMem::Plain(m) => Extern::Memory(*m),
                GuestMem::Shared(m) => Extern::SharedMemory(m.clone()),
            },
            AotImportKind::Helper(HelperKind::Binop) => Extern::Func(Func::wrap(
                &mut *store,
                |a: i64, b: i64, ty: u32, op: u32| -> i64 { otter::otter_binop(a, b, ty, op) },
            )),
            AotImportKind::Helper(HelperKind::Convert) => {
                Extern::Func(Func::wrap(&mut *store, |a: i64, from: u32, to: u32| -> i64 {
                    otter::otter_convert(a, from, to)
                }))
            }
            AotImportKind::Helper(HelperKind::Unop) => {
                Extern::Func(Func::wrap(&mut *store, |a: i64, ty: u32, op: u32| -> i64 {
                    otter::otter_unop(a, ty, op)
                }))
            }
            AotImportKind::Host { .. } => {
                let ptr = *host_order
                    .next()
                    .ok_or_else(|| wasmtime::Error::msg("more Host imports than resolved pointers"))?;
                if ptr == h_add7 as otter::HostFn1 as usize {
                    Extern::Func(Func::wrap(&mut *store, |x: i64| -> i64 { h_add7(x) }))
                } else if ptr == hc_xor as otter::HostFnCtx1 as usize {
                    Extern::Func(Func::wrap(&mut *store, |c: i64, x: i64| -> i64 { hc_xor(c, x) }))
                } else {
                    bail!("unknown host fn pointer in resolution log");
                }
            }
            // `AotImportKind` is `#[non_exhaustive]` — fail CLOSED on a future kind (a silently
            // skipped import would misalign the whole extern list).
            ref other => bail!("unhandled AotImportKind: {other:?}"),
        });
    }
    Ok(externs)
}

/// One instantiated deployment on the §8.2 host shape. `tiny_stack` swaps in the 64 KiB wasm
/// stack for the depth phases. Since O-19 C3 the memory shape keys on the manifest: an A64
/// artifact gets the explicit `wasm_memory64(true)` opt-in (§4 default-reversal defense) and a
/// [`MemoryType::new64`] host memory (premise ⑥ — the S2-M4 receiving-side shape); every A32
/// artifact leaves the knob untouched, so the pre-O-19 lanes' Config is unchanged.
fn instantiate(artifact: &AotArtifact, order: &[usize], tiny_stack: bool) -> Result<(Store<()>, Instance)> {
    // The reference host's Config VERBATIM — via the ONE builder (L-5, the L-4 review's
    // carried-over prescription: the inline duplicate here and `reference_config` were the
    // same bytes twice; a future Config edit touching only one copy would silently fork the
    // A-5 phases' host shape from the L-4/L-5 file modes').
    let memory64 = artifact.manifest.memory64;
    let engine = Engine::new(&reference_config(
        tiny_stack,
        if memory64 { Some(true) } else { None },
    ))?;
    let module = Module::new(&engine, &artifact.bytes)?;
    let mut store = Store::new(&engine, ());
    let mem = if artifact.manifest.shared_memory {
        // shared × memory64 is refused at compile_module (fail-closed, O-19 §2.1) — this arm
        // is A32-only by construction.
        GuestMem::Shared(SharedMemory::new(
            &engine,
            MemoryType::shared(MEMORY_PAGES, MEMORY_PAGES),
        )?)
    } else if memory64 {
        GuestMem::Plain(Memory::new(
            &mut store,
            MemoryType::new64(u64::from(MEMORY_PAGES), None),
        )?)
    } else {
        GuestMem::Plain(Memory::new(&mut store, MemoryType::new(MEMORY_PAGES, None))?)
    };
    let externs = build_externs(&mut store, &artifact.manifest, &mem, order)?;
    let instance = Instance::new(&mut store, &module, &externs)?;
    Ok((store, instance))
}

/// Call `otter_entry` in the §2.4/§12.4 v1 canonical shape: `(ctx=0, args=0, argc=0)` — argc=0
/// makes args=0 safe (the prologue argc-guard never touches the pointer). `memory64` picks the
/// entry's wasm type: the ctx/args params are POINTERS, so they widen to i64 on an A64 artifact
/// (COMPILED_TY_A64, O-19 §2.2 #1) — pass `manifest.memory64`.
fn call_entry(store: &mut Store<()>, instance: &Instance, fuel: u64, memory64: bool) -> Result<Value> {
    store.set_fuel(fuel)?;
    if memory64 {
        let f = instance.get_typed_func::<(i64, i64, i64), i64>(&mut *store, "otter_entry")?;
        f.call(&mut *store, (0, 0, 0))
    } else {
        let f = instance.get_typed_func::<(i32, i32, i64), i64>(&mut *store, "otter_entry")?;
        f.call(&mut *store, (0, 0, 0))
    }
}

// --- bprime-diff mode (O-16c′ L-2 — the spike differential runner, absorbed as a gate) -----------

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Normalize a raw 64-bit word to `ty`'s canonical slot form (floats: low IEEE bits,
/// zero-extended; ints: otter's own norm) — spike-verbatim.
fn bprime_normalize(ty: u32, raw: u64) -> Value {
    match ty {
        8 => i64::from(raw as u16), // F16
        9 => i64::from(raw as u32), // F32
        10 => raw as i64,           // F64
        _ => NumTy::from_tag(ty).norm(raw as i64),
    }
}

/// Interesting seed words per type family (edges + NaN payloads + infinities) — spike-verbatim.
fn bprime_specials(ty: u32) -> Vec<i64> {
    match ty {
        8 => vec![
            0x0000, 0x8000, 0x3C00, 0x7C00, 0xFC00, 0x7E00, 0x7E01, 0xFE01, 0x7BFF, 0x0001, 0x83FF,
        ],
        9 => vec![
            0u32,
            0x8000_0000,
            0x3F80_0000,
            0x7F80_0000,
            0xFF80_0000,
            0x7FC0_0000,
            0x7FC0_0001,
            0x7F7F_FFFF,
            1,
        ]
        .into_iter()
        .map(i64::from)
        .collect(),
        10 => vec![
            0u64,
            0x8000_0000_0000_0000,
            0x3FF0_0000_0000_0000,
            0x7FF0_0000_0000_0000,
            0xFFF0_0000_0000_0000,
            0x7FF8_0000_0000_0000,
            0x7FF8_0000_0000_0001,
            1,
        ]
        .into_iter()
        .map(|x| x as i64)
        .collect(),
        _ => vec![
            0,
            1,
            -1,
            i64::MAX,
            i64::MIN,
            127,
            -128,
            255,
            65535,
            0x7FFF_FFFF,
            -0x8000_0000,
        ],
    }
}

/// The B2–B4 replay gate against the product b′ artifact (spike bprime-runner absorbed; every
/// probe is now an ASSERT — a mismatch/no-trap is exit 1, not a print). The engine is
/// `Engine::default()` with zero imports, spike-verbatim: b′ is import-free by construction
/// (SPK B2 — no_std, no wasi-libc, no WASI imports), so a successful `Instance::new(&[])` IS
/// the import-0 proof.
fn bprime_diff(lib_path: &str) -> Result<()> {
    let engine = Engine::default();
    let module = Module::from_file(&engine, lib_path)?;

    // ---- B2: surface — import 0 + export 4종 실물 (§12.3 duty, names verbatim) ----------------
    let imports: Vec<_> = module.imports().collect();
    if !imports.is_empty() {
        bail!("B2: b′ must import NOTHING, found {} import(s)", imports.len());
    }
    let export_names: Vec<String> = module.exports().map(|e| e.name().to_string()).collect();
    if export_names != ["memory", "otter_binop", "otter_convert", "otter_unop"] {
        bail!("B2: export surface {export_names:?} != [memory, otter_binop, otter_convert, otter_unop]");
    }
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])?;
    println!("BPRIME-B2-SURFACE-OK imports=0 exports=memory+otter_binop+otter_convert+otter_unop");

    let w_binop = instance.get_typed_func::<(i64, i64, i32, i32), i64>(&mut store, "otter_binop")?;
    let w_convert = instance.get_typed_func::<(i64, i32, i32), i64>(&mut store, "otter_convert")?;
    let w_unop = instance.get_typed_func::<(i64, i32, i32), i64>(&mut store, "otter_unop")?;

    diff_b3(&mut store, &w_binop, &w_convert, &w_unop, "BPRIME-B3-DIFF-OK")?;
    diff_b4(&mut store, &w_binop, &w_unop, "BPRIME-B4-PANIC-OK")?;

    println!("BPRIME-DIFF-ALL-OK");
    Ok(())
}

/// Helper-export handles the two differential legs share (bprime-diff on the b′ library,
/// linked-diff on a linked artifact's export union — §5.3 corpus reinforcement).
type WBinop = wasmtime::TypedFunc<(i64, i64, i32, i32), i64>;
type WConvUn = wasmtime::TypedFunc<(i64, i32, i32), i64>;

/// The B3 leg: the pinned 40,000-case differential vs the native canonical helpers.
fn diff_b3(
    store: &mut Store<()>,
    w_binop: &WBinop,
    w_convert: &WConvUn,
    w_unop: &WConvUn,
    marker: &str,
) -> Result<()> {
    let mut rng: u64 = 0x0716_C0B9_D00D_F00D;
    let mut cases: u64 = 0;
    let mut mismatches: u64 = 0;
    const N_RANDOM: usize = 200;

    let inputs_for = |ty: u32, rng: &mut u64| -> Vec<i64> {
        let mut v: Vec<i64> = bprime_specials(ty)
            .into_iter()
            .map(|w| bprime_normalize(ty, w as u64))
            .collect();
        for _ in 0..N_RANDOM {
            v.push(bprime_normalize(ty, splitmix64(rng)));
        }
        v
    };

    // binop: all 11 types × {Add,Sub,Mul,Div,Lt,Eq}, minus int×Div (documented panic contract).
    for ty in 0u32..11 {
        let is_float = ty >= 8;
        let xs = inputs_for(ty, &mut rng);
        for op in 0u32..6 {
            if op == 3 && !is_float {
                continue; // integer Div: panic contract, probed in B4
            }
            for i in 0..xs.len() {
                let a = xs[i];
                let b = xs[(i * 7 + 13) % xs.len()];
                let native = otter::otter_binop(a, b, ty, op);
                let wasm = w_binop.call(&mut *store, (a, b, ty as i32, op as i32))?;
                cases += 1;
                if native != wasm {
                    mismatches += 1;
                    if mismatches <= 10 {
                        println!(
                            "MISMATCH binop ty={ty} op={op} a={a:#x} b={b:#x} native={native:#x} wasm={wasm:#x}"
                        );
                    }
                }
            }
        }
    }
    // convert: all 11 × 11 (from, to) pairs.
    for from in 0u32..11 {
        let xs = inputs_for(from, &mut rng);
        for to in 0u32..11 {
            for &a in &xs {
                let native = otter::otter_convert(a, from, to);
                let wasm = w_convert.call(&mut *store, (a, from as i32, to as i32))?;
                cases += 1;
                if native != wasm {
                    mismatches += 1;
                    if mismatches <= 10 {
                        println!("MISMATCH convert {from}->{to} a={a:#x} native={native:#x} wasm={wasm:#x}");
                    }
                }
            }
        }
    }
    // unop: Neg at all 11 types.
    for ty in 0u32..11 {
        let xs = inputs_for(ty, &mut rng);
        for &a in &xs {
            let native = otter::otter_unop(a, ty, 0);
            let wasm = w_unop.call(&mut *store, (a, ty as i32, 0))?;
            cases += 1;
            if native != wasm {
                mismatches += 1;
                if mismatches <= 10 {
                    println!("MISMATCH unop ty={ty} a={a:#x} native={native:#x} wasm={wasm:#x}");
                }
            }
        }
    }
    // The spike's corpus arithmetic lands on exactly 40,000 — pinned so a silently shrunk
    // corpus (e.g. a specials-table edit) can never weaken the gate.
    if cases != 40_000 {
        bail!("B3: corpus drifted — ran {cases} cases, the pinned corpus is 40000");
    }
    if mismatches != 0 {
        bail!("B3: {mismatches} mismatch(es) out of {cases} cases");
    }
    println!("{marker} cases={cases} mismatches=0");
    Ok(())
}

/// The B4 leg: the 3 documented panic paths → deterministic unreachable; instance survives.
/// (Sequential calls: a b′ trap never poisons the store/instance — that survival IS part of
/// the B4 claim, re-asserted by the post-trap call below.)
fn diff_b4(store: &mut Store<()>, w_binop: &WBinop, w_unop: &WConvUn, marker: &str) -> Result<()> {
    let probes: [(&str, Result<i64>); 3] = [
        (
            "int-div unreachable! (ty=I32,op=Div)",
            w_binop.call(&mut *store, (6, 3, 2, 3)),
        ),
        (
            "NumTy::from_tag OOB (ty=99)",
            w_binop.call(&mut *store, (1, 1, 99, 0)),
        ),
        (
            "otter_unop unknown tag (op=7)",
            w_unop.call(&mut *store, (1, 8, 7)),
        ),
    ];
    for (name, result) in probes {
        match result {
            Ok(v) => bail!("B4: {name} returned {v:#x} instead of trapping"),
            Err(e) => match e.downcast_ref::<Trap>() {
                Some(Trap::UnreachableCodeReached) => {}
                other => bail!("B4: {name} — expected UnreachableCodeReached, got {other:?} ({e})"),
            },
        }
    }
    let after = w_binop.call(&mut *store, (0x3C00, 0x3C00, 8, 0))?; // f16 1.0+1.0
    if after != 0x4000 {
        bail!("B4: post-trap f16 1.0+1.0 = {after:#x}, want 0x4000 (instance must survive)");
    }
    println!("{marker} probes=3 trap=UnreachableCodeReached post-trap=0x4000");
    Ok(())
}

// --- O-16c′ L-4 modes (design §7 L-4 row / §5.5 — execution & diffing only) ----------------------

/// The reference-host Config VERBATIM (hosts/otter-host-wasmtime): `wasm_exceptions(true)`
/// (requires the gc/gc-drc crate features) + `shared_memory(true)`; `consume_fuel` is this
/// gate's own addition (runaway protection), same as every wasmemit test harness. Since L-5
/// this is the SINGLE Config builder for every mode in this binary — the A-5 phases
/// (`instantiate`), the L-4 file modes, the L-5 component mode (the L-4 review's dedup
/// carry-over), and since O-19 C3 the A64 lane too.
///
/// `memory64` (O-19 §4, tech-S1 — a PARAMETER on the one builder, never a duplicate):
/// - `None` — the knob is left UNTOUCHED: every pre-O-19 lane calls this way, so their Config
///   is unchanged by construction (pinned by the `reference_config_memory64_param_no_drift`
///   unit test's verbatim-twin compare);
/// - `Some(true)` — the A64 lane's EXPLICIT opt-in. 48.0.1/45.0.3 both measured default-ON
///   (S2-M1-6), so this is semantically a no-op today; it is kept explicit as the §8-1
///   default-reversal defense (wasmtime's own config.rs:1220 doc still says "false by
///   default" — the documented-vs-measured gap makes a future reversal plausible);
/// - `Some(false)` — the negative-control Config: must REJECT A64/table64 products.
fn reference_config(tiny_stack: bool, memory64: Option<bool>) -> Config {
    let mut config = Config::new();
    config.wasm_exceptions(true);
    config.shared_memory(true);
    config.consume_fuel(true);
    if tiny_stack {
        config.max_wasm_stack(TINY_WASM_STACK);
    }
    if let Some(enable) = memory64 {
        config.wasm_memory64(enable);
    }
    config
}

/// `emit-aot <dir>`: write the three real products + JSON manifest sidecars (single schema
/// owner: otter-aot-link's `schema` module — §3.1/§0-7 producer side).
fn emit_aot(dir: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let isa = isa();
    let products: [(&str, otter::Module, bool); 3] = [
        ("calc", calc_module(), false),
        ("deep", deep_module(DEPTH), false),
        ("calc_shared", calc_module(), true),
    ];
    for (name, module, shared) in &products {
        let opts = AotOpts::new(SCRATCH_BASE).with_shared_memory(*shared);
        let (artifact, _order) = compile_logged(&isa, module, &opts)?;
        let sidecar = otter_aot_link::schema::ManifestSidecar::try_from(&artifact.manifest)
            .map_err(|e| wasmtime::Error::msg(format!("schema bridge: {e}")))?;
        let json = otter_aot_link::schema::manifest_to_json(&sidecar)
            .map_err(|e| wasmtime::Error::msg(format!("schema serialize: {e}")))?;
        let wasm_path = format!("{dir}/{name}.wasm");
        let json_path = format!("{dir}/{name}.manifest.json");
        std::fs::write(&wasm_path, &artifact.bytes)?;
        std::fs::write(&json_path, &json)?;
        let (base, end) = artifact.manifest.scratch_region;
        println!(
            "EMIT-AOT-PRODUCT name={name} shared={shared} bytes={} scratch=[{base:#x},{end:#x}) \
             hosts={} table={}",
            artifact.bytes.len(),
            artifact.manifest.host_slots.len(),
            artifact.manifest.table_size
        );
    }
    println!("EMIT-AOT-OK dir={dir} products=3");
    Ok(())
}

/// Instantiate a LINKED artifact from `path` (memory is the b′-owned definition inside — no
/// memory extern) resolving only residual `otter_host.*` imports BY NAME, then call
/// `otter_entry(0,0,0)`. Any other residual import is a loud failure (belt-and-suspenders
/// after `check --imports` — §5.1: only the manifest's otter_host set may remain).
fn run_linked_file(path: &str, tiny_stack: bool, fuel: u64) -> Result<Value> {
    // Knob untouched (`None`): the link/sidecar chain is wasm32-only (O-19 §2.5).
    let engine = Engine::new(&reference_config(tiny_stack, None))?;
    let module = Module::from_file(&engine, path)?;
    let mut store = Store::new(&engine, ());
    let mut externs: Vec<Extern> = Vec::new();
    for imp in module.imports() {
        if imp.module() != "otter_host" {
            bail!(
                "linked artifact residual import `{}.{}` — only otter_host may remain (§5.1)",
                imp.module(),
                imp.name()
            );
        }
        externs.push(match imp.name() {
            "h_add7" => Extern::Func(Func::wrap(&mut store, |x: i64| -> i64 { h_add7(x) })),
            "hc_xor" => Extern::Func(Func::wrap(&mut store, |c: i64, x: i64| -> i64 { hc_xor(c, x) })),
            other => bail!("unknown otter_host import `{other}` (embedder name map has h_add7/hc_xor)"),
        });
    }
    // Fuel BEFORE instantiation: the shared-profile linked artifact carries the init-only
    // `__wasm_init_memory` start (§5.3), which executes AT Instance::new — with the store's
    // initial 0 fuel it would trap OutOfFuel before the entry call ever runs (measured in the
    // L-4 bring-up). call_entry re-arms the budget for the entry call itself.
    store.set_fuel(fuel)?;
    let instance = Instance::new(&mut store, &module, &externs)?;
    call_entry(&mut store, &instance, fuel, false)
}

/// `run-linked <shape> <linked.wasm>` — the AOT48-E2E-6 execution differential, per shape.
fn run_linked(shape: &str, path: &str) -> Result<()> {
    let isa = isa();
    match shape {
        // Host-carrying entry program, non-shared: bit-identical to the native T0 oracle.
        "calc" => {
            let expected = run_module(&isa, &calc_module(), &[]);
            let got = run_linked_file(path, false, FUEL)?;
            if got != expected {
                bail!("run-linked calc: {got:#x} != oracle {expected:#x}");
            }
            println!("AOT48-E2E-6-LINKED-CALC-OK value={got:#x}");
        }
        // Same program, +atomics b′ × --enable-threads chain (§2-5-③): the linked artifact
        // DEFINES a shared memory — engine acceptance is part of the claim.
        "calc-shared" => {
            let expected = run_module(&isa, &calc_module(), &[]);
            let got = run_linked_file(path, false, FUEL)?;
            if got != expected {
                bail!("run-linked calc-shared: {got:#x} != oracle {expected:#x}");
            }
            println!("AOT48-E2E-6-LINKED-SHARED-OK value={got:#x}");
        }
        // Self-contained 2M-deep mutual tail recursion through the linked artifact, in the
        // 64 KiB wasm stack (the bounded-tail claim, linked edition). Parity contract like
        // phase 3 / aot.rs: even(2M) == 1 (the emitter baked DEPTH at emit-aot time).
        "deep" => {
            let got = run_linked_file(path, true, DEEP_FUEL)?;
            if got != 1 {
                bail!("run-linked deep: even({DEPTH}) = {got}, want 1");
            }
            println!("AOT48-E2E-6-LINKED-DEEP-OK depth={DEPTH} stack={TINY_WASM_STACK}");
        }
        other => bail!("unknown run-linked shape `{other}` (calc|deep|calc-shared)"),
    }
    Ok(())
}

/// `linked-diff <linked.wasm>` — the §5.3 corpus reinforcement (panel G-N-3): the 40k helper
/// differential + B4 probes through a linked artifact's export-union helper exports.
/// Run on the SELF-CONTAINED product (deep) pre-shrink: import 0 is asserted, surface is the
/// union (NOT the exact-4 b′ surface — that check stays bprime-diff's).
fn linked_diff(path: &str) -> Result<()> {
    let engine = Engine::new(&reference_config(false, None))?;
    let module = Module::from_file(&engine, path)?;
    let residual: Vec<String> = module
        .imports()
        .map(|i| format!("{}.{}", i.module(), i.name()))
        .collect();
    if !residual.is_empty() {
        bail!(
            "linked-diff expects a SELF-CONTAINED linked artifact, found import(s): {}",
            residual.join(", ")
        );
    }
    let mut store = Store::new(&engine, ());
    store.set_fuel(DEEP_FUEL)?;
    let instance = Instance::new(&mut store, &module, &[])?;
    let w_binop = instance.get_typed_func::<(i64, i64, i32, i32), i64>(&mut store, "otter_binop")?;
    let w_convert = instance.get_typed_func::<(i64, i32, i32), i64>(&mut store, "otter_convert")?;
    let w_unop = instance.get_typed_func::<(i64, i32, i32), i64>(&mut store, "otter_unop")?;
    diff_b3(&mut store, &w_binop, &w_convert, &w_unop, "LINKED-DIFF-B3-OK")?;
    diff_b4(&mut store, &w_binop, &w_unop, "LINKED-DIFF-B4-OK")?;
    println!("LINKED-DIFF-ALL-OK");
    Ok(())
}

/// `run-component <shape> <component.wasm>` — the AOT48-E2E-7 component execution
/// differential (O-16c′ L-5, design §5.5/§2-4-3). Instantiates a LIFTED component via the
/// wasmtime component API on the reference Config and calls the world export
/// `otter-entry(ctx=0, args=0, argc=0)` — the component-shape entry contract is argc=0 ONLY
/// (§6.1: the world lifts functions, memory is not lifted, the boundary is shared-nothing —
/// the §12.4 args-block marshaling path structurally does not exist here).
///
/// - `calc`: the HOST-LIFT measurement ([추정]② resolved here at the execution layer): the
///   component-level imports `h-add7`/`hc-xor` are wired through the component `Linker` to
///   the SAME native fns the T0 oracle runs, and the result must be bit-identical to that
///   oracle. Marker: `AOT48-E2E-7-COMPONENT-CALC-OK … host-wired=h-add7,hc-xor`.
/// - `deep`: self-contained (empty linker), 2M-deep mutual `return_call_indirect` INSIDE the
///   component wrapper, in the same 64 KiB wasm stack as every bounded-tail gate — the
///   spike's L5 deep run, tightened from the default stack to the TINY one. Marker:
///   `AOT48-E2E-7-COMPONENT-DEEP-OK`.
fn run_component(shape: &str, path: &str) -> Result<()> {
    use wasmtime::component::{Component, Linker};
    let isa = isa();
    let tiny_stack = shape == "deep";
    let fuel = if shape == "deep" { DEEP_FUEL } else { FUEL };
    let engine = Engine::new(&reference_config(tiny_stack, None))?;
    let bytes = std::fs::read(path)?;
    let component = Component::new(&engine, &bytes)?;
    let mut linker: Linker<()> = Linker::new(&engine);
    if shape == "calc" {
        // The §2-4-3 embedder wiring: component-level imports resolved by the embedder —
        // exactly the layer the design assigns host fns to (임베더 제공물), now behind the
        // shared-nothing component boundary.
        linker
            .root()
            .func_wrap("h-add7", |_store, (x,): (i64,)| Ok((h_add7(x),)))?;
        linker
            .root()
            .func_wrap("hc-xor", |_store, (c, x): (i64, i64)| Ok((hc_xor(c, x),)))?;
    }
    let mut store = Store::new(&engine, ());
    store.set_fuel(fuel)?;
    let instance = linker.instantiate(&mut store, &component)?;
    let entry = instance.get_typed_func::<(u32, u32, u64), (i64,)>(&mut store, "otter-entry")?;
    let (got,) = entry.call(&mut store, (0, 0, 0))?;
    match shape {
        "calc" => {
            let expected = run_module(&isa, &calc_module(), &[]);
            if got != expected {
                bail!("run-component calc: {got:#x} != oracle {expected:#x}");
            }
            println!("AOT48-E2E-7-COMPONENT-CALC-OK value={got:#x} host-wired=h-add7,hc-xor");
        }
        "deep" => {
            if got != 1 {
                bail!("run-component deep: even({DEPTH}) = {got}, want 1");
            }
            println!("AOT48-E2E-7-COMPONENT-DEEP-OK depth={DEPTH} stack={TINY_WASM_STACK}");
        }
        other => bail!("unknown run-component shape `{other}` (calc|deep)"),
    }
    Ok(())
}

/// `assert-instantiate <wasm>` — the §0-5 wasmtime instantiation assertion (moved from L-1:
/// otter-aot-link is wasmtime-free, THIS crate is wasmtime's home). Import-free modules only
/// (the marker-injected b′ gate copies): a successful `Instance::new(&[])` on the reference
/// Config IS the assertion, for the shared profile included ([추정 ④] engine acceptance).
fn assert_instantiate(path: &str) -> Result<()> {
    let engine = Engine::new(&reference_config(false, None))?;
    let module = Module::from_file(&engine, path)?;
    let residual: Vec<String> = module
        .imports()
        .map(|i| format!("{}.{}", i.module(), i.name()))
        .collect();
    if !residual.is_empty() {
        bail!(
            "assert-instantiate expects an import-free module, found: {}",
            residual.join(", ")
        );
    }
    let mut store = Store::new(&engine, ());
    store.set_fuel(FUEL)?;
    let _instance = Instance::new(&mut store, &module, &[])?;
    println!(
        "INSTANTIATE-OK file={} exports={}",
        std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string()),
        module.exports().count()
    );
    Ok(())
}

// --- O-19 C3 mode: the A64 (memory64) L1 execution gate (design §4/§7 C3) -------------------------

/// The table64 probe vector (design §4, panel MF-1) — the O-19 S2 probe shape VERBATIM
/// (`m_table64(elem_offset_i32=false)`, spike results §2 M1-3/4 + M4): a `table64` funcref
/// table, `i64.const 0` element offset, an `i64` `call_indirect` index, `go() → 7`. Hand-encoded
/// (wasm-encoder 0.258, the emitter's own pinned train) because the EMITTED product deliberately
/// stays the mixed shape (§2.1 [결정] — table64:false); this probe is what keeps the L1
/// definition's "memory64+**table64** 실행" second half a PRODUCT-LEVEL lane instead of
/// spike-only evidence.
fn table64_probe() -> Vec<u8> {
    use wasm_encoder::{
        CodeSection, ConstExpr, Elements, ExportKind, ExportSection, Function as WasmFunction,
        FunctionSection, Instruction, Module as WasmModule, RefType, TableSection, TableType, TypeSection,
        ValType,
    };
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I64]); // callee() -> 7 / go() -> call_indirect
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    funcs.function(0);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: true,
        minimum: 1,
        maximum: Some(1),
        shared: false,
    });
    let mut exports = ExportSection::new();
    exports.export("go", ExportKind::Func, 1);
    let mut elems = wasm_encoder::ElementSection::new();
    let off = ConstExpr::i64_const(0); // table64 forces the i64 element offset (S2 M1-3/4)
    elems.active(
        None,
        &off,
        Elements::Functions(std::borrow::Cow::Owned(vec![0u32])),
    );
    let mut callee = WasmFunction::new([]);
    callee.instruction(&Instruction::I64Const(7));
    callee.instruction(&Instruction::End);
    let mut go = WasmFunction::new([]);
    go.instruction(&Instruction::I64Const(0)); // table64: the call_indirect index operand is i64
    go.instruction(&Instruction::CallIndirect {
        type_index: 0,
        table_index: 0,
    });
    go.instruction(&Instruction::End);
    let mut code = CodeSection::new();
    code.function(&callee);
    code.function(&go);
    let mut m = WasmModule::new();
    m.section(&types)
        .section(&funcs)
        .section(&tables)
        .section(&exports)
        .section(&elems)
        .section(&code);
    m.finish()
}

/// `a64-diff` — the O-19 (c) gate (design §4 판정문, all five legs):
/// (i) validation is the C2 addrtype suite's (wasmparser 0.258 with its DEFAULT feature set,
/// which includes memory64, plus the `default & !MEMORY64` negative — defaults-pass implies
/// all-features-pass by monotonicity, so this stands in for `wasm-tools validate --features
/// all`; recorded judgment: no CLI wasm-tools dependency is added to this stable-only
/// recipe — label corrected per the closing review's F2), plus wasmtime 48's own compile
/// here; (ii)+(iii) A64 product instantiated and executed, round-trip value == the A32 product
/// of the SAME IR == the native T0 oracle; (iv) the negative controls; (v) the table64 vector.
fn a64_diff() -> Result<()> {
    let isa = isa();
    let calc = calc_module();
    let oracle = run_module(&isa, &calc, &[]);
    let opts64 = AotOpts::new(SCRATCH_BASE).with_memory64(true);

    // ---- the differential pair: SAME IR, A32 and A64, both against the native oracle ----------
    let (a32, o32) = compile_logged(&isa, &calc, &AotOpts::new(SCRATCH_BASE))?;
    let (mut store, instance) = instantiate(&a32, &o32, false)?;
    let got32 = call_entry(&mut store, &instance, FUEL, a32.manifest.memory64)?;
    if got32 != oracle {
        bail!("a64-diff: A32 result {got32:#x} != oracle {oracle:#x}");
    }
    let (a64, o64) = compile_logged(&isa, &calc, &opts64)?;
    if !a64.manifest.memory64 {
        bail!("a64-diff: memory64 flag did not reach the manifest");
    }
    let (mut store64, instance64) = instantiate(&a64, &o64, false)?;
    let got64 = call_entry(&mut store64, &instance64, FUEL, a64.manifest.memory64)?;
    if got64 != oracle || got64 != got32 {
        bail!("a64-diff: A64 result {got64:#x} != A32 {got32:#x} / oracle {oracle:#x}");
    }
    println!("AOT48-E2E-8-A64-DIFF-OK value={got64:#x} (A64 == A32 == native oracle)");

    // ---- mixed-shape execution leg: return_call_indirect through the 32-bit declared table
    // INSIDE the A64 product (the product-level analogue of S2-M4's mixed go()→42). Depth 20,
    // deliberately NOT the 2M bounded-tail re-measurement: boundedness is table machinery, and
    // the C2 modulo-addrtype gate proves that machinery byte-identical across addrtypes —
    // re-measuring it here would add minutes of CI for no new evidence (recorded judgment). ----
    let shallow = deep_module(20);
    let want = run_module(&isa, &shallow, &[]);
    let (a, o) = compile_logged(&isa, &shallow, &opts64)?;
    let (mut store, instance) = instantiate(&a, &o, true)?;
    let got = call_entry(&mut store, &instance, FUEL, a.manifest.memory64)?;
    if got != want {
        bail!("a64-diff: A64 even(20) = {got} != oracle {want}");
    }
    println!("AOT48-E2E-8-A64-MIXED-TAIL-OK even(20)={got} (32-bit table inside the A64 product)");

    // ---- negative (premise ⑥ receiving side): a 32-bit host memory must REFUSE to link
    // against the (memory i64 0) import — the S2-M4 measured `incompatible import type`
    // refusal, pinned loosely (the phrase was directly measured, unlike the config-negative's
    // message — see below). ----
    let engine = Engine::new(&reference_config(false, Some(true)))?;
    let module = Module::new(&engine, &a64.bytes)?;
    let mut store = Store::new(&engine, ());
    let mem32 = GuestMem::Plain(Memory::new(&mut store, MemoryType::new(MEMORY_PAGES, None))?);
    let externs = build_externs(&mut store, &a64.manifest, &mem32, &o64)?;
    match Instance::new(&mut store, &module, &externs) {
        Ok(_) => bail!("a64-diff: 32-bit host memory UNEXPECTEDLY linked against (memory i64 0)"),
        Err(e) => {
            let msg = format!("{e:?}");
            if !msg.contains("incompatible import type") {
                bail!("a64-diff: expected the S2-M4 incompatible-import link refusal, got: {msg}");
            }
        }
    }
    println!("AOT48-E2E-8-A64-NEG-MEM32-OK (32-bit host memory refused by the i64 memory import)");

    // ---- negative (§4 음성 대조): an EXPLICIT wasm_memory64(false) Config must reject the A64
    // product at Module::new. Err asserted, NO message-string pin (tech-S2: the v1-quoted
    // string belonged to a DIFFERENT module shape — a pinned wrong string is a misfire). The
    // same Config must still accept the A32 product (non-vacuousness: the red really is the
    // memory64 gate, not a broken Config). ----
    let engine_off = Engine::new(&reference_config(false, Some(false)))?;
    if Module::new(&engine_off, &a64.bytes).is_ok() {
        bail!("a64-diff: wasm_memory64(false) UNEXPECTEDLY accepted the A64 product");
    }
    if let Err(e) = Module::new(&engine_off, &a32.bytes) {
        bail!("a64-diff: wasm_memory64(false) must still accept the A32 product, got: {e:?}");
    }
    println!("AOT48-E2E-8-A64-NEG-CONFIG-OK (explicit wasm_memory64(false) rejects A64, accepts A32)");

    // ---- table64 probe vector (panel MF-1): S2 shape verbatim, go() → 7 on the explicit
    // opt-in Config; the same explicit-false Config must reject it too (S1 NEGCTRL2's shape —
    // Err asserted, no string pin). ----
    let t64 = table64_probe();
    let module = Module::new(&engine, &t64)?;
    let mut store = Store::new(&engine, ());
    store.set_fuel(FUEL)?;
    let instance = Instance::new(&mut store, &module, &[])?;
    let go = instance.get_typed_func::<(), i64>(&mut store, "go")?;
    let v = go.call(&mut store, ())?;
    if v != 7 {
        bail!("a64-diff: table64 go() = {v}, want 7");
    }
    if Module::new(&engine_off, &t64).is_ok() {
        bail!("a64-diff: wasm_memory64(false) UNEXPECTEDLY accepted the table64 probe");
    }
    println!("AOT48-E2E-8-TABLE64-OK go()=7 (i64-indexed call_indirect through a table64 table)");

    println!("AOT48-E2E-8-ALL-OK");
    Ok(())
}

// --- phases ---------------------------------------------------------------------------------------

fn main() -> Result<()> {
    // Subcommand modes (L-2 bprime-diff, L-4 emit/run/diff/instantiate) — everything else
    // falls through to the A-5 phases unchanged.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("bprime-diff") => {
            let [_, lib_path] = args.as_slice() else {
                bail!("usage: otter-aot-e2e bprime-diff <lib.wasm>");
            };
            return bprime_diff(lib_path);
        }
        Some("emit-aot") => {
            let [_, dir] = args.as_slice() else {
                bail!("usage: otter-aot-e2e emit-aot <dir>");
            };
            return emit_aot(dir);
        }
        Some("run-linked") => {
            let [_, shape, path] = args.as_slice() else {
                bail!("usage: otter-aot-e2e run-linked calc|deep|calc-shared <linked.wasm>");
            };
            return run_linked(shape, path);
        }
        Some("linked-diff") => {
            let [_, path] = args.as_slice() else {
                bail!("usage: otter-aot-e2e linked-diff <linked.wasm>");
            };
            return linked_diff(path);
        }
        Some("assert-instantiate") => {
            let [_, path] = args.as_slice() else {
                bail!("usage: otter-aot-e2e assert-instantiate <module.wasm>");
            };
            return assert_instantiate(path);
        }
        Some("run-component") => {
            let [_, shape, path] = args.as_slice() else {
                bail!("usage: otter-aot-e2e run-component calc|deep <component.wasm>");
            };
            return run_component(shape, path);
        }
        Some("a64-diff") => {
            let [_] = args.as_slice() else {
                bail!("usage: otter-aot-e2e a64-diff (no arguments — artifacts are emitted live)");
            };
            return a64_diff();
        }
        Some(other) => bail!(
            "unknown mode `{other}` (modes: <none> = A-5 phases, bprime-diff, emit-aot, \
             run-linked, linked-diff, assert-instantiate, run-component, a64-diff)"
        ),
        None => {}
    }

    let isa = isa();

    // ---- phase 1: entry program, non-shared, oracle-identical -----------------------------------
    let calc = calc_module();
    let expected = run_module(&isa, &calc, &[]);
    let (artifact, order) = compile_logged(&isa, &calc, &AotOpts::new(SCRATCH_BASE))?;
    let man = &artifact.manifest;
    if man.entry_slot != 0
        || man.nfuncs != 2
        || man.host_slots.len() != 2
        || man.table_size != 2 + 3 + 2
        || man.shared_memory
    {
        bail!("calc manifest shape unexpected: {man:?}");
    }
    let (mut store, instance) = instantiate(&artifact, &order, false)?;
    let got = call_entry(&mut store, &instance, FUEL, man.memory64)?;
    if got != expected {
        bail!("phase 1: entry result {got:#x} != oracle {expected:#x}");
    }
    println!(
        "AOT48-E2E-1-ENTRY-OK value={got:#x} emitter_version={}",
        man.emitter_version
    );

    // ---- phase 2: same program, shared-memory import shape, real SharedMemory -------------------
    let (artifact_sh, order_sh) =
        compile_logged(&isa, &calc, &AotOpts::new(SCRATCH_BASE).with_shared_memory(true))?;
    if !artifact_sh.manifest.shared_memory {
        bail!("shared_memory flag did not reach the manifest");
    }
    let (mut store, instance) = instantiate(&artifact_sh, &order_sh, false)?;
    let got = call_entry(&mut store, &instance, FUEL, artifact_sh.manifest.memory64)?;
    if got != expected {
        bail!("phase 2: shared-link entry result {got:#x} != oracle {expected:#x}");
    }
    println!("AOT48-E2E-2-SHARED-LINK-OK value={got:#x}");

    // ---- phase 3: 2M-deep bounded tail recursion, 64 KiB wasm stack (48-train M2 re-run) --------
    // Shallow-depth oracle cross-check first (the depth runs assert parity only, like aot.rs).
    let shallow = deep_module(20);
    let (a20, o20) = compile_logged(&isa, &shallow, &AotOpts::new(SCRATCH_BASE))?;
    let (mut store, instance) = instantiate(&a20, &o20, true)?;
    let got = call_entry(&mut store, &instance, FUEL, a20.manifest.memory64)?;
    let want = run_module(&isa, &shallow, &[]);
    if got != want {
        bail!("phase 3 sanity: even(20) = {got} != oracle {want}");
    }
    for (depth, parity) in [(DEPTH, 1), (DEPTH + 1, 0)] {
        let m = deep_module(depth);
        let (a, o) = compile_logged(&isa, &m, &AotOpts::new(SCRATCH_BASE))?;
        let (mut store, instance) = instantiate(&a, &o, true)?;
        let got = call_entry(&mut store, &instance, DEEP_FUEL, a.manifest.memory64)?;
        if got != parity {
            bail!("phase 3: even({depth}) = {got}, want {parity}");
        }
    }
    println!("AOT48-E2E-3-TAILBOUND-OK depth={DEPTH} stack={TINY_WASM_STACK}");

    // ---- phase 4: the §2.7 combination — shared import × declared table × return_call_indirect --
    let m = deep_module(DEPTH);
    let (a, o) = compile_logged(&isa, &m, &AotOpts::new(SCRATCH_BASE).with_shared_memory(true))?;
    let (mut store, instance) = instantiate(&a, &o, true)?;
    let got = call_entry(&mut store, &instance, DEEP_FUEL, a.manifest.memory64)?;
    if got != 1 {
        bail!("phase 4: shared even({DEPTH}) = {got}, want 1");
    }
    println!("AOT48-E2E-4-SHARED-TAILBOUND-OK depth={DEPTH} stack={TINY_WASM_STACK}");

    // ---- phase 5: negative control — ordinary Call must overflow the same 64 KiB stack ----------
    let m = nontail_module(DEPTH);
    // Same shared deployment shape as phase 4, so the ONLY axis that moved vs phase 4 is
    // tail-vs-non-tail.
    let (a, o) = compile_logged(&isa, &m, &AotOpts::new(SCRATCH_BASE).with_shared_memory(true))?;
    let (mut store, instance) = instantiate(&a, &o, true)?;
    match call_entry(&mut store, &instance, DEEP_FUEL, a.manifest.memory64) {
        Ok(v) => bail!("phase 5: non-tail 2M-deep recursion returned {v} instead of overflowing"),
        Err(e) => match e.downcast_ref::<Trap>() {
            Some(Trap::StackOverflow) => {}
            other => bail!("phase 5: expected Trap::StackOverflow, got {other:?} ({e})"),
        },
    }
    println!("AOT48-E2E-5-NEGCTRL-OK");

    println!("AOT48-E2E-ALL-OK");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O-19 C3 (design §4 단일-Config 규율, tech-S1): the `memory64` parameter on the ONE
    /// `reference_config` builder must leave every existing lane's Config UNCHANGED — `None`
    /// must compare Debug-identical to the pre-O-19 builder, reconstructed VERBATIM inside
    /// this test (a test-local twin is the O-17 no-drift pattern; the tech-S1 rule forbids a
    /// PRODUCTION fork, not a test witness). Debug equality is sound here: `Config`'s Debug
    /// prints the RESOLVED feature set field-by-field, and this binary's configs set no
    /// compiler-config map entries (the only nondeterministically-ordered Debug fields).
    #[test]
    fn reference_config_memory64_param_no_drift() {
        for tiny in [false, true] {
            let mut old = Config::new();
            old.wasm_exceptions(true);
            old.shared_memory(true);
            old.consume_fuel(true);
            if tiny {
                old.max_wasm_stack(TINY_WASM_STACK);
            }
            assert_eq!(
                format!("{:?}", reference_config(tiny, None)),
                format!("{old:?}"),
                "reference_config(_, None) must be the pre-O-19 Config byte-for-byte \
                 (tiny_stack={tiny})"
            );
        }
        // The parameter really reaches the Config (the feature bit is visible in Debug) …
        let on = format!("{:?}", reference_config(false, Some(true)));
        let off = format!("{:?}", reference_config(false, Some(false)));
        assert!(
            on.contains("wasm_memory64: true"),
            "explicit opt-in did not reach the Config"
        );
        assert!(
            off.contains("wasm_memory64: false"),
            "explicit opt-out did not reach the Config"
        );
        // … and the S2-M1-6 measured default-ON still holds: untouched == explicit-true at the
        // feature level (the explicit opt-in is a defense, not a semantic change today). If
        // wasmtime ever REVERSES the default (§8-1 risk), this assert goes red as the designed
        // tripwire — the A64 lane itself stays green through its explicit opt-in.
        assert_eq!(
            format!("{:?}", reference_config(false, None)),
            on,
            "wasmtime 48 memory64 default flipped (§8-1) — re-record the measured default; \
             the explicit opt-in already protects the A64 lane"
        );
    }
}
