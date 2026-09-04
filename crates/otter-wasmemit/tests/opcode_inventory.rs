//! H4 — emitted-opcode inventory gate (2-stage + mode separation), per the H-package design:
//!
//! * (i) per-artifact: every function body's opcode set is a SUBSET of the 22-entry whitelist —
//!   any new instruction the emitter starts producing turns this red until the whitelist (a
//!   reviewed, security-relevant surface) is consciously extended;
//! * (ii) corpus-wide: the UNION over all artifacts EQUALS the whitelist (set equality) — a
//!   coverage collapse (a corpus edit that stops exercising an emission path) or a dead
//!   whitelist entry turns red immediately, so (i) can never rot into vacuity;
//! * (iii) mode separation: Dispatch-mode (JIT-surface) artifacts must NOT contain
//!   `ReturnCallIndirect`/`I64GeU`/`I32WrapI64` — the A(Table)-mode-only call lowering must
//!   never leak into the JIT surface (tail-call-unsupporting hosts run the JIT surface).
//!
//! Corpus requirements (measured emission conditions, design H4): BOTH CallModes × a jump function
//! (⇒ Block/Loop/Br/BrTable/I64Eqz/Unreachable) × `nparams >= 1` (⇒ I64GtS/I64Load in the argc-
//! guarded param prologue) × a Dispatch-mode TailCall (⇒ Drop). The parser is `common::body_ops`'s
//! walker (promoted from aot.rs — no duplicate body-walker).

mod common;

use std::collections::BTreeSet;

use common::{CALL, COPY, HALT, JUMP, JUMP_IF, MOV_IMM, RET, TAILCALL, all_body_ops, func, isa};
use otter::{FuncDef, Inst, Module as IrModule, Value};
use otter_wasmemit::{AotOpts, HelperIdx, HostImportName, compile_dispatch_fn, compile_module};

const FILL: Value = -99;
const HOST_CTX: Value = 0x7E57;
/// Same fixture value as aot.rs — clear of the mock staging area; never executed here.
const SCRATCH_BASE: u64 = 0x1_0000;

/// Arbitrary-but-valid helper table indices for the Dispatch (JIT) surface — baked as immediates,
/// never called (this suite parses bytes; nothing executes).
fn helper_idx() -> HelperIdx {
    HelperIdx {
        dispatch: 0x101,
        tail: 0x102,
        binop: 0x103,
        convert: 0x104,
        unop: 0x105,
    }
}

// --- corpus (all nparams >= 1 — the I64GtS/I64Load emission condition) ----------------------------

/// Jump shapes: forward + BACKWARD conditional jumps and an unconditional jump — forces the
/// dispatcher-loop lowering (Loop/Block/BrTable/Br + I64Eqz cond tests + the Unreachable default).
fn jump_fn() -> FuncDef {
    func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 1, 3, 0),
            Inst::new(JUMP_IF, 0, 3, 0), // forward
            Inst::new(JUMP, 4, 0, 0),    // forward, unconditional
            Inst::new(JUMP_IF, 1, 1, 0), // BACKWARD (loop shape)
            Inst::new(HALT, 1, 0, 0),
        ],
    )
}

/// An ordinary `Call` site with argc >= 1 (scratch marshaling stores + the call-site shape).
fn call_fn() -> FuncDef {
    func(
        1,
        4,
        vec![
            Inst::new(MOV_IMM, 1, 1, 0), // callee word (static, in range of the 3-func module)
            Inst::new(COPY, 2, 0, 0),    // arg0 = param
            Inst::new(CALL, 3, 1, 1),    // dest=3, base=1, argc=1
            Inst::new(RET, 3, 0, 0),
        ],
    )
}

/// A `TailCall` site with argc >= 1 — in Dispatch mode this is the `Drop` emission condition (the
/// tail helper's discarded return); in Table mode, the `return_call_indirect` condition.
fn tail_fn() -> FuncDef {
    func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 1, 2, 0),  // callee word
            Inst::new(COPY, 2, 0, 0),     // arg0
            Inst::new(TAILCALL, 1, 1, 0), // base=1, argc=1
        ],
    )
}

// --- artifact building -----------------------------------------------------------------------------

/// One Dispatch-mode (JIT-surface) artifact per corpus function.
fn dispatch_artifacts() -> Vec<(&'static str, Vec<u8>)> {
    let isa = isa();
    let resolve = |_: usize| -> Option<u32> { None }; // corpus is host-fn-free — refuse loudly otherwise
    [
        ("dispatch/jump", jump_fn()),
        ("dispatch/call", call_fn()),
        ("dispatch/tail", tail_fn()),
    ]
    .into_iter()
    .map(|(name, f)| {
        let bytes = compile_dispatch_fn(
            &isa,
            0,
            &f,
            FILL,
            HOST_CTX,
            helper_idx(),
            &resolve,
            SCRATCH_BASE as u32,
        )
        .expect("corpus function must compile on the JIT surface");
        (name, bytes)
    })
    .collect()
}

/// The Table-mode (AOT) artifact: ONE module containing all three corpus shapes.
fn table_artifact() -> Vec<u8> {
    let m = IrModule {
        funcs: vec![jump_fn(), call_fn(), tail_fn()],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let resolve = |_: usize| -> Option<HostImportName> { None };
    compile_module(&isa(), &m, &resolve, &AotOpts::new(SCRATCH_BASE))
        .expect("corpus module must compile in Table mode")
        .bytes
}

/// The opcode VARIANT (wasmparser `Operator` variant name — the leading identifier of its `{:?}`
/// form) set of every function body in `bytes`, unioned.
fn opcode_set(bytes: &[u8]) -> BTreeSet<String> {
    all_body_ops(bytes)
        .iter()
        .flatten()
        .map(|op| {
            op.split([' ', '{'])
                .next()
                .expect("nonempty op string")
                .trim()
                .to_string()
        })
        .collect()
}

/// The 22-entry whitelist — the COMPLETE instruction surface `otter-wasmemit` may emit for the
/// call/control/marshal corpus, both CallModes (design H4). Extending the emitter's vocabulary is
/// a conscious, reviewed act: add the opcode here in the same change, or gate (i) goes red.
const WHITELIST: [&str; 22] = [
    "Block",
    "Br",
    "BrTable",
    "CallIndirect",
    "Drop",
    "Else",
    "End",
    "I32Const",
    "I32WrapI64",
    "I64Const",
    "I64Eqz",
    "I64GeU",
    "I64GtS",
    "I64Load",
    "I64Store",
    "If",
    "LocalGet",
    "LocalSet",
    "Loop",
    "Return",
    "ReturnCallIndirect",
    "Unreachable",
];

/// The three A(Table)-mode-only opcodes that must never appear on the JIT (Dispatch) surface —
/// gate (iii)'s fence (`return_call_indirect` needs a tail-call-supporting host; the direct
/// guard+wrap shape exists only in the A-3 Table lowering).
const TABLE_MODE_ONLY: [&str; 3] = ["ReturnCallIndirect", "I64GeU", "I32WrapI64"];

#[test]
fn per_artifact_subset_union_equality_and_mode_separation() {
    let whitelist: BTreeSet<String> = WHITELIST.iter().map(|s| s.to_string()).collect();
    assert_eq!(whitelist.len(), 22, "whitelist entries must be distinct");

    let mut union: BTreeSet<String> = BTreeSet::new();
    let mut artifacts = dispatch_artifacts();
    artifacts.push(("table/all", table_artifact()));

    for (name, bytes) in &artifacts {
        let set = opcode_set(bytes);
        // (i) per-artifact ⊆ whitelist.
        let excess: Vec<&String> = set.difference(&whitelist).collect();
        assert!(
            excess.is_empty(),
            "{name}: emitted opcodes outside the whitelist: {excess:?} — a new emitter instruction \
             must be consciously added to WHITELIST in the same change"
        );
        // (iii) mode separation: Dispatch (JIT-surface) artifacts carry no Table-mode-only opcode.
        if name.starts_with("dispatch/") {
            for banned in TABLE_MODE_ONLY {
                assert!(
                    !set.contains(banned),
                    "{name}: A(Table)-mode-only opcode `{banned}` leaked into the JIT surface"
                );
            }
        }
        union.extend(set);
    }

    // (ii) union over the whole corpus == whitelist (set EQUALITY — coverage collapse or a dead
    // whitelist entry both go red).
    let missing: Vec<&String> = whitelist.difference(&union).collect();
    let extra: Vec<&String> = union.difference(&whitelist).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "corpus union != whitelist: never-emitted (dead) entries {missing:?}, unlisted emissions {extra:?}"
    );
}

/// Non-vacuity of the corpus-condition table (design H4): each corpus requirement really does emit
/// the opcodes it exists to cover — so a corpus edit that silently stops exercising a condition is
/// caught HERE by name, not only as a puzzling union mismatch.
#[test]
fn corpus_conditions_emit_their_designated_opcodes() {
    let dispatch = dispatch_artifacts();
    let by_name = |n: &str| -> BTreeSet<String> {
        let (_, bytes) = dispatch.iter().find(|(name, _)| *name == n).expect("corpus name");
        opcode_set(bytes)
    };

    // Jump function ⇒ the dispatcher-loop control skeleton.
    let jump = by_name("dispatch/jump");
    for op in ["Block", "Loop", "Br", "BrTable", "I64Eqz", "Unreachable"] {
        assert!(jump.contains(op), "jump corpus must emit {op}");
    }
    // nparams >= 1 ⇒ the argc-guarded param prologue, in EVERY corpus function.
    for (name, bytes) in &dispatch {
        let set = opcode_set(bytes);
        for op in ["I64GtS", "I64Load"] {
            assert!(set.contains(op), "{name}: nparams >= 1 corpus must emit {op}");
        }
    }
    // Dispatch TailCall ⇒ Drop (the tail helper's discarded return value).
    assert!(
        by_name("dispatch/tail").contains("Drop"),
        "Dispatch TailCall corpus must emit Drop"
    );
    // Table mode ⇒ the three A-mode-only opcodes all present (the other side of gate (iii)).
    let table = opcode_set(&table_artifact());
    for op in TABLE_MODE_ONLY {
        assert!(table.contains(op), "Table-mode corpus must emit {op}");
    }
}
