//! L-1a gate tests (design §7 L-1a row, verbatim): hand-built wasm fixtures —
//! ① invariance gate, BOTH modes (positive + negative per mode);
//! ② residual-import `=` gate incl. the LOST / EXCESS / TYPE-MISMATCH negatives (G-SF-3);
//! ③ the three kebab fail-closed rejections (§6.1);
//! ④ every rename product green under the wasmparser Validator (EH-excluded feature set —
//!    instantiation assertions are deliberately L-4's, design §0-5);
//! plus schema sanity (roundtrip + deny_unknown_fields) and CLI smoke via the built binary.

use otter_aot_link::imports::check_imports;
use otter_aot_link::invariance::assert_invariance;
use otter_aot_link::rename::rename;
use otter_aot_link::schema::{
    HelperName, ImportEntry, ImportKind, ManifestSidecar, ScratchEntry, ScratchRegion, manifest_from_json,
    manifest_to_json,
};
use otter_aot_link::section::scan_sections;
use otter_aot_link::{LinkError, RenameMaps, RenameMode};

use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, ElementSection, Elements, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, ImportSection, MemorySection, MemoryType, Module, RefType, TableSection,
    TableType, TypeSection, ValType,
};
use wasmparser::{Validator, WasmFeatures};

// ---------------------------------------------------------------------------------------------
// fixture builders (hand wasm — no compile_module, no wasmtime; §7 L-1a gate wording)
// ---------------------------------------------------------------------------------------------

const I64: ValType = ValType::I64;
const I32: ValType = ValType::I32;

fn func_types(types: &mut TypeSection) {
    // t0 binop, t1 unary, t2 host1, t3 host2, t4 entry, t5 hostctx2 — the O-14 §2.3 canonical
    // shapes (t5 added per L-1a review finding 1: the (2,true)→3 mapping needs a positive path).
    types.ty().function([I64, I64, I32, I32], [I64]);
    types.ty().function([I64, I32, I32], [I64]);
    types.ty().function([I64], [I64]);
    types.ty().function([I64, I64], [I64]);
    types.ty().function([I32, I32, I64], [I64]);
    types.ty().function([I64, I64, I64], [I64]);
}

fn funcref_table(min_max: u64) -> TableType {
    TableType {
        element_type: RefType::FUNCREF,
        minimum: min_max,
        maximum: Some(min_max),
        table64: false,
        shared: false,
    }
}

fn entry_body() -> Function {
    let mut f = Function::new([]);
    f.instructions().i64_const(42).end();
    f
}

/// Pre-link artifact shape (import-mode rename target): full O-14 import surface
/// (otter.memory + 3 helpers + 2 hosts), one defined entry function, declared table +
/// active element, exports, and a custom section (invariance passthrough material).
fn prelink_fixture(export_names: &[(&str, ExportKind, u32)]) -> Vec<u8> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    func_types(&mut types);
    module.section(&types);

    let mut imports = ImportSection::new();
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
    imports.import("otter_helpers", "otter_binop", EntityType::Function(0));
    imports.import("otter_helpers", "otter_convert", EntityType::Function(1));
    imports.import("otter_helpers", "otter_unop", EntityType::Function(1));
    imports.import("otter_host", "h_add7", EntityType::Function(2));
    imports.import("otter_host", "hc_xor", EntityType::Function(3));
    module.section(&imports);

    let mut funcs = FunctionSection::new();
    funcs.function(4); // entry type
    module.section(&funcs);

    let mut tables = TableSection::new();
    tables.table(funcref_table(7));
    module.section(&tables);

    let mut exports = ExportSection::new();
    for (name, kind, index) in export_names {
        exports.export(name, *kind, *index);
    }
    module.section(&exports);

    let mut elements = ElementSection::new();
    let offset = ConstExpr::i32_const(0);
    elements.active(Some(0), &offset, Elements::Functions([5u32][..].into()));
    module.section(&elements);

    let mut code = CodeSection::new();
    code.function(&entry_body());
    module.section(&code);

    module.section(&CustomSection {
        name: "spam".into(),
        data: b"eggs"[..].into(),
    });

    module.finish()
}

fn default_prelink() -> Vec<u8> {
    prelink_fixture(&[
        ("otter_entry", ExportKind::Func, 5),
        ("otter_table", ExportKind::Table, 0),
    ])
}

/// Post-link artifact shape (check --imports target): memory DEFINED (library-owner form),
/// residual imports as given — each entry (module, name, i64-param count).
fn linked_fixture(residuals: &[(&str, &str, usize)]) -> Vec<u8> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    func_types(&mut types);
    module.section(&types);

    if !residuals.is_empty() {
        let mut imports = ImportSection::new();
        for (m, n, params) in residuals {
            let tyidx = match params {
                1 => 2, // (i64) -> i64
                2 => 3, // (i64, i64) -> i64
                3 => 5, // (i64, i64, i64) -> i64 — HOSTCTX2 (L-1a review finding 1)
                4 => 0, // binop shape — used to model a leftover helper import
                other => panic!("fixture: unsupported param count {other}"),
            };
            imports.import(m, n, EntityType::Function(tyidx));
        }
        module.section(&imports);
    }

    let mut funcs = FunctionSection::new();
    funcs.function(4);
    module.section(&funcs);

    let mut memory = MemorySection::new();
    memory.memory(MemoryType {
        minimum: 17,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memory);

    let mut exports = ExportSection::new();
    exports.export("otter_entry", ExportKind::Func, residuals.len() as u32);
    module.section(&exports);

    let mut code = CodeSection::new();
    code.function(&entry_body());
    module.section(&code);

    module.finish()
}

/// Manifest sidecar with the standard link-consumed surface + the given otter_host entries.
fn manifest(hosts: &[(&str, u8, bool)]) -> ManifestSidecar {
    let mut imports = vec![
        ImportEntry {
            module: "otter".into(),
            name: "memory".into(),
            kind: ImportKind::Memory,
        },
        ImportEntry {
            module: "otter_helpers".into(),
            name: "otter_binop".into(),
            kind: ImportKind::Helper {
                helper: HelperName::Binop,
            },
        },
        ImportEntry {
            module: "otter_helpers".into(),
            name: "otter_convert".into(),
            kind: ImportKind::Helper {
                helper: HelperName::Convert,
            },
        },
        ImportEntry {
            module: "otter_helpers".into(),
            name: "otter_unop".into(),
            kind: ImportKind::Helper {
                helper: HelperName::Unop,
            },
        },
    ];
    for (name, arity, ctx) in hosts {
        imports.push(ImportEntry {
            module: "otter_host".into(),
            name: (*name).into(),
            kind: ImportKind::Host {
                arity: *arity,
                ctx: *ctx,
            },
        });
    }
    let nhosts = hosts.len() as u32;
    ManifestSidecar {
        entry_slot: 0,
        nfuncs: 2,
        helper_slots: [2, 3, 4],
        host_slots: (0..nhosts).map(|k| 5 + k).collect(),
        imports,
        scratch: vec![ScratchEntry {
            addr: 0x10000,
            words: 1,
        }],
        scratch_region: ScratchRegion {
            base: 0x10000,
            end: 0x10008,
        },
        table_size: 5 + nhosts,
        shared_memory: false,
        memory64: false,
        emitter_version: "0.0.0".into(),
    }
}

/// The §4.2 EH-excluded validation shape (all features minus exceptions/legacy-exceptions),
/// mirrored at the wasmparser level — the L-1a "Validator green" gate.
fn validate_eh_free(bytes: &[u8]) {
    let features = WasmFeatures::all()
        .difference(WasmFeatures::EXCEPTIONS)
        .difference(WasmFeatures::LEGACY_EXCEPTIONS);
    Validator::new_with_features(features)
        .validate_all(bytes)
        .expect("rename product must be Validator-green (EH-excluded feature set)");
}

fn flip_byte_in_section(bytes: &[u8], section_id: u8) -> Vec<u8> {
    let spans = scan_sections(bytes).expect("scan");
    let span = spans
        .iter()
        .find(|s| s.id == section_id)
        .unwrap_or_else(|| panic!("fixture has no section id {section_id}"));
    let mut doctored = bytes.to_vec();
    let idx = span.payload.end - 1;
    doctored[idx] ^= 0xff;
    doctored
}

fn import_maps(pairs: &[(&str, &str)]) -> RenameMaps {
    let mut maps = RenameMaps::default();
    for (old, new) in pairs {
        maps.modules.insert((*old).into(), (*new).into());
    }
    maps
}

// ---------------------------------------------------------------------------------------------
// rename: import mode + invariance (positive & negative)
// ---------------------------------------------------------------------------------------------

#[test]
fn rename_import_mode_green_and_invariant() {
    let original = default_prelink();
    let maps = import_maps(&[("otter", "otter_lib"), ("otter_helpers", "otter_lib")]);
    let renamed = rename(&original, RenameMode::Import, &maps).expect("rename green");

    validate_eh_free(&renamed);
    assert_invariance(&original, &renamed, RenameMode::Import).expect("§2-6-(a) invariance");

    // The import section must show the mapped module strings, otter_host untouched (§2-6).
    let mut mods: Vec<(String, String)> = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(&renamed) {
        if let wasmparser::Payload::ImportSection(r) = payload.expect("parse") {
            for i in r.into_imports() {
                let i = i.expect("import");
                mods.push((i.module.into(), i.name.into()));
            }
        }
    }
    assert_eq!(
        mods,
        vec![
            ("otter_lib".to_string(), "memory".to_string()),
            ("otter_lib".to_string(), "otter_binop".to_string()),
            ("otter_lib".to_string(), "otter_convert".to_string()),
            ("otter_lib".to_string(), "otter_unop".to_string()),
            ("otter_host".to_string(), "h_add7".to_string()),
            ("otter_host".to_string(), "hc_xor".to_string()),
        ]
    );
}

#[test]
fn rename_import_mode_unmatched_key_is_red() {
    let original = default_prelink();
    let maps = import_maps(&[("otter_bogus", "otter_lib")]);
    match rename(&original, RenameMode::Import, &maps) {
        Err(LinkError::RenameMapping(msg)) => assert!(msg.contains("otter_bogus"), "{msg}"),
        other => panic!("unmatched mapping key must be red, got {other:?}"),
    }
}

#[test]
fn invariance_negative_import_mode_code_byte_flip_is_red() {
    let original = default_prelink();
    let maps = import_maps(&[("otter", "otter_lib"), ("otter_helpers", "otter_lib")]);
    let renamed = rename(&original, RenameMode::Import, &maps).expect("rename green");
    let doctored = flip_byte_in_section(&renamed, 10); // code section
    match assert_invariance(&original, &doctored, RenameMode::Import) {
        Err(LinkError::Invariance(msg)) => assert!(msg.contains("id 10"), "{msg}"),
        other => panic!("code-section change must be an invariance red, got {other:?}"),
    }
}

#[test]
fn invariance_negative_custom_section_is_red_too() {
    // Custom sections are inside the invariance perimeter (§2-6: EVERY other section).
    let original = default_prelink();
    let maps = import_maps(&[("otter", "otter_lib"), ("otter_helpers", "otter_lib")]);
    let renamed = rename(&original, RenameMode::Import, &maps).expect("rename green");
    let doctored = flip_byte_in_section(&renamed, 0);
    assert!(
        matches!(
            assert_invariance(&original, &doctored, RenameMode::Import),
            Err(LinkError::Invariance(_))
        ),
        "custom-section change must be an invariance red"
    );
}

// ---------------------------------------------------------------------------------------------
// rename: export mode (kebab) + invariance (positive & negative)
// ---------------------------------------------------------------------------------------------

#[test]
fn rename_export_kebab_green_and_invariant() {
    let original = default_prelink();
    let maps = RenameMaps {
        kebab: true,
        ..RenameMaps::default()
    };
    let renamed = rename(&original, RenameMode::Export, &maps).expect("kebab green");

    validate_eh_free(&renamed);
    assert_invariance(&original, &renamed, RenameMode::Export).expect("§2-6-(b) invariance");

    let mut names: Vec<String> = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(&renamed) {
        if let wasmparser::Payload::ExportSection(r) = payload.expect("parse") {
            for e in r {
                names.push(e.expect("export").name.into());
            }
        }
    }
    assert_eq!(names, vec!["otter-entry".to_string(), "otter-table".to_string()]);
}

#[test]
fn invariance_negative_export_mode_type_byte_flip_is_red() {
    let original = default_prelink();
    let maps = RenameMaps {
        kebab: true,
        ..RenameMaps::default()
    };
    let renamed = rename(&original, RenameMode::Export, &maps).expect("kebab green");
    let doctored = flip_byte_in_section(&renamed, 1); // type section
    assert!(
        matches!(
            assert_invariance(&original, &doctored, RenameMode::Export),
            Err(LinkError::Invariance(_))
        ),
        "type-section change must be an invariance red in export mode"
    );
}

#[test]
fn invariance_modes_are_not_interchangeable() {
    // An import-section rewrite is a violation under the EXPORT-mode perimeter (§2-6 mode split).
    let original = default_prelink();
    let maps = import_maps(&[("otter", "otter_lib"), ("otter_helpers", "otter_lib")]);
    let renamed = rename(&original, RenameMode::Import, &maps).expect("rename green");
    assert!(
        matches!(
            assert_invariance(&original, &renamed, RenameMode::Export),
            Err(LinkError::Invariance(_))
        ),
        "import-section change must be red under export-mode invariance"
    );
}

// ---------------------------------------------------------------------------------------------
// kebab fail-closed rejections ①②③ (§6.1)
// ---------------------------------------------------------------------------------------------

#[test]
fn kebab_rejection_1_dash_in_source() {
    let original = prelink_fixture(&[
        ("otter-entry", ExportKind::Func, 5),
        ("otter_table", ExportKind::Table, 0),
    ]);
    let maps = RenameMaps {
        kebab: true,
        ..RenameMaps::default()
    };
    match rename(&original, RenameMode::Export, &maps) {
        Err(LinkError::KebabDashSource(msg)) => assert!(msg.contains("otter-entry"), "{msg}"),
        other => panic!("dash-in-source must be rejection ①, got {other:?}"),
    }
}

#[test]
fn kebab_rejection_2_post_substitution_collision() {
    let original = prelink_fixture(&[("h_a", ExportKind::Func, 5), ("b", ExportKind::Table, 0)]);
    let mut maps = RenameMaps {
        kebab: true,
        ..RenameMaps::default()
    };
    maps.names.insert("b".into(), "h-a".into()); // explicit target collides with kebab(h_a)
    match rename(&original, RenameMode::Export, &maps) {
        Err(LinkError::RenameCollision(msg)) => assert!(msg.contains("h-a"), "{msg}"),
        other => panic!("post-substitution collision must be rejection ②, got {other:?}"),
    }
}

#[test]
fn kebab_rejection_3_invalid_wit_identifier() {
    for bad in ["Otter_Entry", "_entry", "7start"] {
        let original = prelink_fixture(&[(bad, ExportKind::Func, 5), ("otter_table", ExportKind::Table, 0)]);
        let maps = RenameMaps {
            kebab: true,
            ..RenameMaps::default()
        };
        match rename(&original, RenameMode::Export, &maps) {
            Err(LinkError::KebabInvalidIdent(msg)) => assert!(msg.contains(bad), "{msg}"),
            other => panic!("`{bad}` must be rejection ③ (invalid WIT identifier), got {other:?}"),
        }
    }
}

#[test]
fn kebab_without_export_section_is_red() {
    // L-1a review finding 2: --kebab against a module with NO export section used to no-op
    // green (nothing to audit) — it must be a loud refusal (wrong-input-file guard).
    let mut module = Module::new();
    let mut types = TypeSection::new();
    func_types(&mut types);
    module.section(&types);
    let mut funcs = FunctionSection::new();
    funcs.function(4);
    module.section(&funcs);
    let mut code = CodeSection::new();
    code.function(&entry_body());
    module.section(&code);
    let exportless = module.finish();

    let maps = RenameMaps {
        kebab: true,
        ..RenameMaps::default()
    };
    match rename(&exportless, RenameMode::Export, &maps) {
        Err(LinkError::RenameMapping(msg)) => {
            assert!(msg.contains("no export section"), "{msg}")
        }
        other => panic!("kebab without an export section must be red, got {other:?}"),
    }
}

#[test]
fn kebab_is_export_mode_only() {
    let original = default_prelink();
    let maps = RenameMaps {
        kebab: true,
        ..RenameMaps::default()
    };
    assert!(
        matches!(
            rename(&original, RenameMode::Import, &maps),
            Err(LinkError::RenameMapping(_))
        ),
        "--kebab in import mode must be refused (§6.1)"
    );
}

// ---------------------------------------------------------------------------------------------
// check --imports: exact-match gate (green + LOST / EXCESS / TYPE-MISMATCH negatives, G-SF-3)
// ---------------------------------------------------------------------------------------------

#[test]
fn imports_exact_match_green() {
    let artifact = linked_fixture(&[("otter_host", "h_add7", 1), ("otter_host", "hc_xor", 2)]);
    let m = manifest(&[("h_add7", 1, false), ("hc_xor", 2, false)]);
    let report = check_imports(&artifact, &m, false).expect("exact match green");
    assert_eq!(report.matched_hosts, 2);
    assert!(!report.self_contained);
}

#[test]
fn imports_hostctx_param_arithmetic() {
    // (arity=1, ctx=true) → 2 i64 params (HOST2_TY — the Host2/HostCtx1 coincidence).
    let artifact = linked_fixture(&[("otter_host", "hc_xor", 2)]);
    let m = manifest(&[("hc_xor", 1, true)]);
    check_imports(&artifact, &m, false).expect("ctx word must count into the param total");
}

#[test]
fn imports_hostctx2_positive_path() {
    // L-1a review finding 1: the canonical (arity=2, ctx=true) → 3×i64 (HOSTCTX2_TY) mapping
    // had no positive execution — this is it, plus its type-mismatch negative.
    let artifact = linked_fixture(&[("otter_host", "hc_full", 3)]);
    let m = manifest(&[("hc_full", 2, true)]);
    let report = check_imports(&artifact, &m, false).expect("(2,true) → 3 i64 params, green");
    assert_eq!(report.matched_hosts, 1);

    // Same manifest against a 2-param artifact declaration: red on type mismatch.
    let narrow = linked_fixture(&[("otter_host", "hc_full", 2)]);
    match check_imports(&narrow, &m, false) {
        Err(LinkError::Imports(v)) => {
            assert!(v.iter().any(|s| s.contains("type mismatch")), "{v:?}")
        }
        other => panic!("HOSTCTX2 vs 2-param artifact must be red, got {other:?}"),
    }
}

#[test]
fn imports_lost_is_red() {
    // Proper subset — hc_xor LOST. `⊆` would pass this silently; `=` must refuse (G-SF-3).
    let artifact = linked_fixture(&[("otter_host", "h_add7", 1)]);
    let m = manifest(&[("h_add7", 1, false), ("hc_xor", 2, false)]);
    match check_imports(&artifact, &m, false) {
        Err(LinkError::Imports(v)) => {
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].contains("hc_xor") && v[0].contains("MISSING"), "{v:?}");
        }
        other => panic!("lost import must be red, got {other:?}"),
    }
}

#[test]
fn imports_excess_is_red() {
    // Excess of BOTH classes: a leftover helper import and an unmanifested host import.
    let artifact = linked_fixture(&[
        ("otter_helpers", "otter_binop", 4),
        ("otter_host", "h_add7", 1),
        ("otter_host", "h_extra", 1),
    ]);
    let m = manifest(&[("h_add7", 1, false)]);
    match check_imports(&artifact, &m, false) {
        Err(LinkError::Imports(v)) => {
            assert_eq!(v.len(), 2, "{v:?}");
            assert!(v.iter().any(|s| s.contains("otter_helpers.otter_binop")), "{v:?}");
            assert!(
                v.iter().any(|s| s.contains("h_extra") && s.contains("excess")),
                "{v:?}"
            );
        }
        other => panic!("excess imports must be red, got {other:?}"),
    }
}

#[test]
fn imports_type_mismatch_is_red() {
    // h_add7 declared (i64,i64)->i64 in the artifact, manifest says arity 1 → (i64)->i64.
    let artifact = linked_fixture(&[("otter_host", "h_add7", 2)]);
    let m = manifest(&[("h_add7", 1, false)]);
    match check_imports(&artifact, &m, false) {
        Err(LinkError::Imports(v)) => {
            assert!(v.iter().any(|s| s.contains("type mismatch")), "{v:?}");
        }
        other => panic!("type mismatch must be red, got {other:?}"),
    }
}

#[test]
fn imports_self_contained_modes() {
    let artifact = linked_fixture(&[]);
    let m_empty = manifest(&[]);
    // Host-free artifact: empty-set equality passes with and without the emphasis flag (§2-4).
    let report = check_imports(&artifact, &m_empty, false).expect("empty == empty");
    assert!(report.self_contained);
    check_imports(&artifact, &m_empty, true).expect("--require-self-contained green");

    // The emphasis flag refuses a manifest that declares host imports at all.
    let m_hosts = manifest(&[("h_add7", 1, false)]);
    assert!(
        matches!(
            check_imports(&artifact, &m_hosts, true),
            Err(LinkError::Imports(_))
        ),
        "--require-self-contained with declared hosts must be red"
    );
}

#[test]
fn imports_manifest_arity_out_of_canonical_space_is_manifest_error() {
    let artifact = linked_fixture(&[("otter_host", "h_add7", 1)]);
    let m = manifest(&[("h_add7", 3, false)]); // outside HOST1/HOST2/HOSTCTX2
    assert!(
        matches!(check_imports(&artifact, &m, false), Err(LinkError::Manifest(_))),
        "arity 3 must be a manifest error, never a pass"
    );
}

// ---------------------------------------------------------------------------------------------
// schema: JSON roundtrip + fail-closed unknown fields
// ---------------------------------------------------------------------------------------------

#[test]
fn schema_json_roundtrip() {
    let m = manifest(&[("h_add7", 1, false), ("hc_xor", 2, true)]);
    let json = manifest_to_json(&m).expect("serialize");
    let back = manifest_from_json(&json).expect("deserialize");
    assert_eq!(m, back, "ser→de must be identity");
}

#[test]
fn schema_unknown_field_is_refused() {
    let m = manifest(&[]);
    let mut v: serde_json::Value = serde_json::from_slice(&manifest_to_json(&m).expect("ser")).expect("json");
    v.as_object_mut()
        .expect("object")
        .insert("mystery_field".into(), serde_json::Value::Bool(true));
    let bytes = serde_json::to_vec(&v).expect("ser");
    assert!(
        matches!(manifest_from_json(&bytes), Err(LinkError::Manifest(_))),
        "unknown sidecar field must refuse (schema drift is never guessed over)"
    );
}

// ---------------------------------------------------------------------------------------------
// CLI smoke (the bin half wired end-to-end; exit-code contract 0/1)
// ---------------------------------------------------------------------------------------------

#[test]
fn cli_rename_and_check_smoke() {
    let bin = env!("CARGO_BIN_EXE_otter-aot-link");
    let dir = std::env::temp_dir().join(format!("otter-aot-link-l1a-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let input = dir.join("prelink.wasm");
    let output = dir.join("renamed.wasm");
    let manifest_path = dir.join("prelink.manifest.json");
    std::fs::write(&input, default_prelink()).expect("write fixture");
    std::fs::write(
        &manifest_path,
        manifest_to_json(&manifest(&[("h_add7", 1, false), ("hc_xor", 2, false)])).expect("ser"),
    )
    .expect("write manifest");

    // rename green (import mode).
    let st = std::process::Command::new(bin)
        .args([
            "rename",
            "--mode",
            "import",
            "--input",
            input.to_str().expect("utf8"),
            "--output",
            output.to_str().expect("utf8"),
            "--map-module",
            "otter=otter_lib",
            "--map-module",
            "otter_helpers=otter_lib",
        ])
        .status()
        .expect("spawn");
    assert!(st.success(), "CLI rename must exit 0");

    // standalone invariance check green.
    let st = std::process::Command::new(bin)
        .args([
            "check",
            "--rename-invariance",
            input.to_str().expect("utf8"),
            "--mode",
            "import",
            output.to_str().expect("utf8"),
        ])
        .status()
        .expect("spawn");
    assert!(st.success(), "CLI invariance check must exit 0");

    // kebab rejection ① via CLI: exit code 1 (gate red, not usage error).
    let dashed = dir.join("dashed.wasm");
    std::fs::write(
        &dashed,
        prelink_fixture(&[
            ("otter-entry", ExportKind::Func, 5),
            ("otter_table", ExportKind::Table, 0),
        ]),
    )
    .expect("write fixture");
    let st = std::process::Command::new(bin)
        .args([
            "rename",
            "--mode",
            "export",
            "--kebab",
            "--input",
            dashed.to_str().expect("utf8"),
            "--output",
            dir.join("never.wasm").to_str().expect("utf8"),
        ])
        .status()
        .expect("spawn");
    assert_eq!(st.code(), Some(1), "kebab rejection must exit 1");

    // check --imports red on the PRE-link artifact (helpers still resident) — exit 1.
    let st = std::process::Command::new(bin)
        .args([
            "check",
            "--imports",
            manifest_path.to_str().expect("utf8"),
            input.to_str().expect("utf8"),
        ])
        .status()
        .expect("spawn");
    assert_eq!(st.code(), Some(1), "resident helper imports must exit 1");

    std::fs::remove_dir_all(&dir).ok();
}
