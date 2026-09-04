//! O-19 C0 — **cross-commit golden-vector pin** for BOTH public emission surfaces (design
//! `otter-o19-wasm64-design.md` §2.3 v2 / §6-1 / §7 C0·C1).
//!
//! Why this exists (§2.3 v2, panel tech-M2): the O-17-style same-build pin
//! (`default_opts_are_byte_identical_to_the_legacy_entry_point`) compares two entry points of
//! ONE build, so it is blind to a **common-mode** byte change — exactly the drift class the
//! O-19 C1 addrtype plumbing (u64 widening + `Types` interning injection) could introduce; the
//! AOT surface has no legacy entry point at all, so its same-build "pin" would be a
//! self-comparison (vacuous). The only real witness that "parameter plumbing cannot touch the
//! emission" is a **cross-commit fixture**: the byte products below were captured at the
//! pre-O-19 (pre-addrtype) HEAD and checked in under `tests/golden/`; this test re-emits the
//! same corpus and asserts byte identity + a recorded FNV-1a-64 fingerprint per vector.
//!
//! * The byte-equality against the checked-in `.wasm` file is the load-bearing gate.
//! * The `hashes.txt` fingerprint is a redundancy channel: it makes drift visible in review
//!   diffs (a one-line hash change instead of an opaque binary diff) and catches a corrupted /
//!   swapped golden file independently of the emitter.
//!
//! **Updating the vectors** is a deliberate act (that visibility is the point — §2.3 v2: an
//! intentional emission change must surface as an explicit fixture diff): run
//! `OTTER_GOLDEN_REGEN=1 cargo test -p otter-wasmemit --test golden` and commit the changed
//! files under `tests/golden/` together with the emitter change that motivated them.
//!
//! Corpus scope (representative on purpose, not exhaustive): both surfaces
//! ([`compile_dispatch_fn_with`] Dispatch-mode + [`compile_module`] Table-mode), both memory
//! import shapes (non-shared + shared), straight-line and dispatcher-loop bodies, the full
//! helper/host `call_indirect` type set, the marshaling scratch bake, and the Table-mode
//! guard + direct `call_indirect`/`return_call_indirect` windows — i.e. every emission site the
//! O-19 §2.2 change inventory names on its A32 side.

mod common;

use std::path::PathBuf;

use common::{
    CALL, COPY, DEC_JNZ, GUARD, HALT, HOST1, HOST2, HOSTCTX1, HOSTCTX2, IMM_BIG, IMM_F64, JUMP, JUMP_IF,
    MOV_IMM, MULTI2, RET, TAILCALL, bin_opcode, conv_opcode, neg_opcode,
};
use otter::{FuncDef, Inst, Module as IrModule};
use otter_wasmemit::{
    AotOpts, EmitOpts, HelperIdx, HostImportName, compile_dispatch_fn_with, compile_module,
};

/// Deterministic full-width fixture words (aot.rs suite conventions).
const FILL: otter::Value = 0x5EED_F00D_5EED_F00D_u64 as otter::Value;
const HOST_CTX: otter::Value = 0x7E57_C0DE_7E57_C0DE_u64 as otter::Value;

/// Fixed helper table slots for the JIT-surface vectors (arbitrary but pinned — they are baked
/// into the golden bytes, so they must never drift).
fn helpers() -> HelperIdx {
    HelperIdx {
        dispatch: 1,
        tail: 2,
        binop: 3,
        convert: 4,
        unop: 5,
    }
}

/// JIT-surface host resolver: the four well-known test host fns to fixed, distinct table slots
/// (identity-keyed on the fn pointers — pointer VALUES never reach the bytes, only these slots).
fn resolve_jit(p: usize) -> Option<u32> {
    if p == common::h_add7 as otter::HostFn1 as usize {
        Some(40)
    } else if p == common::h_mix as otter::HostFn2 as usize {
        Some(41)
    } else if p == common::hc_ret_ctx as otter::HostFnCtx1 as usize {
        Some(42)
    } else if p == common::hc_mix as otter::HostFnCtx2 as usize {
        Some(43)
    } else {
        None
    }
}

/// AOT-surface host resolver: the same four fns to their import names (aot.rs `host_name`).
fn resolve_aot(p: usize) -> Option<HostImportName> {
    let name = if p == common::h_add7 as otter::HostFn1 as usize {
        "h_add7"
    } else if p == common::h_mix as otter::HostFn2 as usize {
        "h_mix"
    } else if p == common::hc_ret_ctx as otter::HostFnCtx1 as usize {
        "hc_ret_ctx"
    } else if p == common::hc_mix as otter::HostFnCtx2 as usize {
        "hc_mix"
    } else {
        return None;
    };
    Some(HostImportName(name.to_string()))
}

// --- the corpus ------------------------------------------------------------------------------------

/// Straight-line function exercising every W-1a `call_indirect` type (binop/convert/unop +
/// HOST1/HOST2/HOSTCTX1/HOSTCTX2), the const prologue, and every literal `Expr` shape.
fn rep_fn() -> FuncDef {
    FuncDef {
        nparams: 2,
        consts: vec![0x1122_3344_5566_7788, -9],
        nlocals: 6,
        code: vec![
            Inst::new(bin_opcode(8, 0), 4, 0, 1),   // f16 add -> binop helper
            Inst::new(neg_opcode(3), 4, 4, 0),      // i64 neg -> unop helper
            Inst::new(conv_opcode(3, 10), 5, 4, 0), // i64->f64 -> convert helper
            Inst::new(HOST1, 5, 5, 0),
            Inst::new(HOST2, 4, 5, 2),
            Inst::new(HOSTCTX1, 5, 4, 0),
            Inst::new(HOSTCTX2, 4, 5, 3),
            Inst::new(IMM_F64, 0, 0, 0),
            Inst::new(IMM_BIG, 1, 0, 0),
            Inst::new(GUARD, 0, 1, 0),
            Inst::new(COPY, 2, 4, 0),
            Inst::new(MOV_IMM, 3, -7, 0),
            Inst::new(HALT, 2, 0, 0),
        ],
    }
}

/// Dispatcher-loop function: forward + backward jumps, both `JumpIf` edges, a mid-loop helper
/// and host call — the §3 `loop`/`br_table` skeleton.
fn cfg_fn() -> FuncDef {
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(MOV_IMM, 1, 6, 0),
            Inst::new(MULTI2, 2, 0, 1),
            Inst::new(JUMP_IF, 2, 4, 0),
            Inst::new(JUMP, 6, 0, 0),
            Inst::new(HOST1, 3, 2, 0),
            Inst::new(bin_opcode(3, 0), 2, 2, 3), // i64 add
            Inst::new(DEC_JNZ, 1, 1, 0),
            Inst::new(HALT, 2, 0, 0),
        ],
    }
}

/// Call-carrying straight-line function: one `Call` (argc 2) then a `TailCall` (argc 1) — the
/// Dispatch-mode marshal + `module_dispatch`/`tail_signal` call-site shapes with the scratch
/// address baked.
fn calls_fn() -> FuncDef {
    FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(MOV_IMM, 0, 1, 0),
            Inst::new(MOV_IMM, 1, 41, 0),
            Inst::new(MOV_IMM, 2, 42, 0),
            Inst::new(CALL, 3, 0, 2),
            Inst::new(TAILCALL, 0, 1, 0),
        ],
    }
}

/// The AOT-surface module: entry `Call` chain into a `TailCall`, a dispatcher-loop callee, and a
/// host-type-covering leaf — Table-mode guard windows, declared table + element, all four host
/// import types, scratch self-assignment over multiple functions.
fn mixed_module() -> IrModule {
    let f0 = FuncDef {
        nparams: 0,
        consts: vec![0x7777_0000_0000_0001],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 1, 1, 0),
            Inst::new(MOV_IMM, 2, 9, 0),
            Inst::new(CALL, 3, 1, 1),
            Inst::new(bin_opcode(3, 0), 4, 3, 0), // i64 add (r0 = const slot)
            Inst::new(HOST1, 5, 4, 0),
            Inst::new(HALT, 5, 0, 0),
        ],
    };
    let f1 = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(MOV_IMM, 1, 2, 0),
            Inst::new(COPY, 2, 0, 0),
            Inst::new(TAILCALL, 1, 1, 0),
        ],
    };
    let f2 = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(HOSTCTX2, 1, 0, 0),
            Inst::new(DEC_JNZ, 0, 0, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    };
    let f3 = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(HOST2, 2, 0, 1),
            Inst::new(HOSTCTX1, 3, 2, 0),
            Inst::new(RET, 3, 0, 0),
        ],
    };
    IrModule {
        funcs: vec![f0, f1, f2, f3],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    }
}

/// Emit one named vector. Every input below is a compile-time constant of this file — the
/// vectors are deterministic by construction (host fn POINTERS never reach the bytes: the JIT
/// resolver maps them to pinned slots, the AOT resolver to pinned import names).
fn emit(name: &str) -> Vec<u8> {
    let isa = common::isa();
    match name {
        "dispatch_representative" => compile_dispatch_fn_with(
            &isa,
            0,
            &rep_fn(),
            FILL,
            HOST_CTX,
            helpers(),
            &resolve_jit,
            0x1000,
            EmitOpts::new(),
        )
        .expect("golden corpus must compile"),
        "dispatch_representative_shared" => compile_dispatch_fn_with(
            &isa,
            0,
            &rep_fn(),
            FILL,
            HOST_CTX,
            helpers(),
            &resolve_jit,
            0x1000,
            EmitOpts::new().with_shared_memory(true),
        )
        .expect("golden corpus must compile"),
        "dispatch_dispatcher_loop" => compile_dispatch_fn_with(
            &isa,
            1,
            &cfg_fn(),
            FILL,
            HOST_CTX,
            helpers(),
            &resolve_jit,
            0x1000,
            EmitOpts::new(),
        )
        .expect("golden corpus must compile"),
        "dispatch_calls" => compile_dispatch_fn_with(
            &isa,
            2,
            &calls_fn(),
            FILL,
            HOST_CTX,
            helpers(),
            &resolve_jit,
            0x2000,
            EmitOpts::new(),
        )
        .expect("golden corpus must compile"),
        "module_mixed" => {
            compile_module(&isa, &mixed_module(), &resolve_aot, &AotOpts::new(0x1_0000))
                .expect("golden corpus must compile")
                .bytes
        }
        "module_mixed_shared" => {
            compile_module(
                &isa,
                &mixed_module(),
                &resolve_aot,
                &AotOpts::new(0x1_0000).with_shared_memory(true),
            )
            .expect("golden corpus must compile")
            .bytes
        }
        other => panic!("unknown golden vector name {other:?}"),
    }
}

/// Names of every golden vector, in `hashes.txt` order.
const VECTORS: [&str; 6] = [
    "dispatch_representative",
    "dispatch_representative_shared",
    "dispatch_dispatcher_loop",
    "dispatch_calls",
    "module_mixed",
    "module_mixed_shared",
];

/// FNV-1a 64 (the redundancy fingerprint — see the module doc; byte equality is the real gate).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The C0/§6-1 gate: every re-emitted vector is byte-identical to its checked-in pre-O-19
/// fixture, and matches the recorded FNV-1a-64 fingerprint + length. With
/// `OTTER_GOLDEN_REGEN=1` the fixtures are (re)written first — an explicit, review-visible act.
#[test]
fn golden_vectors_pin_default_emission_across_commits() {
    let dir = golden_dir();
    let regen = std::env::var_os("OTTER_GOLDEN_REGEN").is_some();
    if regen {
        std::fs::create_dir_all(&dir).expect("create tests/golden");
        let mut hashes = String::new();
        for name in VECTORS {
            let bytes = emit(name);
            std::fs::write(dir.join(format!("{name}.wasm")), &bytes).expect("write golden vector");
            hashes.push_str(&format!("{name} {:016x} {}\n", fnv1a64(&bytes), bytes.len()));
        }
        std::fs::write(dir.join("hashes.txt"), hashes).expect("write hashes.txt");
        eprintln!(
            "golden: REGENERATED {} vectors at {}",
            VECTORS.len(),
            dir.display()
        );
    }

    let hashes = std::fs::read_to_string(dir.join("hashes.txt"))
        .expect("tests/golden/hashes.txt must be checked in (regen: OTTER_GOLDEN_REGEN=1)");
    let mut recorded = std::collections::BTreeMap::new();
    for line in hashes.lines() {
        let mut it = line.split_whitespace();
        let name = it.next().expect("hashes.txt: name column");
        let fnv = u64::from_str_radix(it.next().expect("hashes.txt: fnv column"), 16)
            .expect("hashes.txt: fnv column must be hex");
        let len: usize = it
            .next()
            .expect("hashes.txt: len column")
            .parse()
            .expect("hashes.txt: len column must be an integer");
        recorded.insert(name.to_string(), (fnv, len));
    }
    assert_eq!(
        recorded.len(),
        VECTORS.len(),
        "hashes.txt must record exactly the {} corpus vectors",
        VECTORS.len()
    );

    for name in VECTORS {
        let golden = std::fs::read(dir.join(format!("{name}.wasm")))
            .unwrap_or_else(|e| panic!("golden vector {name}.wasm must be checked in: {e}"));
        let (fnv, len) = recorded[name];
        assert_eq!(
            golden.len(),
            len,
            "{name}: checked-in fixture length drifted from hashes.txt"
        );
        assert_eq!(
            fnv1a64(&golden),
            fnv,
            "{name}: checked-in fixture no longer matches its recorded fingerprint (corrupted or \
             swapped golden file)"
        );
        let emitted = emit(name);
        assert_eq!(
            emitted, golden,
            "{name}: emission drifted from the pre-O-19 golden vector — if this change is \
             INTENTIONAL, regenerate with OTTER_GOLDEN_REGEN=1 and commit the fixture diff \
             alongside the emitter change (design §2.3 v2: the visible diff is the point)"
        );
    }
}
