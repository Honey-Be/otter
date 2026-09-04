//! ─────────────────────── O-16c′ L-6 MUTATION LEDGER (design §5.6 ①~⑩) ───────────────────────
//!
//! §7 L-6's "뮤테이션 전건 적색 실증 기록", A-4-ledger precedent verbatim (crates/otter-wasmemit/
//! tests/aot.rs): each EXECUTED entry below was a TEMPORARY tamper of a COPY of the real chain
//! artifacts (`just wasm-aot-link-e2e` / `just wasm-aot-component` outputs under
//! hosts/otter-aot-e2e/target/link-e2e/, b′ under hosts/otter-helpers-wasm/target/bprime/), the
//! red observed and recorded verbatim-in-gist, the copy discarded — the repo tree was never
//! touched. Entries marked PERMANENT-SUBSTITUTE follow the A-3 precedent: they are standing
//! tests rebuilt on every run (named below; the [`l6_permanent_substitute_tests_exist`]
//! meta-test pins their existence), strictly stronger than a one-off drill record.
//!
//! To REPRODUCE any executed entry: run the two recipes above once (artifacts appear at the
//! paths named), copy the named artifact to a scratch dir, apply the quoted tamper (WAT
//! round-trip via the PINNED wasm-tools — print → edit → parse; §3.2 non-PATH paths), run the
//! quoted gate command, observe the quoted red. Tool pins: binaryen version_132 + wasm-tools
//! 1.246.2 (justfile pin variables; a pin bump re-runs this whole ledger — design §3.2).
//!
//! ── ① provider-helper export removal → §5.1 red (E7 re-run, REAL chain) — EXECUTED ──
//!   Tamper: `otter-aot-link shrink-exports --input bprime.calc.wasm --keep memory --keep
//!   otter_convert --keep otter_unop` (drops otter_binop from the injected b′ provider), then
//!   the recipe's wasm-merge line verbatim (calc.renamed.wasm as main).
//!   Red (2026-09-04): wasm-merge exits **0** (the SPK §4-1 silent part-link, live) and
//!   `check --imports calc.manifest.json` fails: "residual non-host import
//!   `otter_lib.otter_binop` (must be 0 — §5.1: only the manifest's otter_host set may
//!   remain)". Unit-level backing (fixture, standing): l1a `imports_lost_is_red` /
//!   `imports_excess_is_red`.
//!
//! ── ② marker segment removal / 8-byte shift → §5.2-(i) red — PERMANENT-SUBSTITUTE ──
//!   Standing tests (l1b, real inject-marker products at the wasmparser level):
//!   `scratch_missing_end_marker_is_red`, `scratch_moved_marker_is_red`,
//!   `scratch_wrong_magic_is_red`, `scratch_duplicate_marker_is_red`.
//!
//! ── ③ memory-min undersize → §5.2-(ii) red + execution-trap negative (E8) — EXECUTED ──
//!   Static half is PERMANENT (l1b `scratch_coverage_undersize_is_red`); the REAL-chain drill
//!   and both execution variants were run (2026-09-04), tamper = WAT round-trip of
//!   calc.final.wasm with `(memory (;0;) 18)` → `17`:
//!   - static: `check --scratch` red — "(ii) memory min 17 page(s) < required 18
//!     (⌈0x110008/65536⌉) — scratch region not covered".
//!   - markers INTACT, run (`otter-aot-e2e run-linked calc`): "wasm trap: out of bounds
//!     memory access" at INSTANTIATION — the active marker segment at 0x110000 no longer fits.
//!     RECORDED FINDING: the §2-1/§2-3 marker moves the failure EARLIER than the spike's E8
//!     (instantiation-time, before any call) — the belt-and-suspenders working as designed.
//!   - markers ALSO removed (data lines deleted — the pre-marker shape): E8 verbatim on the
//!     real chain — "memory fault at wasm address 0x110000 in linear memory of size 0x110000"
//!     at the FIRST MARSHALLING STORE (SPK §3-3/E8's trap, relocated 0x10000 → 0x110000 by the
//!     L-4 base upgrade).
//!
//! ── ④ scratch_base=0x10000 violation manifest → §5.2-(iii) red — PERMANENT-SUBSTITUTE ──
//!   Standing twice: l1b `scratch_floor_violation_is_red` (unit) AND the recipe-level
//!   NEGPROBE-SCRATCH-III probe inside `just wasm-aot-link-e2e` (a base=0x10000 manifest
//!   against the REAL calc.linked.wasm + real b′ sidecar must fail naming "(iii)" — runs on
//!   every chain invocation, CI included).
//!
//! ── ⑤ renamer code-section byte tamper → §2-6 invariance red — PERMANENT-SUBSTITUTE ──
//!   Standing tests (l1a): `invariance_negative_import_mode_code_byte_flip_is_red` (code byte
//!   flip, import mode) + `invariance_negative_export_mode_type_byte_flip_is_red` (type byte,
//!   export mode) + `invariance_negative_custom_section_is_red_too`.
//!
//! ── ⑥ kebab rename omitted → loud chain red (E10 family) — EXECUTED, locus corrected ──
//!   Tamper: skip the kebab rename — `component embed --world aot-artifact deep.final.wasm`
//!   (exports still snake `otter_entry`).
//!   Red (2026-09-04): **embed exits 0** — the design's "embed 실패" guess was WRONG about the
//!   locus (embed only attaches world metadata); the refusal fires at the NEXT chain step,
//!   `component new`: "failed to find export of function `otter-entry`" — loud, names the
//!   missing kebab export, no component is produced. Fail-closed still holds one step later,
//!   and `check --component` (empty-world/world-shape) sits behind it as the standing net:
//!   l5 `component_red_empty_world` + the recipe's NEGPROBE-EMPTY-WORLD-OK (E10's
//!   new-without-embed half, every `just wasm-aot-component` run).
//!
//! ── ⑦ shrink drops otter_table → §5.3 red — EXECUTED ──
//!   Tamper: `shrink-exports --input calc.linked.wasm --keep memory --keep otter_entry`.
//!   Red (2026-09-04): `check --linked` — "`otter_table` table export missing". Unit backing
//!   (standing): l4 `linked_red_missing_table_export`.
//!
//! ── ⑧ EH-operator injection → EH-free gate rejection direction (panel MF-3) — EXECUTED ──
//!   Tamper: WAT round-trip of calc.linked.wasm appending `(func $__eh_probe try_table end)`
//!   (parse exits 0 — syntax level; the GATES must be the red). Three channels, all red
//!   (2026-09-04):
//!   - §4.2/§5.5 pinned CLI line verbatim (`validate
//!     --features=all,-exceptions,-legacy-exceptions`): "func 12 failed to validate:
//!     exceptions support is not enabled" — the gate's FIRST measured rejection (green runs
//!     only, before this).
//!   - recipe WAT census grep: 1 (≠ 0 ⇒ recipe red).
//!   - `check --linked` machine census: "EH operator present: func 12: TryTable { … }".
//!
//!   Unit backing (standing): l4 `linked_red_eh_operator` / `linked_red_tag_section`, l5
//!   `component_red_eh_in_nested_module` (the component-side rejection).
//!
//! ── ⑨ sidecar forgery (SHA/pair) → inject-marker·check red — PERMANENT-SUBSTITUTE ──
//!   Standing tests (l1b): `inject_sha_mismatch_is_red` (artifact_sha256 byte flip),
//!   `finalize_wrong_pair_is_red`, `scratch_wrong_lib_pair_is_red` (mispaired sidecar),
//!   `finalize_rebind_same_metadata_different_bytes_is_red` (re-bind refusal).
//!
//! ── ⑩ non-marker data segment inside the region → §5.2-(i) ∩=∅ red — PERMANENT-SUBSTITUTE ──
//!   Standing tests (l1b): `scratch_overlap_segment_injection_is_red` (the sub-check ② alone
//!   cannot bite — panel G-SF-7) + `scratch_boundary_touching_segment_is_green` (the
//!   boundary guard against over-rejection) + `scratch_empty_region_spanning_segment_is_green`
//!   (W==0 exemption).
//!
//! ─────────────────────────────────────────────────────────────────────────────────────────────

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Anti-silent-skip guard (the A-4 `a4_gate_registry_tests_exist_and_none_are_ignored`
/// precedent): every PERMANENT-SUBSTITUTE test named in the ledger above must still exist in
/// its named suite file, and none may be `#[ignore]`d. Textual, deliberately dumb — it catches
/// renames/deletions of the substitute tests (which would quietly vacate a ledger entry), not
/// semantic drift (the tests' own assertions own that).
#[test]
fn l6_permanent_substitute_tests_exist() {
    let by_file: BTreeMap<&str, Vec<&str>> = BTreeMap::from([
        (
            "l1a_gates.rs",
            vec![
                // ① unit backing
                "imports_lost_is_red",
                "imports_excess_is_red",
                // ⑤
                "invariance_negative_import_mode_code_byte_flip_is_red",
                "invariance_negative_export_mode_type_byte_flip_is_red",
                "invariance_negative_custom_section_is_red_too",
            ],
        ),
        (
            "l1b_gates.rs",
            vec![
                // ②
                "scratch_missing_end_marker_is_red",
                "scratch_moved_marker_is_red",
                "scratch_wrong_magic_is_red",
                "scratch_duplicate_marker_is_red",
                // ③ static half
                "scratch_coverage_undersize_is_red",
                // ④
                "scratch_floor_violation_is_red",
                // ⑨
                "inject_sha_mismatch_is_red",
                "finalize_wrong_pair_is_red",
                "scratch_wrong_lib_pair_is_red",
                "finalize_rebind_same_metadata_different_bytes_is_red",
                // ⑩
                "scratch_overlap_segment_injection_is_red",
                "scratch_boundary_touching_segment_is_green",
                "scratch_empty_region_spanning_segment_is_green",
            ],
        ),
        (
            "l4_gates.rs",
            vec![
                // ⑦ / ⑧ unit backing
                "linked_red_missing_table_export",
                "linked_red_eh_operator",
                "linked_red_tag_section",
            ],
        ),
        (
            "l5_gates.rs",
            vec![
                // ⑥ standing net / ⑧ component side
                "component_red_empty_world",
                "component_red_eh_in_nested_module",
            ],
        ),
    ]);

    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    for (file, tests) in by_file {
        let src = fs::read_to_string(tests_dir.join(file))
            .unwrap_or_else(|e| panic!("ledger meta-test: cannot read {file}: {e}"));
        for name in tests {
            let needle = format!("fn {name}()");
            let pos = src
                .find(&needle)
                .unwrap_or_else(|| panic!("{file}: substitute test `{name}` VANISHED — the ledger entry it backs is vacated; restore or re-record"));
            // No #[ignore] between the preceding #[test] and the fn (window: the 200 bytes
            // before the fn — attributes sit immediately above it).
            let window = &src[pos.saturating_sub(200)..pos];
            assert!(
                !window.contains("#[ignore"),
                "{file}: substitute test `{name}` is #[ignore]d — a silently skipped ledger backing"
            );
        }
    }
}
