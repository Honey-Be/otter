//! O-19 C5 gates (design `otter-o19-wasm64-design.md` §2.5/§7 C5 + the C0-C2 review's F2
//! carry-over): the schema-bridge NEGATIVES the C2 review deferred here, plus the sidecar
//! `memory64` field's back-compat contract.
//!
//! * **F2-1** — the A64-manifest refusal (schema.rs `TryFrom`): a `memory64: true` manifest
//!   must never become a v1 sidecar (the link/sidecar chain is wasm32-only until its external
//!   blockers clear — silently dropping the flag was the exact fail-open C2 closed). This is
//!   also the PRODUCT-PATH seam: otter-aot-e2e's `emit-aot` serializes through this very
//!   `TryFrom`, so the refusal here IS "emit-aot의 A64 사이드카 명시 거부".
//! * **F2-2** — the `narrow()` width refusal: a u64 address that does not fit u32 (possible
//!   since the O-19 C1 widening) must REFUSE, never truncate — both the per-function scratch
//!   entry and the region base.
//! * **back-compat** — `#[serde(default)]` on the new `memory64` sidecar field: an OLD sidecar
//!   (field absent) parses as `false`; default=false stays fail-closed because the consumer
//!   cross-check (`check --linked`'s memory64 predicate, tested in l4_gates.rs) refuses
//!   memory64 BYTES against a false/old manifest. `deny_unknown_fields` stays intact.
//!
//! Fixture note: [`otter_wasmemit::AotManifest`] is `#[non_exhaustive]` — tests cannot
//! construct a doctored one, but CAN mutate a real one (the addrtype.rs precedent), so the
//! fixtures below come from a live `compile_module` call on a minimal halt-only IR module
//! (hence the `otter` dev-dependency).

use otter::{Effect, Expr, FuncDef, Inst, Isa, OpDef, OpHints};
use otter_aot_link::LinkError;
use otter_aot_link::schema::{ManifestSidecar, manifest_from_json, manifest_to_json};
use otter_wasmemit::{AotManifest, AotOpts, HostImportName, compile_module};

/// The l4/l1b fixtures' scratch base (0x11_0000 — the b′ +atomics heap_floor).
const BASE: u64 = 0x11_0000;

/// A REAL manifest from a live `compile_module` on a one-op halt module: no host imports, one
/// function, one (words == 0) scratch entry — the smallest total product the bridge accepts.
fn tiny_manifest(memory64: bool) -> AotManifest {
    let isa = Isa {
        ops: vec![OpDef {
            name: "halt",
            effects: vec![Effect::Halt {
                value: Expr::LocalAt(0),
            }],
            hints: OpHints {
                pure: false,
                terminator: true,
            },
        }],
    };
    let module = otter::Module {
        funcs: vec![FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: 1,
            code: vec![Inst::new(0, 0, 0, 0)],
        }],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    let resolve = |_: usize| -> Option<HostImportName> { None };
    compile_module(
        &isa,
        &module,
        &resolve,
        &AotOpts::new(BASE).with_memory64(memory64),
    )
    .expect("tiny module compiles")
    .manifest
}

fn expect_manifest_err(result: Result<ManifestSidecar, LinkError>, needle: &str, what: &str) {
    match result {
        Err(LinkError::Manifest(msg)) => assert!(
            msg.contains(needle),
            "{what}: refusal must name `{needle}`, got: {msg}"
        ),
        other => panic!("{what}: expected a Manifest refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// F2-1 — the A64-manifest bridge refusal (schema.rs TryFrom, the emit-aot product-path seam)
// ---------------------------------------------------------------------------------------------

#[test]
fn f2_a64_manifest_is_refused_by_the_sidecar_bridge() {
    // Control first: the A32 twin of the same module bridges green (the refusal below is the
    // memory64 flag's doing, nothing else).
    let a32 = tiny_manifest(false);
    let sidecar = ManifestSidecar::try_from(&a32).expect("A32 manifest must bridge");
    assert!(!sidecar.memory64, "the A32 sidecar mirror must be false");

    // The REAL A64 manifest (live compile_module product, not a doctored flag): refused.
    let a64 = tiny_manifest(true);
    assert!(a64.memory64, "compile_module must mirror with_memory64(true)");
    expect_manifest_err(
        ManifestSidecar::try_from(&a64),
        "wasm32-only",
        "A64 manifest × v1 sidecar bridge",
    );

    // And the doctored direction (the F2 wording exactly): an A32 product whose flag is
    // flipped after the fact must hit the SAME refusal — the seam reads the flag, not the
    // bytes, so record-side forgery alone must already fail closed here.
    let mut forged = tiny_manifest(false);
    forged.memory64 = true;
    expect_manifest_err(
        ManifestSidecar::try_from(&forged),
        "wasm32-only",
        "forged memory64 flag × v1 sidecar bridge",
    );
}

// ---------------------------------------------------------------------------------------------
// F2-2 — the narrow() width refusals: refuse, never truncate (schema.rs, O-19 §2.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn f2_over_u32_scratch_entry_addr_is_refused_not_truncated() {
    let mut man = tiny_manifest(false);
    assert_eq!(
        man.scratch.len(),
        1,
        "one entry per function (words == 0 included)"
    );
    // A value whose u32 truncation would be the HARMLESS-LOOKING 0 — exactly the silent
    // corruption class the refusal exists for.
    man.scratch[0].addr = 1u64 << 32;
    expect_manifest_err(
        ManifestSidecar::try_from(&man),
        "does not fit the v1",
        "scratch addr 2^32 (A32-flagged manifest)",
    );
}

#[test]
fn f2_over_u32_scratch_region_base_is_refused_not_truncated() {
    let mut man = tiny_manifest(false);
    man.scratch_region.0 = (1u64 << 32) + 8;
    expect_manifest_err(
        ManifestSidecar::try_from(&man),
        "does not fit the v1",
        "scratch_region base > u32 (A32-flagged manifest)",
    );
}

// ---------------------------------------------------------------------------------------------
// sidecar memory64 field — #[serde(default)] back-compat + deny_unknown_fields intact
// ---------------------------------------------------------------------------------------------

/// A v1-generation sidecar JSON — NO memory64 field (the pre-C5 shape, verbatim from the
/// l4-era fixtures).
const OLD_SIDECAR: &str = r#"{
  "entry_slot": 0, "nfuncs": 1, "helper_slots": [1, 2, 3], "host_slots": [],
  "imports": [], "scratch": [{ "addr": 1114112, "words": 0 }],
  "scratch_region": { "base": 1114112, "end": 1114112 },
  "table_size": 4, "shared_memory": false, "emitter_version": "0.0.0"
}"#;

#[test]
fn old_sidecar_without_memory64_parses_as_false() {
    let sidecar = manifest_from_json(OLD_SIDECAR.as_bytes()).expect("old sidecar must still parse");
    assert!(
        !sidecar.memory64,
        "#[serde(default)]: an old sidecar is a wasm32 (false) claim — the consumer \
         cross-check refuses it against memory64 bytes, so the default stays fail-closed"
    );
}

#[test]
fn explicit_memory64_field_round_trips_and_unknown_fields_still_refuse() {
    // Explicit false round-trips through the serializer (field present from now on).
    let man = tiny_manifest(false);
    let sidecar = ManifestSidecar::try_from(&man).expect("bridge");
    let json = manifest_to_json(&sidecar).expect("serialize");
    let text = core::str::from_utf8(&json).expect("utf8");
    assert!(
        text.contains("\"memory64\": false"),
        "the serialized sidecar must CARRY the field explicitly: {text}"
    );
    let back = manifest_from_json(&json).expect("round trip");
    assert_eq!(back, sidecar, "serialize → parse must be the identity");

    // An explicit true PARSES (the schema can express it — the refusal against memory64 lives
    // at the bridge (producer) and the cross-checks (consumer), not in serde)…
    let forged = OLD_SIDECAR.replace(
        "\"shared_memory\": false",
        "\"shared_memory\": false, \"memory64\": true",
    );
    let parsed = manifest_from_json(forged.as_bytes()).expect("explicit true parses");
    assert!(parsed.memory64);

    // …and deny_unknown_fields survived the field addition (schema drift still refuses).
    let unknown = OLD_SIDECAR.replace(
        "\"shared_memory\": false",
        "\"shared_memory\": false, \"memory65\": true",
    );
    assert!(
        manifest_from_json(unknown.as_bytes()).is_err(),
        "unknown fields must still refuse (deny_unknown_fields)"
    );
}
