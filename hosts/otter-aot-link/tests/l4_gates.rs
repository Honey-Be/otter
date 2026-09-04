//! L-4 gate tests (design §7 L-4 row: `check --linked` + `shrink-exports`, both moved from
//! L-1 by panel G-SF-5 — unit level; the real linked artifacts are `just wasm-aot-link-e2e`'s):
//! - `check --linked`: every §5.3 semantic item with a positive AND a negative — single owned
//!   memory, otter_entry/otter_table exports, declared-table shape, active-element slot shape,
//!   machine EH census (operator + tag section), and the §6.3 profile-split start check
//!   (absent / shared init-only whitelist / non-shared start red / call-in-start red);
//! - `shrink-exports`: keep-list reduction + export-mode invariance + Validator, with the
//!   fail-closed refusals (empty keep, unmatched keep, no export section) and the composition
//!   check (post-shrink artifact still passes `check --linked`);
//! - L-6 hardening (the L-4 review's carried-over prescriptions): the memory-sharedness
//!   cross-predicate (artifact declaration == manifest.shared_memory — green shared shape,
//!   both mismatch directions, and the forged-manifest start-smuggle composition) plus the
//!   four sub-branch negatives (imported table / multiple element segments / imported tag /
//!   start on an imported func).

use otter_aot_link::LinkError;
use otter_aot_link::linked::{StartDisposition, check_linked};
use otter_aot_link::schema::{ManifestSidecar, ScratchRegion};
use otter_aot_link::shrink::shrink_exports;

use std::collections::BTreeSet;

use wasm_encoder::{
    CodeSection, ConstExpr, ElementSection, Elements, EntityType, ExportKind, ExportSection, Function,
    FunctionSection, ImportSection, MemorySection, MemoryType, Module, RefType, StartSection, TableSection,
    TableType, TagKind, TagSection, TagType, TypeSection, ValType,
};

// ---------------------------------------------------------------------------------------------
// fixtures — a "linked-shaped" module (memory OWNED, table + element, export union)
// ---------------------------------------------------------------------------------------------

/// Start-section flavor of a fixture.
enum Start {
    None,
    /// Whitelisted init-only body (`i32.const 0; drop; end`).
    InitOnly,
    /// Body containing a `call 0` — the load-bearing whitelist exclusion.
    WithCall,
}

struct Fx {
    table_size: u64,
    element_items: u64,
    element_offset: i32,
    import_memory: bool,
    memories: usize,
    /// Defined memories are SHARED (+atomics linked shape — shared requires a max).
    shared_mem: bool,
    /// Defined memories are memory64 (O-19 C5 cross-predicate material — no legal product
    /// reaches this shape today, the fixture exists for the forged/mispaired negatives).
    mem64: bool,
    /// Imports a table (the declared table must be a DEFINITION — sub-branch negative, L-6).
    import_table: bool,
    /// Imports a tag (EH infrastructure census, imported half — sub-branch negative, L-6).
    import_tag: bool,
    /// Imports a func of type t0 — shifts defined func indices by 1, so a `StartSection`
    /// at index 0 points at the IMPORTED func (the not-inspectable refusal, L-6).
    import_func: bool,
    /// Number of (identical) active element segments — >1 exercises the exactly-1 arm (L-6).
    element_segments: usize,
    start: Start,
    /// Adds `throw_ref` into the entry body (EH census material — parse-level, not validated).
    eh_op: bool,
    /// Adds a tag section (EH infrastructure).
    tag: bool,
    exports: Vec<(&'static str, ExportKind, u32)>,
}

impl Default for Fx {
    fn default() -> Self {
        Fx {
            table_size: 4,
            element_items: 4,
            element_offset: 0,
            import_memory: false,
            memories: 1,
            shared_mem: false,
            mem64: false,
            import_table: false,
            import_tag: false,
            import_func: false,
            element_segments: 1,
            start: Start::None,
            eh_op: false,
            tag: false,
            // The export-union shape (b′ surface + product surface).
            exports: vec![
                ("memory", ExportKind::Memory, 0),
                ("otter_binop", ExportKind::Func, 0),
                ("otter_entry", ExportKind::Func, 1),
                ("otter_table", ExportKind::Table, 0),
            ],
        }
    }
}

fn build(fx: &Fx) -> Vec<u8> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([], []); // t0: start/helper shape
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I64], [ValType::I64]); // t1: entry
    module.section(&types);

    if fx.import_memory || fx.import_table || fx.import_tag || fx.import_func {
        let mut imports = ImportSection::new();
        if fx.import_func {
            imports.import("otter_lib", "init_stub", EntityType::Function(0));
        }
        if fx.import_memory {
            imports.import(
                "otter",
                "memory",
                EntityType::Memory(MemoryType {
                    minimum: 0,
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
        }
        if fx.import_table {
            imports.import(
                "otter_lib",
                "spare_table",
                EntityType::Table(TableType {
                    element_type: RefType::FUNCREF,
                    minimum: 1,
                    maximum: Some(1),
                    table64: false,
                    shared: false,
                }),
            );
        }
        if fx.import_tag {
            imports.import(
                "otter_lib",
                "exn_tag",
                EntityType::Tag(TagType {
                    kind: TagKind::Exception,
                    func_type_idx: 0,
                }),
            );
        }
        module.section(&imports);
    }

    let mut funcs = FunctionSection::new();
    funcs.function(0); // f0: helper/start-shaped
    funcs.function(1); // f1: entry-shaped
    module.section(&funcs);

    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        minimum: fx.table_size,
        maximum: Some(fx.table_size),
        table64: false,
        shared: false,
    });
    module.section(&tables);

    if !fx.import_memory {
        let mut memory = MemorySection::new();
        for _ in 0..fx.memories {
            memory.memory(MemoryType {
                minimum: 18,
                // Shared memories require a declared max (the +atomics linked shape).
                maximum: if fx.shared_mem { Some(65536) } else { None },
                memory64: fx.mem64,
                shared: fx.shared_mem,
                page_size_log2: None,
            });
        }
        module.section(&memory);
    }

    if fx.tag {
        let mut tags = TagSection::new();
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: 0,
        });
        module.section(&tags);
    }

    let mut exports = ExportSection::new();
    for (name, kind, index) in &fx.exports {
        exports.export(name, *kind, *index);
    }
    module.section(&exports);

    if !matches!(fx.start, Start::None) {
        module.section(&StartSection { function_index: 0 });
    }

    let mut elements = ElementSection::new();
    let offset = ConstExpr::i32_const(fx.element_offset);
    let items: Vec<u32> = vec![0; fx.element_items as usize];
    for _ in 0..fx.element_segments {
        elements.active(Some(0), &offset, Elements::Functions(items[..].into()));
    }
    module.section(&elements);

    let mut code = CodeSection::new();
    let mut f0 = Function::new([]);
    match fx.start {
        Start::WithCall => {
            f0.instructions().call(0).end();
        }
        _ => {
            f0.instructions().i32_const(0).drop().end();
        }
    }
    code.function(&f0);
    let mut f1 = Function::new([]);
    if fx.eh_op {
        f1.instructions().throw_ref().end();
    } else {
        f1.instructions().i64_const(7).end();
    }
    code.function(&f1);
    module.section(&code);

    module.finish()
}

/// Manifest fixture — `check --linked` consults `table_size` and `shared_memory` only.
fn manifest(table_size: u32, shared: bool) -> ManifestSidecar {
    ManifestSidecar {
        entry_slot: 0,
        nfuncs: 1,
        helper_slots: [1, 2, 3],
        host_slots: vec![],
        imports: vec![],
        scratch: vec![],
        scratch_region: ScratchRegion {
            base: 0x11_0000,
            end: 0x11_0008,
        },
        table_size,
        shared_memory: shared,
        memory64: false,
        emitter_version: "0.0.0".to_string(),
    }
}

fn linked_violations(fx: &Fx, man: &ManifestSidecar) -> Vec<String> {
    match check_linked(&build(fx), man) {
        Err(LinkError::Linked(v)) => v,
        other => panic!("expected Linked violations, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// check --linked — positives
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_green_nonshared_no_start() {
    let report = check_linked(&build(&Fx::default()), &manifest(4, false)).expect("green");
    assert_eq!(report.start, StartDisposition::Absent);
    assert_eq!(report.table_size, 4);
    assert_eq!(report.exports, 4);
}

#[test]
fn linked_green_shared_init_only_start() {
    // L-6 sharpening: the fixture memory is now genuinely SHARED — under the sharedness
    // cross-predicate, manifest.shared_memory=true against a non-shared memory would be red
    // (exactly the fail-open this fixture used to embody before the predicate existed).
    let fx = Fx {
        start: Start::InitOnly,
        shared_mem: true,
        ..Fx::default()
    };
    let report = check_linked(&build(&fx), &manifest(4, true)).expect("green");
    // i32.const + drop + end — all whitelisted, 0 call-class.
    assert_eq!(report.start, StartDisposition::InitOnly { ops: 3 });
    assert!(report.shared_memory);
}

// ---------------------------------------------------------------------------------------------
// check --linked — sharedness cross-predicate (L-6, the L-4 review's carried-over defect 2)
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_red_shared_manifest_on_nonshared_artifact() {
    // The closed fail-open: a forged/mispaired manifest claiming shared_memory=true against a
    // non-shared artifact used to silently select the permissive start profile. No start here —
    // the sharedness mismatch alone must be red.
    let v = linked_violations(&Fx::default(), &manifest(4, true));
    assert!(
        v.iter()
            .any(|m| m.contains("sharedness mismatch") && m.contains("non-shared memory")),
        "shared manifest × non-shared artifact must be red on its own: {v:?}"
    );
}

#[test]
fn linked_red_nonshared_manifest_on_shared_artifact() {
    let fx = Fx {
        shared_mem: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter()
            .any(|m| m.contains("sharedness mismatch") && m.contains("SHARED memory")),
        "non-shared manifest × shared artifact must be red: {v:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// check --linked — memory64 cross-predicate (O-19 C5, design §2.5: the sharedness predicate's
// addrtype twin — the bytes' memory64 bit is the ground truth, record==live at the linked layer)
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_red_memory64_artifact_on_false_or_old_manifest() {
    // A memory64 linked artifact against a `memory64: false` manifest — which is ALSO what an
    // OLD (pre-C5) sidecar parses to via #[serde(default)] — must be red on this predicate
    // alone: the default stays fail-closed precisely because this refusal exists.
    let fx = Fx {
        mem64: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter()
            .any(|m| m.contains("memory64 mismatch") && m.contains("memory64 (i64)")),
        "memory64 artifact × false/old manifest must be red: {v:?}"
    );
}

#[test]
fn linked_red_memory64_manifest_on_wasm32_artifact() {
    // The forged-record direction: a manifest CLAIMING memory64 against a wasm32 artifact.
    let mut man = manifest(4, false);
    man.memory64 = true;
    let v = linked_violations(&Fx::default(), &man);
    assert!(
        v.iter()
            .any(|m| m.contains("memory64 mismatch") && m.contains("wasm32 (i32)")),
        "memory64 manifest × wasm32 artifact must be red: {v:?}"
    );
}

#[test]
fn linked_memory64_predicate_is_equality_and_reports_explicitly() {
    // The predicate's chosen posture (recorded, design §2.5): EQUALITY like the sharedness
    // twin, not an outright memory64 ban at this layer — `check --scratch`'s owned_memory seam
    // is the layer that refuses memory64 artifacts outright; --linked's job is record==live.
    // No legal product reaches true×true today (the A64×b′ link is externally blocked), so
    // this green documents the predicate shape, not a shipping artifact.
    let fx = Fx {
        mem64: true,
        ..Fx::default()
    };
    let mut man = manifest(4, false);
    man.memory64 = true;
    let report = check_linked(&build(&fx), &man).expect("true == true passes the predicate");
    assert!(
        report.memory64,
        "the report must carry the flag explicitly, never silently"
    );
    let report32 = check_linked(&build(&Fx::default()), &manifest(4, false)).expect("false == false");
    assert!(!report32.memory64);
}

#[test]
fn linked_red_shared_manifest_does_not_buy_a_start_on_nonshared_artifact() {
    // Composition of the two closed holes: manifest says shared (permissive start profile) but
    // the artifact is non-shared AND carries an init-shaped start. The sharedness predicate
    // must fire — the start alone would pass the (manifest-selected) whitelist.
    let fx = Fx {
        start: Start::InitOnly,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, true));
    assert!(
        v.iter().any(|m| m.contains("sharedness mismatch")),
        "the forged-manifest start smuggle must be caught by the sharedness predicate: {v:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// check --linked — start negatives (§6.3 profile split + whitelist)
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_red_start_on_nonshared() {
    let fx = Fx {
        start: Start::InitOnly,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("NON-shared")),
        "start on non-shared must name the §6.3 profile split: {v:?}"
    );
}

#[test]
fn linked_red_start_on_imported_func() {
    // Start pointing at an IMPORTED function: no body to audit — fail-closed refusal, never
    // an assumed-green (L-6 sub-branch negative; shared profile so the §6.3 split doesn't
    // pre-empt the branch under test).
    let fx = Fx {
        start: Start::InitOnly,
        import_func: true,
        shared_mem: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, true));
    assert!(
        v.iter()
            .any(|m| m.contains("IMPORTED function") && m.contains("fail-closed")),
        "start on an imported func must refuse loudly: {v:?}"
    );
}

#[test]
fn linked_red_start_with_call() {
    let fx = Fx {
        start: Start::WithCall,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, true));
    assert!(
        v.iter()
            .any(|m| m.contains("outside the init whitelist") && m.contains("Call")),
        "call-in-start must be a whitelist red naming the operator: {v:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// check --linked — memory / export / table / element negatives
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_red_imported_memory() {
    let fx = Fx {
        import_memory: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("IMPORTS") && m.contains("memor")),
        "imported memory must be red (library-owned shape): {v:?}"
    );
}

#[test]
fn linked_red_two_memories() {
    let fx = Fx {
        memories: 2,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("defines 2 memories")),
        "two defined memories must be red: {v:?}"
    );
}

#[test]
fn linked_red_missing_entry_export() {
    let fx = Fx {
        exports: vec![
            ("memory", ExportKind::Memory, 0),
            ("otter_table", ExportKind::Table, 0),
        ],
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(v.iter().any(|m| m.contains("otter_entry")), "{v:?}");
}

#[test]
fn linked_red_missing_table_export() {
    let fx = Fx {
        exports: vec![
            ("memory", ExportKind::Memory, 0),
            ("otter_entry", ExportKind::Func, 1),
        ],
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(v.iter().any(|m| m.contains("otter_table")), "{v:?}");
}

#[test]
fn linked_red_entry_export_wrong_kind() {
    // otter_entry present but as a TABLE export — kind matters, not just the name.
    let fx = Fx {
        exports: vec![
            ("memory", ExportKind::Memory, 0),
            ("otter_entry", ExportKind::Table, 0),
            ("otter_table", ExportKind::Table, 0),
        ],
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(v.iter().any(|m| m.contains("otter_entry")), "{v:?}");
}

#[test]
fn linked_red_table_size_mismatch() {
    let v = linked_violations(&Fx::default(), &manifest(5, false));
    assert!(
        v.iter().any(|m| m.contains("manifest.table_size")),
        "table min/max vs manifest mismatch must be red: {v:?}"
    );
}

#[test]
fn linked_red_element_offset_nonzero() {
    let fx = Fx {
        element_offset: 1,
        element_items: 3, // keep items within the table at offset 1 (shape is parse-level anyway)
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("offset")),
        "nonzero element offset must be red (slot identity): {v:?}"
    );
}

#[test]
fn linked_red_element_count_short() {
    let fx = Fx {
        element_items: 3,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("slot coverage incomplete")),
        "short element segment must be red: {v:?}"
    );
}

#[test]
fn linked_red_imported_table() {
    // L-6 sub-branch negative: a table IMPORT alongside the declared table — the declared
    // table must be the module's own definition, an import is a foreign-slot surface.
    let fx = Fx {
        import_table: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("IMPORTS") && m.contains("table")),
        "an imported table must be red: {v:?}"
    );
}

#[test]
fn linked_red_multiple_element_segments() {
    // L-6 sub-branch negative: two active segments — the exactly-1 arm, not the shape checks.
    let fx = Fx {
        element_segments: 2,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("2 element segment(s)")),
        "multiple element segments must be red: {v:?}"
    );
}

#[test]
fn linked_red_imported_tag() {
    // L-6 sub-branch negative: EH infrastructure arriving as an IMPORT (no defined tag
    // section) — the census counts both halves.
    let fx = Fx {
        import_tag: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter()
            .any(|m| m.contains("tag section present") && m.contains("1 imported")),
        "an imported tag must be an EH infrastructure red: {v:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// check --linked — EH census negatives (machine counterpart of the WAT grep)
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_red_eh_operator() {
    let fx = Fx {
        eh_op: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter()
            .any(|m| m.contains("EH operator present") && m.contains("ThrowRef")),
        "throw_ref in a body must be an EH census red: {v:?}"
    );
}

#[test]
fn linked_red_tag_section() {
    let fx = Fx {
        tag: true,
        ..Fx::default()
    };
    let v = linked_violations(&fx, &manifest(4, false));
    assert!(
        v.iter().any(|m| m.contains("tag section present")),
        "a tag section must be an EH infrastructure red: {v:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// shrink-exports
// ---------------------------------------------------------------------------------------------

fn keep(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn shrink_green_and_still_linked_green() {
    let bytes = build(&Fx::default());
    let (out, report) =
        shrink_exports(&bytes, &keep(&["memory", "otter_entry", "otter_table"])).expect("green");
    assert_eq!(report.kept, vec!["memory", "otter_entry", "otter_table"]);
    assert_eq!(report.dropped, vec!["otter_binop"]);
    // Composition: the shrunk artifact still passes the §5.3 semantic gate.
    let linked = check_linked(&out, &manifest(4, false)).expect("post-shrink linked green");
    assert_eq!(linked.exports, 3);
}

#[test]
fn shrink_red_empty_keep() {
    let bytes = build(&Fx::default());
    match shrink_exports(&bytes, &BTreeSet::new()) {
        Err(LinkError::Shrink(m)) => assert!(m.contains("empty keep list"), "{m}"),
        other => panic!("empty keep must be red, got {other:?}"),
    }
}

#[test]
fn shrink_red_unmatched_keep() {
    let bytes = build(&Fx::default());
    match shrink_exports(&bytes, &keep(&["memory", "nope"])) {
        Err(LinkError::Shrink(m)) => {
            assert!(m.contains("nope") && m.contains("matched no export"), "{m}");
        }
        other => panic!("unmatched keep must be red, got {other:?}"),
    }
}

#[test]
fn shrink_red_no_export_section() {
    // A minimal module with no exports at all.
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    match shrink_exports(&module.finish(), &keep(&["memory"])) {
        Err(LinkError::Shrink(m)) => assert!(m.contains("no export section"), "{m}"),
        other => panic!("no export section must be red, got {other:?}"),
    }
}
