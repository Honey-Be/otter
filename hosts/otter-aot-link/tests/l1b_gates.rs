//! L-1b gate tests (design §7 L-1b row, verbatim):
//! - `check --scratch`: EVERY check item (i)/(ii)/(iii) with a positive AND a negative each
//!   (panel G-SF-7 — "검사 3종 각 1"은 과소);
//! - the overlap-segment injection negative (the `∩ region = ∅` sub-check, §5.6-⑩'s target);
//! - W ∈ {0, 8, ≥16} three-shape fixtures through inject → check;
//! - the shared-memory active-marker fixture at the wasmparser level (design [추정 ④] stage-1
//!   measurement — engine acceptance is L-4's);
//! - schema roundtrip (ser → de identity) for the library sidecar;
//!
//! plus the §2-1/§2-2/§4.3 fail-closed seams: input-SHA pair comparison, unfinalized sidecar
//! refusal, min-past-max loud failure, §8-6 loud extract failure, data-count bump, and CLI
//! smoke over the full extract → finalize → inject → check chain.

use otter_aot_link::lib_manifest::{extract, finalize};
use otter_aot_link::marker::{
    MARKER_BASE_MAGIC, MARKER_END_MAGIC, MARKER_MERGED_MAGIC, RegionShape, inject_markers,
};
use otter_aot_link::schema::{
    ImportEntry, LibrarySidecar, ManifestSidecar, ScratchEntry, ScratchRegion, library_from_json,
    library_to_json, manifest_to_json,
};
use otter_aot_link::scratch::check_scratch;
use otter_aot_link::{LinkError, sha256_hex};

use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, DataCountSection, DataSection, ExportKind, ExportSection,
    Function, FunctionSection, GlobalSection, GlobalType, MemorySection, MemoryType, Module, NameMap,
    NameSection, Section, TypeSection, ValType,
};
use wasmparser::{Validator, WasmFeatures};

// ---------------------------------------------------------------------------------------------
// fixture builders (hand wasm — no compile_module, no wasmtime)
// ---------------------------------------------------------------------------------------------

/// One data segment of a fixture.
enum Seg {
    /// Active at `i32.const offset` into memory 0.
    Active(u32, Vec<u8>),
    /// Passive (the §2-2 blind-spot material — +atomics `.rodata` shape).
    Passive(Vec<u8>),
}

/// Memory-owning module fixture: b′-shaped when `sp_init`+`with_names` are set, product-shaped
/// otherwise. Section order: type, func, memory, global, export, datacount, code, data, names.
struct Fx {
    shared: bool,
    min: u64,
    max: Option<u64>,
    /// `Some(v)` adds globals `[g0: const i32 0x400, g1: mut i32 v]`.
    sp_init: Option<i32>,
    /// Adds a name section naming g1 `__stack_pointer` (pre-strip shape).
    with_names: bool,
    /// Emits a data-count section (count == segs.len()).
    with_data_count: bool,
    segs: Vec<Seg>,
}

impl Default for Fx {
    fn default() -> Self {
        Fx {
            shared: false,
            min: 16,
            max: None,
            sp_init: Some(0x10_0000),
            with_names: true,
            with_data_count: false,
            segs: vec![],
        }
    }
}

fn build(fx: &Fx) -> Vec<u8> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I64]);
    module.section(&types);

    let mut funcs = FunctionSection::new();
    funcs.function(0);
    module.section(&funcs);

    let mut memory = MemorySection::new();
    memory.memory(MemoryType {
        minimum: fx.min,
        maximum: fx.max,
        memory64: false,
        shared: fx.shared,
        page_size_log2: None,
    });
    module.section(&memory);

    if let Some(sp) = fx.sp_init {
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(0x400),
        );
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(sp),
        );
        module.section(&globals);
    }

    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("run", ExportKind::Func, 0);
    module.section(&exports);

    if fx.with_data_count {
        module.section(&DataCountSection {
            count: fx.segs.len() as u32,
        });
    }

    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instructions().i64_const(7).end();
    code.function(&body);
    module.section(&code);

    if !fx.segs.is_empty() {
        let mut data = DataSection::new();
        for seg in &fx.segs {
            match seg {
                Seg::Active(offset, bytes) => {
                    data.active(0, &ConstExpr::i32_const(*offset as i32), bytes.iter().copied());
                }
                Seg::Passive(bytes) => {
                    data.passive(bytes.iter().copied());
                }
            }
        }
        module.section(&data);
    }

    if fx.with_names && fx.sp_init.is_some() {
        let mut names = NameSection::new();
        let mut map = NameMap::new();
        map.append(1, "__stack_pointer");
        names.globals(&map);
        module.section(&names);
    }

    module.finish()
}

/// (unstripped, stripped, finalized sidecar) — the §2-2 three-stage pipeline in one call.
fn bprime(fx: Fx) -> (Vec<u8>, Vec<u8>, LibrarySidecar) {
    assert!(fx.with_names, "b′ builder needs the pre-strip shape");
    let unstripped = build(&fx);
    let stripped = build(&Fx {
        with_names: false,
        ..fx
    });
    let extracted = extract(&unstripped, "0.0.0", "rustc-test").expect("extract green");
    let sidecar = finalize(&extracted, &stripped).expect("finalize green");
    (unstripped, stripped, sidecar)
}

fn nonshared_bprime() -> (Vec<u8>, Vec<u8>, LibrarySidecar) {
    bprime(Fx::default())
}

/// The +atomics-shaped b′: shared min 17 with max, passive `.rodata` stand-in + data count.
fn atomics_bprime() -> (Vec<u8>, Vec<u8>, LibrarySidecar) {
    bprime(Fx {
        shared: true,
        min: 17,
        max: Some(1024),
        with_data_count: true,
        segs: vec![Seg::Passive(b"panic-string-stand-in".to_vec())],
        ..Fx::default()
    })
}

/// Product manifest with the given region/profile (only link-consumed fields matter here).
fn product_manifest(base: u32, end: u64, shared: bool) -> ManifestSidecar {
    let imports: Vec<ImportEntry> = vec![];
    ManifestSidecar {
        entry_slot: 0,
        nfuncs: 1,
        helper_slots: [1, 2, 3],
        host_slots: vec![],
        imports,
        scratch: vec![ScratchEntry {
            addr: base,
            words: (end - u64::from(base)) / 8,
        }],
        scratch_region: ScratchRegion { base, end },
        table_size: 4,
        shared_memory: shared,
        memory64: false,
        emitter_version: "0.0.0".into(),
    }
}

/// §4.2 EH-excluded validation (mirrors the l1a helper — threads stay enabled for shared).
fn validate_eh_free(bytes: &[u8]) {
    let features = WasmFeatures::all()
        .difference(WasmFeatures::EXCEPTIONS)
        .difference(WasmFeatures::LEGACY_EXCEPTIONS);
    Validator::new_with_features(features)
        .validate_all(bytes)
        .expect("product must be Validator-green (EH-excluded feature set)");
}

/// Active data spans `(offset, bytes)` of a module, via the crate's own facts parser.
fn active_segments(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let facts = otter_aot_link::facts::ModuleFacts::parse(bytes).expect("parse");
    facts
        .data
        .iter()
        .filter_map(|d| {
            d.active
                .and_then(|a| a.static_offset)
                .map(|off| (off, d.bytes.clone()))
        })
        .collect()
}

fn memory_min(bytes: &[u8]) -> u64 {
    otter_aot_link::facts::ModuleFacts::parse(bytes)
        .expect("parse")
        .owned_memory("test")
        .expect("owned memory")
        .min
}

// ---------------------------------------------------------------------------------------------
// lib-manifest: extract (heap_floor formula incl. memory_min term) + §8-6 loud failure
// ---------------------------------------------------------------------------------------------

#[test]
fn extract_nonshared_floor_and_fields() {
    let (unstripped, _, _) = nonshared_bprime();
    let sc = extract(&unstripped, "0.0.0", "rustc-test").expect("extract green");
    // §2-2: max(sp 0x100000, active end 0, 16×65536 = 0x100000) = 0x100000.
    assert_eq!(sc.heap_floor, 0x10_0000);
    assert!(!sc.shared);
    assert_eq!((sc.memory_min, sc.memory_max), (16, None));
    assert_eq!(sc.exports, vec!["memory".to_string(), "run".to_string()]);
    assert_eq!(
        sc.artifact_sha256, None,
        "--extract must NOT bind the hash (§2-2 order)"
    );
}

#[test]
fn extract_atomics_shape_memory_min_term_wins() {
    // Panel M1's measured trap: sp = 0x100000, active data 0 (all passive) — the old two-term
    // formula would say 0x100000; the memory_min term (17 pages) must lift it to 0x110000.
    let (unstripped, _, _) = atomics_bprime();
    let sc = extract(&unstripped, "0.0.0", "rustc-test").expect("extract green");
    assert_eq!(
        sc.heap_floor, 0x11_0000,
        "memory_min×65536 must absorb the passive-segment blind spot"
    );
    assert!(sc.shared);
    assert_eq!((sc.memory_min, sc.memory_max), (17, Some(1024)));
}

#[test]
fn extract_active_data_end_term() {
    // An active segment ending past both other terms must set the floor.
    let unstripped = build(&Fx {
        segs: vec![Seg::Active(0x10_8000, vec![0xaa; 32])],
        ..Fx::default()
    });
    let sc = extract(&unstripped, "0.0.0", "rustc-test").expect("extract green");
    // max(0x100000, 0x108020, 0x100000) = 0x108020.
    assert_eq!(sc.heap_floor, 0x10_8020);
}

#[test]
fn extract_without_name_section_is_loud_failure() {
    // §8-6: no name-section identification → FAILURE, never the silent largest-mutable-i32
    // heuristic (extraction is pinned to the pre-strip artifact).
    let stripped = build(&Fx {
        with_names: false,
        ..Fx::default()
    });
    match extract(&stripped, "0.0.0", "rustc-test") {
        Err(LinkError::Extract(msg)) => {
            assert!(msg.contains("PRE-strip"), "{msg}");
            assert!(msg.contains("heuristic"), "{msg}");
        }
        other => panic!("stripped extract must fail loudly (§8-6), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// lib-manifest: finalize (content binding + wrong-pair refusal) + schema roundtrip
// ---------------------------------------------------------------------------------------------

#[test]
fn finalize_binds_content_hash() {
    let (_, stripped, sidecar) = nonshared_bprime();
    assert_eq!(
        sidecar.artifact_sha256.as_deref(),
        Some(sha256_hex(&stripped).as_str()),
        "finalize must bind the SHA-256 of the strip-final master bytes (§4.3)"
    );
    // Idempotent on the same bytes.
    let again = finalize(&sidecar, &stripped).expect("re-finalize same bytes");
    assert_eq!(again, sidecar);
}

#[test]
fn finalize_wrong_pair_is_red() {
    let (unstripped, _, _) = nonshared_bprime();
    let extracted = extract(&unstripped, "0.0.0", "rustc-test").expect("extract");
    // A DIFFERENT stripped artifact (min 17 instead of 16): metadata cross-check must refuse.
    let other = build(&Fx {
        with_names: false,
        min: 17,
        ..Fx::default()
    });
    assert!(
        matches!(finalize(&extracted, &other), Err(LinkError::Pair(_))),
        "finalize against a non-pair artifact must be red"
    );
}

#[test]
fn library_schema_roundtrip() {
    // §7 L-1b gate: schema 왕복 (ser → de identity), unfinalized AND finalized shapes.
    let (_, _, finalized) = nonshared_bprime();
    for sc in [
        LibrarySidecar {
            artifact_sha256: None,
            ..finalized.clone()
        },
        finalized,
    ] {
        let json = library_to_json(&sc).expect("ser");
        let back = library_from_json(&json).expect("de");
        assert_eq!(sc, back, "ser→de must be identity");
    }
}

#[test]
fn library_schema_unknown_field_is_refused() {
    let (_, _, sidecar) = nonshared_bprime();
    let mut v: serde_json::Value =
        serde_json::from_slice(&library_to_json(&sidecar).expect("ser")).expect("json");
    v.as_object_mut()
        .expect("object")
        .insert("mystery".into(), serde_json::Value::Bool(true));
    let bytes = serde_json::to_vec(&v).expect("ser");
    assert!(
        matches!(library_from_json(&bytes), Err(LinkError::Manifest(_))),
        "unknown library-sidecar field must refuse"
    );
}

// ---------------------------------------------------------------------------------------------
// inject-marker: W ∈ {0, 8, ≥16} three shapes → check --scratch green (the (i)/(ii)/(iii)
// positives), constructive min raise, SHA pair, min-past-max, data-count bump
// ---------------------------------------------------------------------------------------------

#[test]
fn inject_w_ge_16_two_markers_then_check_green() {
    let (_, stripped, sidecar) = nonshared_bprime();
    let manifest = product_manifest(0x11_0000, 0x11_0018, false); // W = 24
    let (out, report) = inject_markers(&stripped, &sidecar, &manifest).expect("inject green");

    assert_eq!(report.shape, RegionShape::TwoMarkers);
    assert_eq!((report.old_min, report.new_min, report.injected), (16, 18, 2));
    assert_eq!(memory_min(&out), 18, "constructive min raise (§2-1)");
    assert_eq!(
        active_segments(&out),
        vec![
            (0x11_0000, MARKER_BASE_MAGIC.to_vec()),
            (0x11_0010, MARKER_END_MAGIC.to_vec()),
        ]
    );
    validate_eh_free(&out);

    let report = check_scratch(&out, &manifest, &sidecar, &stripped).expect("all three checks green");
    assert_eq!(report.shape, RegionShape::TwoMarkers);
    assert_eq!((report.min_pages, report.required_pages), (18, 18));
    assert_eq!(report.heap_floor, 0x10_0000);
}

#[test]
fn inject_w8_merged_marker_then_check_green() {
    let (_, stripped, sidecar) = nonshared_bprime();
    let manifest = product_manifest(0x11_0000, 0x11_0008, false); // W = 8 — calc shape
    let (out, report) = inject_markers(&stripped, &sidecar, &manifest).expect("inject green");

    assert_eq!(report.shape, RegionShape::MergedWord);
    assert_eq!(report.injected, 1);
    assert_eq!(
        active_segments(&out),
        vec![(0x11_0000, MARKER_MERGED_MAGIC.to_vec())]
    );
    validate_eh_free(&out);

    let report = check_scratch(&out, &manifest, &sidecar, &stripped).expect("W==8 check green");
    assert_eq!(report.shape, RegionShape::MergedWord);
}

#[test]
fn inject_w0_is_explicit_noop_then_check_green() {
    let (_, stripped, sidecar) = nonshared_bprime();
    let manifest = product_manifest(0x11_0000, 0x11_0000, false); // W = 0 — call-free program
    let (out, report) = inject_markers(&stripped, &sidecar, &manifest).expect("inject green");

    assert_eq!(report.shape, RegionShape::Empty);
    assert_eq!((report.injected, report.old_min, report.new_min), (0, 16, 16));
    assert_eq!(out, stripped, "W==0 is a byte-for-byte no-op (§2-1)");

    let report = check_scratch(&out, &manifest, &sidecar, &stripped).expect("W==0 check green");
    assert_eq!(report.shape, RegionShape::Empty);
    assert_eq!(
        report.required_pages, 0,
        "coverage trivially green on the empty branch"
    );
}

#[test]
fn inject_sha_mismatch_is_red() {
    // §4.3 pair binding: a forged artifact_sha256 (1 hex char) must refuse BEFORE injecting.
    let (_, stripped, sidecar) = nonshared_bprime();
    let mut forged = sidecar.clone();
    let mut sha = forged.artifact_sha256.take().expect("finalized");
    let last = sha.pop().expect("nonempty");
    sha.push(if last == '0' { '1' } else { '0' });
    forged.artifact_sha256 = Some(sha);
    assert!(
        matches!(
            inject_markers(&stripped, &forged, &product_manifest(0x11_0000, 0x11_0018, false)),
            Err(LinkError::Pair(_))
        ),
        "forged sidecar hash must be a pair red"
    );
}

#[test]
fn inject_unfinalized_sidecar_is_red() {
    let (_, stripped, sidecar) = nonshared_bprime();
    let unfinalized = LibrarySidecar {
        artifact_sha256: None,
        ..sidecar
    };
    match inject_markers(
        &stripped,
        &unfinalized,
        &product_manifest(0x11_0000, 0x11_0018, false),
    ) {
        Err(LinkError::Pair(msg)) => assert!(msg.contains("UNFINALIZED"), "{msg}"),
        other => panic!("unfinalized sidecar must refuse, got {other:?}"),
    }
}

#[test]
fn inject_min_raise_past_declared_max_is_red() {
    // §2-1: raising past the declared max is a LOUD injection failure (shared profiles always
    // declare a max — modeled here with a non-shared max-17 memory needing 18 pages).
    let (_, stripped, sidecar) = bprime(Fx {
        min: 16,
        max: Some(17),
        ..Fx::default()
    });
    match inject_markers(
        &stripped,
        &sidecar,
        &product_manifest(0x11_0000, 0x11_0018, false),
    ) {
        Err(LinkError::Inject(msg)) => assert!(msg.contains("max 17"), "{msg}"),
        other => panic!("min raise past max must be an inject red, got {other:?}"),
    }
}

#[test]
fn inject_profile_mismatch_is_red() {
    // Library sidecar says non-shared, product manifest says shared → wrong-profile pairing.
    let (_, stripped, sidecar) = nonshared_bprime();
    assert!(
        matches!(
            inject_markers(&stripped, &sidecar, &product_manifest(0x11_0000, 0x11_0018, true)),
            Err(LinkError::Pair(_))
        ),
        "profile mismatch (O-14 §12.1 sharedness) must refuse"
    );
}

#[test]
fn inject_shared_memory_active_marker_wasmparser_level() {
    // Design [추정 ④] stage-1 measurement (§2-1/§7 L-1b row): active data-segment markers
    // TARGETING A SHARED MEMORY are wasmparser-Validator green (engine acceptance +
    // merge-survival are L-4-③). Also exercises the data-count bump and passive coexistence.
    let (_, stripped, sidecar) = atomics_bprime();
    let manifest = product_manifest(0x11_0000, 0x11_0018, true);
    let (out, report) = inject_markers(&stripped, &sidecar, &manifest).expect("inject green");

    assert_eq!(report.shape, RegionShape::TwoMarkers);
    assert_eq!((report.old_min, report.new_min), (17, 18));
    let facts = otter_aot_link::facts::ModuleFacts::parse(&out).expect("parse");
    assert_eq!(
        facts.data_count,
        Some(3),
        "data-count section must be bumped 1 → 3"
    );
    assert_eq!(facts.data.len(), 3, "passive .rodata stand-in + 2 markers");
    validate_eh_free(&out); // ← the actual measurement (threads enabled, EH excluded)

    let report = check_scratch(&out, &manifest, &sidecar, &stripped).expect("shared check green");
    assert_eq!(report.min_pages, 18);
}

// ---------------------------------------------------------------------------------------------
// check --scratch negatives: (i) marker missing / moved / wrong magic / duplicate,
// (i)-∩ overlap injection, (ii) coverage, (iii) floor, pair seam
// ---------------------------------------------------------------------------------------------

/// A linked-product-shaped module (no name section needed) with the given memory min and data.
fn product(min: u64, segs: Vec<Seg>) -> Vec<u8> {
    build(&Fx {
        min,
        sp_init: None,
        with_names: false,
        segs,
        ..Fx::default()
    })
}

/// The standard b′ pair used as the floor/pair side of the scratch negatives.
fn lib_pair() -> (Vec<u8>, LibrarySidecar) {
    let (_, stripped, sidecar) = nonshared_bprime();
    (stripped, sidecar)
}

#[test]
fn scratch_missing_end_marker_is_red() {
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0018, false);
    let artifact = product(18, vec![Seg::Active(0x11_0000, MARKER_BASE_MAGIC.to_vec())]);
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].contains("missing marker") && v[0].contains("OTTRSCR1"),
                "{v:?}"
            );
        }
        other => panic!("missing end marker must be red, got {other:?}"),
    }
}

#[test]
fn scratch_moved_marker_is_red() {
    // Base marker shifted +8: no longer a slot → overlap violation AND missing-marker.
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0018, false);
    let artifact = product(
        18,
        vec![
            Seg::Active(0x11_0008, MARKER_BASE_MAGIC.to_vec()),
            Seg::Active(0x11_0010, MARKER_END_MAGIC.to_vec()),
        ],
    );
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert!(v.iter().any(|s| s.contains("intersects")), "{v:?}");
            assert!(v.iter().any(|s| s.contains("missing marker")), "{v:?}");
        }
        other => panic!("moved marker must be red, got {other:?}"),
    }
}

#[test]
fn scratch_wrong_magic_is_red() {
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0008, false);
    let artifact = product(18, vec![Seg::Active(0x11_0000, b"OTTRSCRX".to_vec())]);
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert!(v.iter().any(|s| s.contains("wrong magic")), "{v:?}")
        }
        other => panic!("wrong magic must be red, got {other:?}"),
    }
}

#[test]
fn scratch_duplicate_marker_is_red() {
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0008, false);
    let artifact = product(
        18,
        vec![
            Seg::Active(0x11_0000, MARKER_MERGED_MAGIC.to_vec()),
            Seg::Active(0x11_0000, MARKER_MERGED_MAGIC.to_vec()),
        ],
    );
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert!(v.iter().any(|s| s.contains("duplicate marker")), "{v:?}")
        }
        other => panic!("duplicate marker must be red, got {other:?}"),
    }
}

#[test]
fn scratch_overlap_segment_injection_is_red() {
    // §7 L-1b row verbatim: the overlap-segment injection negative — markers all correct,
    // but a NON-marker segment sits inside the region (mutation ⑩'s unit-level twin; (ii) and
    // (iii) are green here, so ONLY the ∩ = ∅ sub-check can catch it).
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0020, false); // W = 32
    let artifact = product(
        18,
        vec![
            Seg::Active(0x11_0000, MARKER_BASE_MAGIC.to_vec()),
            Seg::Active(0x11_0018, MARKER_END_MAGIC.to_vec()),
            Seg::Active(0x11_0008, b"INTRUDER".to_vec()), // inside [base, end)
        ],
    );
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].contains("intersects") && v[0].contains("reservation violated"),
                "{v:?}"
            );
        }
        other => panic!("overlap segment must be red, got {other:?}"),
    }
}

#[test]
fn scratch_boundary_touching_segment_is_green() {
    // ∩ = ∅ is about INTERSECTION: segments ending exactly at base / starting exactly at end
    // are legal neighbors, not violations.
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0008, false);
    let artifact = product(
        19,
        vec![
            Seg::Active(0x10_fff8, vec![0xbb; 8]), // ends AT base
            Seg::Active(0x11_0000, MARKER_MERGED_MAGIC.to_vec()),
            Seg::Active(0x11_0008, vec![0xcc; 8]), // starts AT end
        ],
    );
    check_scratch(&artifact, &manifest, &sidecar, &lib).expect("touching neighbors are green");
}

#[test]
fn scratch_coverage_undersize_is_red() {
    // (ii) negative: markers green, floor green, min 16 < required 18 (E8's static twin).
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0018, false);
    let artifact = product(
        16,
        vec![
            Seg::Active(0x11_0000, MARKER_BASE_MAGIC.to_vec()),
            Seg::Active(0x11_0010, MARKER_END_MAGIC.to_vec()),
        ],
    );
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].contains("(ii)") && v[0].contains("not covered"), "{v:?}");
        }
        other => panic!("undersize memory must be red, got {other:?}"),
    }
}

#[test]
fn scratch_floor_violation_is_red() {
    // (iii) negative: the spike's base=0x10000 against the real library floor 0x100000 —
    // SPK §5-2's shadow-stack overlap trap, statically refused.
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x1_0000, 0x1_0008, false);
    let artifact = product(16, vec![Seg::Active(0x1_0000, MARKER_MERGED_MAGIC.to_vec())]);
    match check_scratch(&artifact, &manifest, &sidecar, &lib) {
        Err(LinkError::Scratch(v)) => {
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].contains("(iii)") && v[0].contains("RE-EMIT"), "{v:?}");
        }
        other => panic!("floor violation must be red, got {other:?}"),
    }
}

#[test]
fn scratch_wrong_lib_pair_is_red() {
    // §4.3 seam on the check side: right sidecar, WRONG b′ master bytes handed in.
    let (_, sidecar) = lib_pair();
    let other_lib = build(&Fx {
        with_names: false,
        min: 17,
        ..Fx::default()
    });
    let manifest = product_manifest(0x11_0000, 0x11_0008, false);
    let artifact = product(18, vec![Seg::Active(0x11_0000, MARKER_MERGED_MAGIC.to_vec())]);
    assert!(
        matches!(
            check_scratch(&artifact, &manifest, &sidecar, &other_lib),
            Err(LinkError::Pair(_))
        ),
        "wrong lib artifact must be a pair red"
    );
}

// ---------------------------------------------------------------------------------------------
// CLI smoke: the full extract → strip → finalize → inject → check chain via the built binary
// ---------------------------------------------------------------------------------------------

#[test]
fn cli_l1b_chain_smoke() {
    let bin = env!("CARGO_BIN_EXE_otter-aot-link");
    let dir = std::env::temp_dir().join(format!("otter-aot-link-l1b-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let (unstripped, stripped, _) = nonshared_bprime();
    let un_path = dir.join("bprime.unstripped.wasm");
    let st_path = dir.join("bprime.wasm");
    let sc_path = dir.join("otter_helpers.manifest.json");
    let mf_path = dir.join("product.manifest.json");
    let out_path = dir.join("bprime.injected.wasm");
    std::fs::write(&un_path, &unstripped).expect("write");
    std::fs::write(&st_path, &stripped).expect("write");
    std::fs::write(
        &mf_path,
        manifest_to_json(&product_manifest(0x11_0000, 0x11_0018, false)).expect("ser"),
    )
    .expect("write");

    let run = |args: &[&str]| {
        std::process::Command::new(bin)
            .args(args)
            .output()
            .expect("spawn")
    };

    // extract (pre-strip) → finalize (strip-final) — §2-2 order.
    let out = run(&[
        "lib-manifest",
        "--extract",
        un_path.to_str().expect("utf8"),
        "--output",
        sc_path.to_str().expect("utf8"),
        "--otter-version",
        "0.0.0",
        "--rustc-version",
        "rustc-test",
    ]);
    assert!(out.status.success(), "extract: {out:?}");
    let out = run(&[
        "lib-manifest",
        "--finalize",
        st_path.to_str().expect("utf8"),
        "--sidecar",
        sc_path.to_str().expect("utf8"),
    ]);
    assert!(out.status.success(), "finalize: {out:?}");

    // inject → check, green.
    let out = run(&[
        "inject-marker",
        "--input",
        st_path.to_str().expect("utf8"),
        "--output",
        out_path.to_str().expect("utf8"),
        "--manifest",
        mf_path.to_str().expect("utf8"),
        "--lib-manifest",
        sc_path.to_str().expect("utf8"),
    ]);
    assert!(out.status.success(), "inject: {out:?}");
    let out = run(&[
        "check",
        "--scratch",
        "--manifest",
        mf_path.to_str().expect("utf8"),
        "--lib-manifest",
        sc_path.to_str().expect("utf8"),
        "--lib-artifact",
        st_path.to_str().expect("utf8"),
        out_path.to_str().expect("utf8"),
    ]);
    assert!(out.status.success(), "check --scratch: {out:?}");

    // Forge one byte of the bound hash → inject must exit 1 (gate red, not usage).
    let sidecar = library_from_json(&std::fs::read(&sc_path).expect("read")).expect("de");
    let mut forged = sidecar;
    let sha = forged.artifact_sha256.take().expect("finalized");
    let flipped = if sha.ends_with('0') {
        format!("{}1", &sha[..63])
    } else {
        format!("{}0", &sha[..63])
    };
    forged.artifact_sha256 = Some(flipped);
    std::fs::write(&sc_path, library_to_json(&forged).expect("ser")).expect("write");
    let out = run(&[
        "inject-marker",
        "--input",
        st_path.to_str().expect("utf8"),
        "--output",
        out_path.to_str().expect("utf8"),
        "--manifest",
        mf_path.to_str().expect("utf8"),
        "--lib-manifest",
        sc_path.to_str().expect("utf8"),
    ]);
    assert_eq!(out.status.code(), Some(1), "forged hash must exit 1: {out:?}");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// L-2 hardening — the L-1b review's four carried-over prescriptions, each with its test
// (design §7 L-2 row rider; prescriptions recorded in the L-3/L-1a/L-1b commit message)
// ---------------------------------------------------------------------------------------------

#[test]
fn scratch_empty_region_spanning_segment_is_green() {
    // (a) the W==0 twin: `[base, base)` reserves nothing, so a segment SPANNING the empty
    // point is NOT an intersection. Before the fix, the ∩ sub-check's `off < end &&
    // off+len > base` still fired when base == end — a false red on a legal artifact.
    let (lib, sidecar) = lib_pair();
    let manifest = product_manifest(0x11_0000, 0x11_0000, false); // W = 0
    let artifact = product(18, vec![Seg::Active(0x10_fff8, vec![0xdd; 16])]); // spans 0x110000
    let report = check_scratch(&artifact, &manifest, &sidecar, &lib)
        .expect("a segment spanning an EMPTY region must be green (nothing to intersect)");
    assert_eq!(report.shape, RegionShape::Empty);
}

/// Minimal memory-owning module with an exotic memory shape (no sanctioned pipeline produces
/// these — the point of (b)/(c) is that they must REFUSE, not silently mis-compute/rewrite).
fn exotic_memory_module(memory64: bool, page_size_log2: Option<u32>) -> Vec<u8> {
    let mut module = Module::new();
    let mut memory = MemorySection::new();
    memory.memory(MemoryType {
        minimum: 16,
        maximum: None,
        memory64,
        shared: false,
        page_size_log2,
    });
    module.section(&memory);
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    module.section(&exports);
    module.finish()
}

/// Assert all four owned_memory consumer paths (extract / finalize / inject / check --scratch)
/// refuse `bad` with a `Malformed` mentioning `needle` — the common-seam property the L-2
/// hardening chose over four per-path rejections (one wiring point for one invariant).
fn assert_all_owned_memory_paths_refuse(bad: &[u8], needle: &str) {
    let refused = |what: &str, r: Result<(), LinkError>| match r {
        Err(LinkError::Malformed(m)) => assert!(m.contains(needle), "{what}: {m}"),
        other => panic!("{what} must refuse with Malformed({needle}), got {other:?}"),
    };

    // lib-manifest --extract.
    refused("extract", extract(bad, "0.0.0", "rustc-test").map(drop));

    // lib-manifest --finalize (sidecar from a NORMAL b′ — owned_memory fires before the
    // metadata cross-check can turn this into a mere pair mismatch).
    let (unstripped, _, _) = nonshared_bprime();
    let extracted = extract(&unstripped, "0.0.0", "rustc-test").expect("extract");
    refused("finalize", finalize(&extracted, bad).map(drop));

    // inject-marker (sidecar hand-bound over the bad bytes so the §4.3 SHA seam PASSES —
    // proving the refusal is owned_memory's, not the pair check's).
    let sidecar = LibrarySidecar {
        heap_floor: 0x10_0000,
        shared: false,
        memory_min: 16,
        memory_max: None,
        exports: vec!["memory".into()],
        otter_version: "0.0.0".into(),
        rustc_version: "rustc-test".into(),
        artifact_sha256: Some(sha256_hex(bad)),
    };
    let manifest = product_manifest(0x11_0000, 0x11_0008, false);
    refused("inject", inject_markers(bad, &sidecar, &manifest).map(drop));

    // check --scratch (bad bytes as the PRODUCT artifact; the lib pair itself is valid).
    let (lib, lib_sidecar) = lib_pair();
    refused(
        "check",
        check_scratch(bad, &manifest, &lib_sidecar, &lib).map(drop),
    );
}

#[test]
fn memory64_is_refused_on_every_owned_memory_path() {
    // (c) memory64 refusal at the common seam: before, only --extract refused — inject would
    // even have silently REWRITTEN memory64→false in its memory re-encode (the review's
    // silent-rewrite class; the re-encode now carries the field faithfully AND can never see
    // a memory64 input).
    assert_all_owned_memory_paths_refuse(&exotic_memory_module(true, None), "memory64");
}

#[test]
fn custom_page_size_is_refused_on_every_owned_memory_path() {
    // (b) custom-page-size refusal: `memory_min × 65536` (heap_floor) and `⌈end/65536⌉`
    // (coverage) both assume 64 KiB pages — a 1-byte-page memory would make every formula
    // silently wrong (fail-open). Refused loudly instead.
    assert_all_owned_memory_paths_refuse(&exotic_memory_module(false, Some(0)), "custom page size");
}

#[test]
fn finalize_rebind_same_metadata_different_bytes_is_red() {
    // (d) the finalize re-bind refusal branch, previously untested: a FINALIZED sidecar × an
    // artifact with IDENTICAL metadata (memory + exports both cross-check green) but different
    // bytes — only the already-bound content hash can catch it, and it must (no silent rebind).
    let (_, stripped, sidecar) = nonshared_bprime();
    let mut other = stripped.clone();
    CustomSection {
        name: "l2-rebind-probe".into(),
        data: (&[1u8, 2, 3][..]).into(),
    }
    .append_to(&mut other);

    // Premise proof: same metadata, different bytes.
    let f_a = otter_aot_link::facts::ModuleFacts::parse(&stripped).expect("parse");
    let f_b = otter_aot_link::facts::ModuleFacts::parse(&other).expect("parse");
    assert_eq!(f_a.exports, f_b.exports, "premise: identical exports");
    assert_eq!(f_a.memories, f_b.memories, "premise: identical memory shape");
    assert_ne!(stripped, other, "premise: different bytes");

    match finalize(&sidecar, &other) {
        Err(LinkError::Pair(m)) => assert!(m.contains("refusing to rebind"), "{m}"),
        other => panic!("re-bind over different bytes must be red, got {other:?}"),
    }
}
