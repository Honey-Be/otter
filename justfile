# otter — a reusable, project-agnostic meta-VM framework (register-based IR, tiered execution:
# T0 interpreter / T1 x86-64 baseline JIT / T2 Cranelift JIT / Twasm wasm-emit JIT / A-mode AOT).
# Extracted from the neosurf monorepo at commit 3201f32 (2026-09-04); release channels:
# main = unstable, testing = pre-stable (first cut: v1.0.0-testing.1).

set shell := ["bash", "-uc"]

# list available recipes
default:
    @just --list

# ──────────────────────────── dev tasks (Rust workspace) ────────────────────────────

# build the whole workspace
build:
    cargo build --workspace

# run all tests
# GATE REGISTRY (O-14 A-4): this workspace run IS the CI promotion of the otter-wasmemit AOT gate
# suite (crates/otter-wasmemit/tests/aot.rs) — record==live R1/R2/R3 (§5.2), the AOT⊂JIT 3-way
# differential + fail-closed + refusal-pair gates (§6), and the AotError regressions (§8.4). No
# separate lane exists on purpose (과잉 배선 금지); the anti-silent-skip guard is the
# `a4_gate_registry_tests_exist_and_none_are_ignored` meta-test inside that same file.
test:
    cargo test --workspace

# auto-format
fmt:
    cargo fmt --all

# format check + clippy (deny warnings)
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# verify the core is truly no_std (build for a bare-metal target)
no-std-check:
    cargo build -p otter -p otter-wasmemit --target x86_64-unknown-none

# mirror CI locally (lint + test + no_std)
ci: lint test no-std-check
    @echo "✓ local CI passed"

# ──────────────────── O-13 wasm-emit deployment build config (W-4, design §7) ────────────────────
# Pin bump procedure (which gates to re-run, where every copy of the pin lives): design doc
# .claude-research-library/otter-o13-wasm-emit-design.md §7.1 — bump THERE first, then here.

# W-0-measured nightly pin (M2 green: rustc 1.100.0-nightly, LLVM 23.1.0, exnref default).
wasm-nightly := "nightly-2026-08-31"

# §7 confirmed guest RUSTFLAGS — the INTEGRATED configuration (W-0 disposition C#1: EH flags and
# the §5.3 embedder link flags together in one artifact, as deployment will combine them):
#   -Cpanic=unwind                          EH on (with -Zbuild-std=std,panic_unwind)
#   -Cllvm-args=-wasm-use-legacy-eh=false   exnref pin — a no-op on the current pin (LLVM 23 default),
#                                           kept as a defensive pin; §7.1 re-evaluates it on bump
#   --export-table --growable-table         §5.3 embedder link requirement (shared funcref table)
wasm-eh-rustflags := "-Cpanic=unwind -Cllvm-args=-wasm-use-legacy-eh=false -C link-arg=--export-table -C link-arg=--growable-table"

# O-17 §7 ATOMICS recipe (shared linear memory) — the spike-measured "V4" configuration, verbatim
# (design note: .claude-research-library/otter-o17-shared-memory-l0.md §3). On top of the §7 flags:
#   -Ctarget-feature=+atomics,-bulk-memory,+mutable-globals
#       +atomics = real atomic instructions AND the engine's compile-time self-knowledge signal
#       (otter-engine keys the shared memory-import shape on cfg!(target_feature = "atomics"));
#       -bulk-memory is LOAD-BEARING, not an optimization: atomics∧bulk-memory makes LLVM emit
#       real TLS relocations for std's `#[thread_local] errno`, which clash with the prebuilt
#       single-threaded wasi-libc's .bss errno (R_WASM_MEMORY_ADDR_TLS_SLEB link error) — turning
#       bulk-memory OFF disables the TLS lowering while keeping atomics (spike M1).
#   -C link-arg=--shared-memory -C link-arg=--max-memory=1073741824
#       shared memories must declare a max; 1 GiB (16384 pages) is the measured spike value.
#   -C link-arg=--no-check-features
#       required: rust-std's PREBUILT wasi-libc objects (and Rust's own cgu objects) don't carry
#       the atomics/shared feature flags, so lld's per-object feature check rejects the link.
#       This disables an lld safety net — sound under L0's single-agent norm (no concurrent
#       execution exists), pinned as a §7.1-style decision in the O-17 design note §3.1.
wasm-atomics-rustflags := wasm-eh-rustflags + " -Ctarget-feature=+atomics,-bulk-memory,+mutable-globals -C link-arg=--shared-memory -C link-arg=--max-memory=1073741824 -C link-arg=--no-check-features"

# BOTH the rust-src component (build-std) AND the wasm32-wasip1 target (self-contained wasi-libc
# rides in rust-std) are required — omitting the target fails much later with EH-unrelated-looking
# link errors (crt1-command.o / -lc / -lunwind).
# install the §7 toolchain prerequisites (idempotent): nightly pin + rust-src + wasip1 target
wasm-toolchain toolchain=wasm-nightly:
    # skip the rustup network round trip when the toolchain is already present (review A#4: keeps
    # offline local re-runs working; `rustup toolchain install` is otherwise idempotent anyway)
    rustup toolchain list | grep -q "^{{toolchain}}" || rustup toolchain install {{toolchain}} --profile minimal --component rust-src --target wasm32-wasip1
    rustup component add --toolchain {{toolchain}} rust-src 2>/dev/null || true
    rustup target add --toolchain {{toolchain}} wasm32-wasip1 2>/dev/null || true

# Reuses the committed W-0 spike artifacts verbatim (spikes/o13-w0-jit-contract/, no source edits);
# runs on the wasmtime-45 host with Config::wasm_exceptions(true).
# W-4 gate 1: W-0 M2 smoke in one command — in-wasm catch_unwind must catch under the §7 config
wasm-eh-smoke toolchain=wasm-nightly: (wasm-toolchain toolchain)
    cd spikes/o13-w0-jit-contract/guest-eh && RUSTFLAGS="{{wasm-eh-rustflags}}" cargo +{{toolchain}} build --locked --release --target wasm32-wasip1 -Zbuild-std=std,panic_unwind
    # pipefail (review B#1): without it the pipeline's exit code is grep's alone, so a host that
    # prints M2-GUEST-OK and THEN fails (post-marker assertion, non-zero exit) would pass silently.
    set -o pipefail; cd spikes/o13-w0-jit-contract/host && cargo run --locked --release -- ../guest-eh/target/wasm32-wasip1/release/guest-eh.wasm --eh | tee /dev/stderr | grep -q "M2-GUEST-OK"
    @echo "✓ wasm-eh-smoke ({{toolchain}}): in-wasm catch_unwind caught + post-catch execution OK (M2-GUEST-OK)"

# `-C panic=unwind` mirrors the §7 config so the O-12 compile_error! gate passes for check/clippy
# (no link happens here; the real EH link is wasm-eh-smoke / wasm-demo-build).
# W-4 gate 2: the W-3 cross-checks — otter-engine wasm-emit check + clippy -D warnings on wasip1
wasm-cross-check:
    RUSTFLAGS="-C panic=unwind" cargo check -p otter-engine --features wasm-emit --target wasm32-wasip1
    RUSTFLAGS="-C panic=unwind" cargo clippy -p otter-engine --features wasm-emit --target wasm32-wasip1 -- -D warnings
    @echo "✓ wasm-cross-check: check + clippy -D warnings green on wasm32-wasip1 (wasm-emit on)"

# Deployment-representative release profile (C#7) rides in the demo's own Cargo.toml. Explicit
# `cargo +<toolchain>` (not a per-directory rust-toolchain.toml): demos/otter-wasm-demo/README.md
# owns the rationale.
# W-4 gate 3 (C#7/C#8): workspace-excluded demo under full §7 config — final link must pass
wasm-demo-build toolchain=wasm-nightly: (wasm-toolchain toolchain)
    cd demos/otter-wasm-demo && RUSTFLAGS="{{wasm-eh-rustflags}}" cargo +{{toolchain}} build --locked --release --target wasm32-wasip1 -Zbuild-std=std,panic_unwind --features wasm-emit
    @echo "✓ wasm-demo-build ({{toolchain}}): otter-wasm-demo linked under the full §7 configuration"

# ──────────────────── O-19 C4: wasm64 T0 compile canary (design otter-o19-wasm64-design.md §3.1) ────────────────────
# The (b) gate of the O-19 triple — the S1-M1 MINIMAL shape verbatim: the otter library
# (no_std+alloc, half 2.7.1 included) compiled for wasm64-unknown-unknown via
# -Zbuild-std=core,alloc on the §7 pinned nightly. S1 measured this green with ZERO source
# edits; the recipe pins that fact. Pin value canon: the `wasm-nightly` variable above (this
# lane is now an O-13 §7.1 bump-procedure re-confirmation item — a Tier-3 target can break at
# any pin bump with no upstream notice, O-19 §8-2; a red here at a bump is the designed
# tripwire, not a lane to silence).
# VERIFICATION LEVEL (recorded — design §1.2(b)/§9-2): this is a COMPILE canary. Execution of
# rustc-produced wasm64 code is evidenced by the C0-persisted S1 record
# (.claude-research-library/otter-o19-spike-results.md), NOT re-run in CI — the always-on
# guest-std execution lane was evaluated and REJECTED (§9-2: nightly+build-std execution cost
# vs gate value; gate (c) execution lives in `wasm-aot-e2e`'s a64-diff lane instead).
# Guest/unwind shapes stay out on purpose (§3.1 [결정]: the gate must not hang on
# document-stale surfaces — core::arch::wasm64 is wholly unstable, unwind is pin-measured only).
# Dedicated target dir (the O-12 isolated-target-dir pattern): the wasm64 build-std flavor
# never touches the workspace's incremental state; the CI step removes the dir post-run so the
# rust-cache rows never grow a wasm64 flavor (§8-6 disk discipline — the S1 deviation log
# records a tmpfs saturation from exactly this class of target bloat).
# O-19 C4 gate: otter (no_std+alloc) must COMPILE for wasm64-unknown-unknown on the pinned nightly
wasm64-smoke toolchain=wasm-nightly: (wasm-toolchain toolchain)
    cargo +{{toolchain}} build --locked -p otter --target wasm64-unknown-unknown -Zbuild-std=core,alloc --target-dir target/wasm64-smoke
    @echo "✓ wasm64-smoke ({{toolchain}}): otter (no_std+alloc) compiles for wasm64-unknown-unknown (-Zbuild-std=core,alloc, source unmodified)"

# W-5 gate (design §10 W-5): the FULL §8(c) acceptance run — reference host (§5.5,
# hosts/otter-host-wasmtime, stable) + demo guest in both feature configs, three executions:
#   1. normal      → acceptance 1/2/3/4/7 + §8(b) markers, and HOST-COMPILE-REQUESTS ==
#                    HOST-COMPILE-COUNT == the guest's own E2E-EXPECT-COMPILES (acceptance 5's
#                    normal-side "exactly one compile per promoted function")
#   2. --reject-compiles → E2E-5 marker + HOST-COMPILE-COUNT=0 (nothing compiled) +
#                    HOST-COMPILE-REQUESTS == E2E-EXPECT-COMPILE-REQUESTS (sticky refusal ⇒
#                    exactly one request per hot function, no retry loop — acceptance 5)
#   3. --no-otter-jit + feature-OFF guest → E2E-6 marker (acceptance 6, O-7 preservation)
# The feature-on/off guests share a target dir, so each artifact is copied aside post-build.
# W-5 gate: full §8(c) E2E acceptance — reference host + demo guest, normal/reject/feature-off runs
wasm-e2e toolchain=wasm-nightly: (wasm-toolchain toolchain)
    #!/usr/bin/env bash
    set -euo pipefail
    demo=demos/otter-wasm-demo/target/wasm32-wasip1/release
    host=hosts/otter-host-wasmtime/target/release/otter-host-wasmtime
    # reference host (repo-pinned stable via rust-toolchain.toml; locked)
    (cd hosts/otter-host-wasmtime && cargo build --locked --release)
    # panel #3 C#2: the two workspace-EXCLUDED crates get no gate from `just lint`/`just test` —
    # gate them here (CI's wasm lanes call this recipe, so this covers CI automatically). The
    # host's `cargo test` runs the §5.2 section-whitelist unit tests (panel #3 lens B#1/B#2 fix).
    (cd hosts/otter-host-wasmtime && cargo fmt --check && cargo clippy --locked -- -D warnings && cargo test --locked)
    (cd demos/otter-wasm-demo && cargo fmt --check)
    # feature-ON guest: reuse the W-4 recipe verbatim, then copy aside
    just wasm-demo-build {{toolchain}}
    cp "$demo/otter-wasm-demo.wasm" "$demo/otter-wasm-demo-emit.wasm"
    # feature-OFF guest (acceptance 6): same §7 pipeline, minus --features wasm-emit
    (cd demos/otter-wasm-demo && RUSTFLAGS="{{wasm-eh-rustflags}}" cargo +{{toolchain}} build --locked --release --target wasm32-wasip1 -Zbuild-std=std,panic_unwind)
    cp "$demo/otter-wasm-demo.wasm" "$demo/otter-wasm-demo-t0.wasm"
    # ---- run 1: normal (acceptance 1/2/3/4/7 + 8b + compile accounting) ----
    out="$demo/e2e-tierup.log"
    "$host" "$demo/otter-wasm-demo-emit.wasm" tierup | tee "$out"
    for m in E2E-1-BUILD-OK E2E-2-TIERUP-OK E2E-3-ORACLE-OK E2E-4-TRAP-OK E2E-7-GROW-OK E2E-8B-CROSSTIER-OK E2E-ALL-OK; do grep -q "$m" "$out" || { echo "wasm-e2e: missing marker $m"; exit 1; }; done
    exp=$(grep -oE 'E2E-EXPECT-COMPILES=[0-9]+' "$out" | cut -d= -f2)
    req=$(grep -oE 'HOST-COMPILE-REQUESTS=[0-9]+' "$out" | cut -d= -f2)
    cnt=$(grep -oE 'HOST-COMPILE-COUNT=[0-9]+' "$out" | cut -d= -f2)
    [ -n "$exp" ] && [ "$req" = "$exp" ] && [ "$cnt" = "$exp" ] || { echo "wasm-e2e: normal-run compile accounting mismatch (expect=$exp requests=$req count=$cnt)"; exit 1; }
    # ---- run 2: --reject-compiles (acceptance 5) ----
    out="$demo/e2e-reject.log"
    "$host" --reject-compiles "$demo/otter-wasm-demo-emit.wasm" reject | tee "$out"
    for m in E2E-1-BUILD-OK E2E-5-REJECT-OK E2E-ALL-OK; do grep -q "$m" "$out" || { echo "wasm-e2e: missing marker $m"; exit 1; }; done
    grep -q '^HOST-COMPILE-COUNT=0$' "$out" || { echo "wasm-e2e: reject run compiled something (HOST-COMPILE-COUNT != 0)"; exit 1; }
    exp=$(grep -oE 'E2E-EXPECT-COMPILE-REQUESTS=[0-9]+' "$out" | cut -d= -f2)
    req=$(grep -oE 'HOST-COMPILE-REQUESTS=[0-9]+' "$out" | cut -d= -f2)
    [ -n "$exp" ] && [ "$req" = "$exp" ] || { echo "wasm-e2e: reject-run request accounting mismatch (expect=$exp requests=$req) — retry loop?"; exit 1; }
    # ---- run 3: feature-off guest on an otter_jit-less host (acceptance 6) ----
    out="$demo/e2e-t0.log"
    "$host" --no-otter-jit "$demo/otter-wasm-demo-t0.wasm" t0 | tee "$out"
    for m in E2E-1-BUILD-OK E2E-6-FEATUREOFF-OK E2E-ALL-OK; do grep -q "$m" "$out" || { echo "wasm-e2e: missing marker $m"; exit 1; }; done
    # panel #3 C#3: anchored counter asserts — a feature-off guest on an otter_jit-less host must
    # never request (let alone get) a compile; 0/0 pins the pure-T0 shape, not just the markers.
    grep -q '^HOST-COMPILE-REQUESTS=0$' "$out" || { echo "wasm-e2e: t0 run requested a compile (HOST-COMPILE-REQUESTS != 0)"; exit 1; }
    grep -q '^HOST-COMPILE-COUNT=0$' "$out" || { echo "wasm-e2e: t0 run compiled something (HOST-COMPILE-COUNT != 0)"; exit 1; }
    echo "✓ wasm-e2e ({{toolchain}}): §8(c) acceptance 1-7 + §8(b) cross-tier all green"

# O-14 A-5 gate (design otter-o14-aot-core-design.md §8.2/§9 A-5): the AOT-artifact E2E on the
# HOST-train shape — hosts/otter-aot-e2e (workspace-excluded, wasmtime 48, reference-host features
# + Config verbatim; README.md there has the placement rationale). STABLE-only: the artifact is
# generated at run time by compile_module (single-lowering-source rule — no pre-emitted bytes to
# drift), so no nightly/`wasm-toolchain` prerequisite. Phases/markers: entry (argc=0 v1 contract,
# oracle-identical) / shared-memory link / 2M-deep bounded tail recursion in a 64 KiB wasm stack /
# the §2.7 shared × declared-table × return_call_indirect combination / StackOverflow negative
# control. Same pipefail + --locked + marker-grep discipline as wasm-e2e.
#
# O-19 C3 (design otter-o19-wasm64-design.md §4/§7 C3 — the (c) gate): the `a64-diff` mode rides
# THIS recipe, not a new one (recorded judgment: same binary, same stable-only constraint, same
# host-train cache rows; a separate recipe would rebuild the crate for one extra invocation, and
# CI inherits the lane through the existing `just wasm-aot-e2e` step with zero new wiring).
# Legs: A64 == A32 == native-oracle round trip on explicit wasm_memory64(true) + MemoryType::new64
# host memory / mixed-shape tail leg / 32-bit-host-memory link refusal / explicit
# wasm_memory64(false) rejection (Err-assert, A32 still accepted) / the MF-1 table64 vector
# (go()→7). `cargo test` runs the reference_config no-drift pin (the tech-S1 single-builder
# discipline made machine-checked).
# O-14 A-5 + O-19 C3 gate: AOT artifact E2E (A32 phases + A64 lane) on the wasmtime-48 host shape
wasm-aot-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    bin=hosts/otter-aot-e2e/target/release/otter-aot-e2e
    (cd hosts/otter-aot-e2e && cargo build --locked --release)
    # Workspace-excluded crate ⇒ no gate from `just lint`/`just test` — gate fmt/clippy here
    # (the wasm-e2e recipe's panel #3 C#2 rule, applied to this crate), plus the O-19 C3
    # reference_config no-drift unit test.
    (cd hosts/otter-aot-e2e && cargo fmt --check && cargo clippy --locked --release -- -D warnings)
    (cd hosts/otter-aot-e2e && cargo test --locked --release)
    out=hosts/otter-aot-e2e/target/aot48-e2e.log
    "$bin" | tee "$out"
    for m in AOT48-E2E-1-ENTRY-OK AOT48-E2E-2-SHARED-LINK-OK AOT48-E2E-3-TAILBOUND-OK AOT48-E2E-4-SHARED-TAILBOUND-OK AOT48-E2E-5-NEGCTRL-OK AOT48-E2E-ALL-OK; do grep -q "$m" "$out" || { echo "wasm-aot-e2e: missing marker $m"; exit 1; }; done
    # ---- O-19 C3: the A64 differential lane (gate (c) — design §4 판정문 (ii)-(v)) ----
    out64=hosts/otter-aot-e2e/target/aot48-a64-diff.log
    "$bin" a64-diff | tee "$out64"
    for m in AOT48-E2E-8-A64-DIFF-OK AOT48-E2E-8-A64-MIXED-TAIL-OK AOT48-E2E-8-A64-NEG-MEM32-OK AOT48-E2E-8-A64-NEG-CONFIG-OK AOT48-E2E-8-TABLE64-OK AOT48-E2E-8-ALL-OK; do grep -q "$m" "$out64" || { echo "wasm-aot-e2e: missing marker $m"; exit 1; }; done
    echo "✓ wasm-aot-e2e: O-14 A-5 + O-19 C3 — A32 phases green AND the A64 lane (diff/mixed-tail/negatives/table64) green on the 48 host shape"

# O-16c′ L-1a+L-1b gate — hosts/otter-aot-link unit gates (design §7 L-1a/L-1b rows):
# L-1a — rename mapping-table (import/export modes) + §2-6 post-rename invariance both modes +
# §5.1 residual-import EXACT match (lost/excess/type-mismatch negatives) + §6.1 kebab fail-closed
# rejections ①②③ + rename products wasmparser-Validator green (NO wasmtime here — instantiation
# asserts are L-4's, §0-5).
# L-1b — inject-marker W∈{0,8,≥16} three shapes (§2-1) + constructive min raise + §4.3 input-SHA
# pair seam + check --scratch (i)/(ii)/(iii) each ± (§5.2, panel G-SF-7) + overlap-segment
# injection negative + shared-memory active-marker wasmparser-level fixture + lib-manifest
# extract/finalize (§2-2 order, §8-6 loud fallback) + library-sidecar schema roundtrip.
# L-2 hardening (the L-1b review's carried-over prescriptions, each tested): W==0 ∩-exemption
# (empty-region false-red), memory64 + custom-page-size refusal at the common owned_memory seam
# (all four consumer paths), finalize re-bind refusal branch.
# L-4/L-5 unit suites (l4_gates/l5_gates) + L-6 hardening: the check --linked memory-sharedness
# cross-predicate (artifact == manifest, both mismatch directions + the forged-manifest start
# smuggle), the four sub-branch negatives (imported table / multi-element / imported tag /
# imported-func start), the check --component kind≠func branch, and the §5.6 mutation ledger
# (tests/l6_mutation_ledger.rs — executed-drill records ①③⑥⑦⑧ + permanent-substitute registry
# ②④⑤⑨⑩ pinned by its meta-test).
# O-19 C5 (design otter-o19-wasm64-design.md §2.5/§7 C5 + the C0-C2 review's F2 carry-over):
# tests/o19_c5_gates.rs — the A64-manifest sidecar-bridge refusal (the emit-aot product-path
# seam) + the narrow() over-u32 refusals (entry addr + region base, refuse-never-truncate) +
# the sidecar memory64 #[serde(default)] back-compat contract; l4_gates.rs grows the
# check --linked memory64 cross-predicate (the sharedness predicate's addrtype twin — both
# forged directions red, equality posture recorded, report flag explicit).
# Workspace-excluded crate ⇒ no gate from `just lint`/`just test` — fmt/clippy/test gated here
# (the wasm-aot-e2e recipe's rule, applied to this crate).
wasm-aot-link-check:
    #!/usr/bin/env bash
    set -euo pipefail
    (cd hosts/otter-aot-link && cargo build --locked --release)
    (cd hosts/otter-aot-link && cargo fmt --check && cargo clippy --locked --release --all-targets -- -D warnings)
    (cd hosts/otter-aot-link && cargo test --locked --release)
    echo "✓ wasm-aot-link-check: O-16c′ L-1a+L-1b(+L-2 경화 4건)+L-4/L-5 단위+L-6 경화 — rename/invariance/residual-import=(±)/kebab + inject-marker(W0/W8/W≥16)/check --scratch(3종 ±)/lib-manifest/schema-roundtrip + memory64/page-size/W0-∩/재결부 거부 + linked sharedness-교차(±)/세부 분기 음성 4종/component kind≠func + 뮤테이션 레저 메타-테스트 + O-19 C5(F2 사이드카-브리지·narrow 거부, serde-default 하위호환, linked memory64-교차 ±) all green"

# ──────────────── O-16c′ L-3: pinned external link tools — binaryen(wasm-merge) + wasm-tools ────────────────
# Pin VALUES are canonical HERE (design §3.2: version + SHA-256 justfile variables); the PROCEDURE
# (TOFU establishment, bump steps — full-chain + §5.6 mutation re-run + re-encode drift check —
# and the mirror policy) is canonical in the design doc:
# .claude-research-library/otter-o16c-componentization-design.md §3.2 — bump THERE first, then here.
# Pins cover the x86_64-linux release tarballs ONLY (§3.2 / panel T-N4): other platforms fail
# closed below and re-establish locally per that section.
# TOFU record (established 2026-09-04, O-16c′ L-3 increment):
#   binaryen  — 3-way agreement: b″-spike preserved tarball (scratch o16c-bdblprime/binaryen.tar.gz)
#               ∩ fresh GitHub release download ∩ upstream-published .tar.gz.sha256 release asset.
#   wasm-tools — 2-endpoint agreement (browser_download_url ∩ GitHub API asset endpoint, same
#               release storage — weaker than binaryen's three anchors, recorded honestly):
#               upstream publishes NO checksum asset for v1.246.2, and the spike used the distro
#               SYSTEM binary (same version 1.246.2 — §3.2 provenance-switch note), so no spike
#               tarball anchor exists. Mitigation per §3.2: --version assert + the L-4 full chain
#               re-validates the pinned build end-to-end.
binaryen-version := "version_132"
binaryen-sha256 := "195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572"
wasm-tools-version := "1.246.2"
wasm-tools-sha256 := "f0df9428792225322e9b4344ca581f03b1740d509d6bdad33d14a810009be9ec"

# Cache lives OUTSIDE any scratch/target dir (survives `cargo clean` and reboots; CI caches it
# keyed on the SHA pins). NON-PATH EXPLICIT tool paths: the link chain (L-2 strip/validate, L-4
# merge, L-5 embed/new) must invoke these two variables, never bare `wasm-merge`/`wasm-tools` —
# a system wasm-tools on PATH must never silently substitute for the pin (design §3.2).
wasm-aot-tools-dir := env_var("HOME") + "/.cache/neosurf/wasm-aot-tools"
wasm-merge-bin := wasm-aot-tools-dir + "/binaryen-" + binaryen-version + "/bin/wasm-merge"
wasm-tools-bin := wasm-aot-tools-dir + "/wasm-tools-" + wasm-tools-version + "-x86_64-linux/wasm-tools"

# O-16c′ L-3 (design §3.2): download+verify+unpack the pinned release tarballs, idempotently.
# Offline guard (wasm-toolchain precedent): a verified cache hit does ZERO network I/O. Every run
# (cache hit included) ends with --version asserts on the explicit-path binaries — both a pin
# check and a cache-health check. Hash mismatch is fail-closed: loud error, no unpack, pins +
# re-establishment procedure referenced; the offending tarball is left in place as evidence.
# O-16c′ L-3: procure pinned binaryen(wasm-merge)+wasm-tools — SHA-256 verified, idempotent, non-PATH
wasm-aot-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="{{wasm-aot-tools-dir}}"
    mkdir -p "$dir"
    # x86_64-linux only (design §3.2): never let a wrong-platform binary pass silently.
    if [ "$(uname -sm)" != "Linux x86_64" ]; then
        echo "wasm-aot-tools: pins are for x86_64-linux release tarballs ONLY (this host: $(uname -sm))"
        echo "  re-establish pins locally per .claude-research-library/otter-o16c-componentization-design.md §3.2"
        exit 1
    fi
    fetch_verify_unpack() { # <label> <tarball-basename> <url> <pinned-sha256> <probe-binary>
        local label="$1" tarball="$dir/$2" url="$3" pin="$4" probe="$5"
        local stamp="$dir/.$2.verified"
        if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$pin" ] && [ -x "$probe" ]; then
            echo "wasm-aot-tools: $label — cache hit (already verified against pin), no network"
            return 0
        fi
        if [ ! -f "$tarball" ]; then
            echo "wasm-aot-tools: downloading $label ..."
            curl -fsSL --retry 3 -o "$tarball.part" "$url"
            mv "$tarball.part" "$tarball"
        fi
        local got
        got="$(sha256sum "$tarball" | cut -d' ' -f1)"
        if [ "$got" != "$pin" ]; then
            echo "wasm-aot-tools: SHA-256 MISMATCH for $label — fail-closed, refusing to unpack"
            echo "  file:     $tarball (left in place as evidence)"
            echo "  expected: $pin"
            echo "  actual:   $got"
            echo "  cause: tampered/corrupted download, a silently republished upstream asset, or a"
            echo "  non-x86_64-linux tarball; pin values + re-establishment/bump procedure:"
            echo "  .claude-research-library/otter-o16c-componentization-design.md §3.2"
            exit 1
        fi
        tar xzf "$tarball" -C "$dir"
        printf '%s' "$pin" > "$stamp"
        echo "wasm-aot-tools: $label — sha256 verified + unpacked"
    }
    fetch_verify_unpack "binaryen {{binaryen-version}}" \
        "binaryen-{{binaryen-version}}-x86_64-linux.tar.gz" \
        "https://github.com/WebAssembly/binaryen/releases/download/{{binaryen-version}}/binaryen-{{binaryen-version}}-x86_64-linux.tar.gz" \
        "{{binaryen-sha256}}" "{{wasm-merge-bin}}"
    fetch_verify_unpack "wasm-tools {{wasm-tools-version}}" \
        "wasm-tools-{{wasm-tools-version}}-x86_64-linux.tar.gz" \
        "https://github.com/bytecodealliance/wasm-tools/releases/download/v{{wasm-tools-version}}/wasm-tools-{{wasm-tools-version}}-x86_64-linux.tar.gz" \
        "{{wasm-tools-sha256}}" "{{wasm-tools-bin}}"
    # --version asserts on the EXPLICIT paths (never the PATH tools) — every run, cache hit included.
    "{{wasm-merge-bin}}" --version | grep -qF "({{binaryen-version}})" \
        || { echo "wasm-aot-tools: wasm-merge --version assert FAILED: got '$("{{wasm-merge-bin}}" --version || true)', want '({{binaryen-version}})'"; exit 1; }
    "{{wasm-tools-bin}}" --version | grep -qE "^wasm-tools {{wasm-tools-version}}( |$)" \
        || { echo "wasm-aot-tools: wasm-tools --version assert FAILED: got '$("{{wasm-tools-bin}}" --version || true)', want 'wasm-tools {{wasm-tools-version}}'"; exit 1; }
    echo "✓ wasm-aot-tools: $("{{wasm-merge-bin}}" --version) + $("{{wasm-tools-bin}}" --version) — pinned, verified, at {{wasm-aot-tools-dir}}"

# ──────────────── O-16c′ L-2: b′ productization — hosts/otter-helpers-wasm build + gates ────────────────
# The design §4.2 recipe verbatim, both profiles (non-shared / +atomics), STABLE via plain
# `cargo` (rust-toolchain.toml governs — panel T-S2: no `cargo +1.96.0` second pin authority):
#   cargo build --locked --release --target wasm32-wasip1
#   otter-aot-link lib-manifest --extract <unstripped>       # sidecar stage ① (§2-2): PRE-strip
#   wasm-tools strip                                         # pinned, non-PATH (§3.2)
#   otter-aot-link lib-manifest --finalize <stripped>        # stage ③: binds strip-final SHA-256
#   wasm-tools validate --features=all,-exceptions,-legacy-exceptions   # EH-free gate (verbatim)
# plus, per the §7 L-2 gate row: EH census grep == 0 (both profiles), the panel-M1 defense line
# "sidecar heap_floor == profile expected constant" (non-shared 0x100000=1048576 / +atomics
# 0x110000=1114112 — a formula regression can never green-pass), the B2–B5 replay through
# `otter-aot-e2e bprime-diff` (the spike's 40,000-case differential absorbed there — B2 surface
# (import 0 / export 4종), B3 40k×0-mismatch, B4 panic paths), and the representative-region
# injected TEST artifact (verification-only, NOT a shipping shape — §4.2: injection is a
# per-product L-4 link step; the recipe ships the UNINJECTED master + sidecar only) with its
# §2-1 re-measurement, all profile-split shape figures printed as BPRIME-SHAPE lines.
# +atomics profile: spike-verbatim RUSTFLAGS in a separate target dir (target/atomics — under
# the ignored target/); its "unstable feature … `atomics`" warning is the O-W watchlist entry
# (MODE doc §1.4 row 7 — warning→error flip on a new stable re-opens the shared profile only).
# Workspace-excluded crate ⇒ fmt/clippy gated HERE (the wasm-aot-e2e rule).
# O-16c′ L-2: b′ build+gates — both profiles, sidecars, EH-free, heap_floor consts, B2–B5 replay
wasm-bprime: wasm-aot-tools
    #!/usr/bin/env bash
    set -euo pipefail
    bp=hosts/otter-helpers-wasm
    out=$bp/target/bprime
    mkdir -p "$out"
    # fmt/clippy gates (workspace-excluded ⇒ gate here), then the two profile builds, --locked.
    (cd "$bp" && cargo fmt --check)
    (cd "$bp" && cargo clippy --locked --release --target wasm32-wasip1 -- -D warnings)
    (cd "$bp" && cargo build --locked --release --target wasm32-wasip1)
    (cd "$bp" && RUSTFLAGS="-Ctarget-feature=+atomics,+bulk-memory -Clink-arg=--shared-memory -Clink-arg=--max-memory=4294967296" \
        cargo build --locked --release --target wasm32-wasip1 --target-dir target/atomics)
    # The link driver (lib-manifest / inject-marker / check) — its own gates run in
    # `just wasm-aot-link-check`; here it only needs to exist.
    (cd hosts/otter-aot-link && cargo build --locked --release)
    link=hosts/otter-aot-link/target/release/otter-aot-link
    otter_ver="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
    rustc_ver="$(rustc --version)"
    census() { # EH census (SPK B2 verbatim grep) — must be 0
        # L-2 review fix: print to a temp file FIRST so a print failure is loud (exit under
        # pipefail) instead of being absorbed by the grep-no-match `|| true` (fail-open census=0).
        local wat_tmp
        wat_tmp="$(mktemp)"
        "{{wasm-tools-bin}}" print "$1" > "$wat_tmp"
        grep -cE '\b(try|try_table|catch|catch_all|catch_ref|throw|throw_ref|rethrow|delegate)\b' "$wat_tmp" || true
        rm -f "$wat_tmp"
    }
    shape() { # BPRIME-SHAPE line: size + data-segment count + memory/start decl (§2-1 re-measurement record)
        local label="$1" f="$2" wat
        wat="$("{{wasm-tools-bin}}" print "$f")"
        echo "BPRIME-SHAPE profile=$label size=$(stat -c %s "$f")B data-segs=$(grep -c '^ *(data' <<<"$wat" || true) start=$(grep -c '^ *(start' <<<"$wat" || true) memory=[$(grep -m1 '(memory' <<<"$wat" | sed 's/^ *//;s/ *$//')]"
    }
    process_profile() { # <label> <unstripped> <stripped-out> <sidecar-out> <expected-heap-floor-decimal>
        local label="$1" un="$2" st="$3" sc="$4" floor="$5"
        "$link" lib-manifest --extract "$un" --output "$sc" --otter-version "$otter_ver" --rustc-version "$rustc_ver"
        "{{wasm-tools-bin}}" strip "$un" -o "$st"
        "$link" lib-manifest --finalize "$st" --sidecar "$sc"
        "{{wasm-tools-bin}}" validate --features=all,-exceptions,-legacy-exceptions "$st"
        c=$(census "$st")
        [ "$c" = "0" ] || { echo "wasm-bprime: $label EH census = $c, want 0"; exit 1; }
        # Panel M1 defense line: the sidecar floor must equal the profile's expected constant.
        grep -q "\"heap_floor\": $floor," "$sc" \
            || { echo "wasm-bprime: $label heap_floor != $floor (formula regression?) — sidecar:"; grep heap_floor "$sc"; exit 1; }
        shape "$label" "$st"
    }
    process_profile nonshared "$bp/target/wasm32-wasip1/release/otter_helpers_wasm.wasm" \
        "$out/otter_helpers.wasm" "$out/otter_helpers.manifest.json" 1048576
    process_profile atomics "$bp/target/atomics/wasm32-wasip1/release/otter_helpers_wasm.wasm" \
        "$out/otter_helpers.atomics.wasm" "$out/otter_helpers.atomics.manifest.json" 1114112
    # ---- B2–B5 replay (§7 L-2 gate row): the absorbed 40k differential vs the PRODUCT b′ ----
    (cd hosts/otter-aot-e2e && cargo build --locked --release)
    log="$out/bprime-diff.log"
    hosts/otter-aot-e2e/target/release/otter-aot-e2e bprime-diff "$out/otter_helpers.wasm" | tee "$log"
    for m in BPRIME-B2-SURFACE-OK BPRIME-B3-DIFF-OK BPRIME-B4-PANIC-OK BPRIME-DIFF-ALL-OK; do
        grep -q "$m" "$log" || { echo "wasm-bprime: missing marker $m"; exit 1; }
    done
    # ---- representative-region injected TEST artifact (§2-1 re-measurement; verification-only) ----
    rep="$out/rep-region.manifest.json"
    cat > "$rep" <<'EOF'
    {
      "entry_slot": 0,
      "nfuncs": 1,
      "helper_slots": [1, 2, 3],
      "host_slots": [],
      "imports": [],
      "scratch": [{ "addr": 1114112, "words": 2 }],
      "scratch_region": { "base": 1114112, "end": 1114128 },
      "table_size": 4,
      "shared_memory": false,
      "emitter_version": "0.0.0"
    }
    EOF
    inj="$out/otter_helpers.rep-injected.wasm"
    "$link" inject-marker --input "$out/otter_helpers.wasm" --output "$inj" \
        --manifest "$rep" --lib-manifest "$out/otter_helpers.manifest.json"
    "$link" check --scratch --manifest "$rep" --lib-manifest "$out/otter_helpers.manifest.json" \
        --lib-artifact "$out/otter_helpers.wasm" "$inj"
    "{{wasm-tools-bin}}" validate --features=all,-exceptions,-legacy-exceptions "$inj"
    c=$(census "$inj")
    [ "$c" = "0" ] || { echo "wasm-bprime: rep-injected EH census = $c, want 0"; exit 1; }
    shape rep-injected "$inj"
    echo "✓ wasm-bprime: O-16c′ L-2 — both profiles built (stable, --locked), sidecars 2-stage bound, EH-free, heap_floor==0x100000/0x110000, B2–B5 replay (40k×0), rep-region injection re-measured — all green"

# ──────────────── O-16c′ L-4: the b″ link chain E2E — real b′ × real compile_module products ────────────────
# Design §7 L-4 row + §1.1 chain, verbatim ordering NOTE (recorded): §5.5 words the chain
# "…merge→산물별 주입→check…", but §2-1/§4.2 (and the L-1b implementation's fail-closed input-SHA
# pair seam) fix injection as a PER-PRODUCT step ON THE b′ MASTER — the marker must live in the
# memory-OWNING module BEFORE the merge so that [추정 ①] "주입-후 결합 보존" is a measurable claim
# (SPK §3-3: markers measured as preserved THROUGH wasm-merge). So the chain here is:
#   emit-aot (real products, §5.5 single-lowering-source)
#   → rename (manifest-driven module maps → otter_lib, §2-6)
#   → per-product inject-marker into the profile-matched b′ master (§2-1/§4.2)
#   → assert-instantiate on each injected gate copy (§0-5's L-1→L-4 moved wasmtime assertion;
#     placed in otter-aot-e2e, wasmtime's existing home — otter-aot-link stays wasmtime-free)
#   → wasm-merge (pinned non-PATH path, per-profile feature flags — census below)
#   → check --imports / --scratch (post-merge marker preservation = [추정 ①]'s live run;
#     check (iii)'s FIRST live run against the real b′ sidecars, §2-5-②) / --linked (§5.3 —
#     start-check first live run: the shared artifact carries the init-only __wasm_init_memory)
#   → EH-free validate + WAT census (§2-5-①: the linked artifact, the §7 nightly-escape claim's
#     final-artifact unit) → shrink-exports (§5.4 keep list = otter_entry+otter_table+memory)
#   → post-shrink re-check + re-validate + EXECUTION differentials (§5.4 재실행):
#     AOT48-E2E-6 three shapes oracle-identical + deep 2M bounded + the 40k corpus once through
#     the (pre-shrink, export-union) self-contained linked artifact (§5.3 / panel G-N-3).
#
# WASM-MERGE FEATURE-FLAG CENSUS (§2-5-④, measured 2026-09-04 on the real b′ chain; the
# necessity probes below re-verify EVERY entry on EVERY run — a redundant or missing flag is red):
#   --enable-nontrapping-float-to-int  required by EVERY merge — the real b′ convert path uses
#                                      i32.trunc_sat_f64_s (otter::UnOp::eval); SPK §4-10's
#                                      "실물 Rust convert 경로에도 필요한지" → YES, confirmed.
#   --enable-tail-call                 required where the PRODUCT carries TailCall (deep's
#                                      return_call_indirect); calc/calc_shared merge fine
#                                      without it — kept in the chain-standard set (union, not
#                                      per-product-minimal; recorded honestly).
#   --enable-threads                   shared profile only (shared memory declarations).
#   --enable-bulk-memory               shared profile only — NEW vs the spike set (SPK §2 had
#                                      no bulk-memory: its stand-in b′ was hand-WAT with no
#                                      passive segments); the REAL +atomics b′ carries passive
#                                      $.rodata + memory.init/data.drop in __wasm_init_memory.
# pipefail + --locked + non-PATH pinned tools ({{wasm-merge-bin}}/{{wasm-tools-bin}}) verbatim.
# O-16c′ L-4: link-chain E2E — rename→inject→merge→checks→shrink→run, 3 shapes oracle-identical
wasm-aot-link-e2e: wasm-bprime
    #!/usr/bin/env bash
    set -euo pipefail
    bp=hosts/otter-helpers-wasm/target/bprime
    out=hosts/otter-aot-e2e/target/link-e2e
    rm -rf "$out" && mkdir -p "$out"
    (cd hosts/otter-aot-link && cargo build --locked --release)
    (cd hosts/otter-aot-e2e && cargo build --locked --release)
    link=hosts/otter-aot-link/target/release/otter-aot-link
    e2e=hosts/otter-aot-e2e/target/release/otter-aot-e2e
    log="$out/link-e2e.log"; : > "$log"
    census() { # EH census (wasm-bprime verbatim): WAT grep must be 0 — loud print failure under pipefail
        local wat_tmp; wat_tmp="$(mktemp)"
        "{{wasm-tools-bin}}" print "$1" > "$wat_tmp"
        grep -cE '\b(try|try_table|catch|catch_all|catch_ref|throw|throw_ref|rethrow|delegate)\b' "$wat_tmp" || true
        rm -f "$wat_tmp"
    }
    # ---- real products (single-lowering-source: generated NOW by the live emitter) ----
    "$e2e" emit-aot "$out" | tee -a "$log"
    # ---- §2-5-④ necessity probes: every recorded flag must be REQUIRED where claimed ----
    flag_probe() { # <label> <expect:ok|fatal> <args...>
        local label="$1" expect="$2"; shift 2
        if "{{wasm-merge-bin}}" "$@" -o /dev/null >/dev/null 2>&1; then
            [ "$expect" = ok ] || { echo "wasm-aot-link-e2e: flag probe '$label' UNEXPECTEDLY green (recorded census stale)"; exit 1; }
        else
            [ "$expect" = fatal ] || { echo "wasm-aot-link-e2e: flag probe '$label' UNEXPECTEDLY fatal"; exit 1; }
        fi
        echo "FEATURE-FLAG-PROBE $label expect=$expect ok" | tee -a "$log"
    }
    chain() { # <name> <master> <sidecar> <run-shape> <check-imports-extra> <merge flags...>
        local name="$1" master="$2" sidecar="$3" shape="$4" imports_extra="$5"; shift 5
        local man="$out/$name.manifest.json"
        # rename: manifest-driven module maps (imports 정본 — a stale derivation fails loudly
        # downstream: unmatched key in rename, or unrenamed residual in check --imports)
        local maps=(--map-module otter=otter_lib)
        grep -q '"module": "otter_helpers"' "$man" && maps+=(--map-module otter_helpers=otter_lib)
        "$link" rename --mode import --input "$out/$name.wasm" --output "$out/$name.renamed.wasm" "${maps[@]}" | tee -a "$log"
        # per-product marker injection into the profile-matched b′ master (§2-1/§4.2)
        "$link" inject-marker --input "$master" --output "$out/bprime.$name.wasm" \
            --manifest "$man" --lib-manifest "$sidecar" | tee -a "$log"
        # §0-5 moved wasmtime instantiation assertion (shared run = [추정 ④] engine acceptance)
        "$e2e" assert-instantiate "$out/bprime.$name.wasm" | tee -a "$log"
        # the ONE external tool: pinned wasm-merge, injected b′ registered as otter_lib
        "{{wasm-merge-bin}}" "$@" "$out/bprime.$name.wasm" otter_lib "$out/$name.renamed.wasm" main \
            -o "$out/$name.linked.wasm"
        # b″ self-gates (wasm-merge silently part-links — SPK §4-1: gates are OURS)
        # shellcheck disable=SC2086
        "$link" check --imports $imports_extra "$man" "$out/$name.linked.wasm" | tee -a "$log"
        "$link" check --scratch --manifest "$man" --lib-manifest "$sidecar" \
            --lib-artifact "$master" "$out/$name.linked.wasm" | tee -a "$log"
        "$link" check --linked --manifest "$man" "$out/$name.linked.wasm" | tee -a "$log"
        # §2-5-① EH-free, final-artifact unit (validate + census, both)
        "{{wasm-tools-bin}}" validate --features=all,-exceptions,-legacy-exceptions "$out/$name.linked.wasm"
        local c; c=$(census "$out/$name.linked.wasm")
        [ "$c" = "0" ] || { echo "wasm-aot-link-e2e: $name.linked EH census = $c, want 0"; exit 1; }
        # §5.4 shrink to the O-14 §2.4 surface + memory, then re-check + re-validate
        "$link" shrink-exports --input "$out/$name.linked.wasm" --output "$out/$name.final.wasm" \
            --keep memory --keep otter_entry --keep otter_table | tee -a "$log"
        "$link" check --linked --manifest "$man" "$out/$name.final.wasm" | tee -a "$log"
        "{{wasm-tools-bin}}" validate --features=all,-exceptions,-legacy-exceptions "$out/$name.final.wasm"
        c=$(census "$out/$name.final.wasm")
        [ "$c" = "0" ] || { echo "wasm-aot-link-e2e: $name.final EH census = $c, want 0"; exit 1; }
        # §5.4 post-shrink execution differential (the shrink must not change meaning)
        "$e2e" run-linked "$shape" "$out/$name.final.wasm" | tee -a "$log"
    }
    nonshared_flags=(--enable-tail-call --enable-nontrapping-float-to-int)
    shared_flags=(--enable-tail-call --enable-nontrapping-float-to-int --enable-threads --enable-bulk-memory)
    chain calc        "$bp/otter_helpers.wasm"         "$bp/otter_helpers.manifest.json"         calc        ""                        "${nonshared_flags[@]}"
    chain deep        "$bp/otter_helpers.wasm"         "$bp/otter_helpers.manifest.json"         deep        "--require-self-contained" "${nonshared_flags[@]}"
    chain calc_shared "$bp/otter_helpers.atomics.wasm" "$bp/otter_helpers.atomics.manifest.json" calc-shared ""                        "${shared_flags[@]}"
    # flag necessity (census above): drop-one → fatal where required; the calc/calc_shared
    # tail-call union non-minimality is asserted too, so census drift can never pass silently
    flag_probe "nontrapping-required(calc)"      fatal --enable-tail-call "$out/bprime.calc.wasm" otter_lib "$out/calc.renamed.wasm" main
    flag_probe "tailcall-required(deep)"         fatal --enable-nontrapping-float-to-int "$out/bprime.deep.wasm" otter_lib "$out/deep.renamed.wasm" main
    flag_probe "threads-required(shared)"        fatal --enable-tail-call --enable-nontrapping-float-to-int --enable-bulk-memory "$out/bprime.calc_shared.wasm" otter_lib "$out/calc_shared.renamed.wasm" main
    flag_probe "bulk-memory-required(shared)"    fatal --enable-tail-call --enable-nontrapping-float-to-int --enable-threads "$out/bprime.calc_shared.wasm" otter_lib "$out/calc_shared.renamed.wasm" main
    flag_probe "tailcall-union-nonminimal(calc)" ok    --enable-nontrapping-float-to-int "$out/bprime.calc.wasm" otter_lib "$out/calc.renamed.wasm" main
    # ---- §5.3 / panel G-N-3: the 40k corpus ONCE through the linked artifact (pre-shrink
    # export union of the self-contained deep product — helper exports still reachable) ----
    "$e2e" linked-diff "$out/deep.linked.wasm" | tee -a "$log"
    # ---- §2-5-② negative probe: a base=0x10000 violation manifest must go RED on (iii) ----
    cat > "$out/violation.manifest.json" <<'EOF'
    {
      "entry_slot": 0, "nfuncs": 1, "helper_slots": [1, 2, 3], "host_slots": [],
      "imports": [], "scratch": [{ "addr": 65536, "words": 1 }],
      "scratch_region": { "base": 65536, "end": 65544 },
      "table_size": 4, "shared_memory": false, "emitter_version": "0.0.0"
    }
    EOF
    if "$link" check --scratch --manifest "$out/violation.manifest.json" \
        --lib-manifest "$bp/otter_helpers.manifest.json" --lib-artifact "$bp/otter_helpers.wasm" \
        "$out/calc.linked.wasm" 2> "$out/violation.err"; then
        echo "wasm-aot-link-e2e: violation manifest (base=0x10000) UNEXPECTEDLY green"; exit 1
    fi
    grep -q '(iii)' "$out/violation.err" || { echo "wasm-aot-link-e2e: violation red did not fire check (iii)"; cat "$out/violation.err"; exit 1; }
    echo "NEGPROBE-SCRATCH-III-OK base=0x10000 red with (iii)" | tee -a "$log"
    # ---- marker gate ----
    for m in EMIT-AOT-OK \
             AOT48-E2E-6-LINKED-CALC-OK AOT48-E2E-6-LINKED-DEEP-OK AOT48-E2E-6-LINKED-SHARED-OK \
             LINKED-DIFF-B3-OK LINKED-DIFF-B4-OK LINKED-DIFF-ALL-OK NEGPROBE-SCRATCH-III-OK; do
        grep -q "$m" "$log" || { echo "wasm-aot-link-e2e: missing marker $m"; exit 1; }
    done
    [ "$(grep -c 'INSTANTIATE-OK' "$log")" = "3" ] || { echo "wasm-aot-link-e2e: want 3 INSTANTIATE-OK markers"; exit 1; }
    [ "$(grep -c 'init-only start' "$log")" -ge 1 ] || { echo "wasm-aot-link-e2e: shared start-check live run missing"; exit 1; }
    echo "AOT48-E2E-6-LINKED-OK" | tee -a "$log"
    echo "✓ wasm-aot-link-e2e: O-16c′ L-4 — 3 shapes linked+gated+shrunk, oracle-identical (calc/shared) + 2M bounded (deep), EH-free final artifacts, check(iii)+start-check live, flag census verified, 40k linked-diff green"

# ──────────────── O-16c′ L-5: componentization — embed/new chain + host-lift + §6.2 gates ────────────────
# Design §7 L-5 row + §6.1/§6.2, on top of the L-4 chain's FINAL (shrunk) artifacts — the §1.1
# chain order verbatim: … export 축소 → component embed/new. Per shape:
#   kebab export rename (L-1a rename --mode export --kebab: otter_entry→otter-entry,
#     otter_table→otter-table, memory unchanged — the non-entry exports stay CORE exports and
#     are NOT world-lifted, §6.1 disposition recorded in wit/aot-artifact.wit)
#   → [calc only] HOST-LIFT import rename ([추정]⑤ RESOLVED by the L-5 measurement, pinned
#     wasm-tools 1.246.2: world-level function imports bind to core imports under the module
#     string `$root` with the kebab field name — otter_host.h_add7 → $root.h-add7 etc.; the
#     general mapping-table rename absorbs the convention with zero tool rework, §6.1)
#   → component embed --world (canonical wit/: aot-artifact for host-free shapes,
#     aot-artifact-host for the calc shape) → component new (both pinned, non-PATH)
#   → otter-aot-link check --component (§6.2 NATIVE checks: EH-free-feature validation,
#     empty-world detection, world-shape exact match vs the manifest's otter_host kebab image,
#     nested-core-module EH census)
#   → pinned CLI validate --features=all,-exceptions,-legacy-exceptions (measured green on
#     components — the L-4 EH-free line verbatim) + WAT EH census grep (recipe parity)
#   → otter-aot-e2e run-component (execution differential: calc = HOST-WIRED oracle
#     bit-identity ([추정]② execution leg), deep = 2M bounded INSIDE the component, 64 KiB).
# Negative probes: E10 재현 — `component new` WITHOUT embed on the self-contained deep final
# artifact yields an empty world (exit 0!), which check --component must catch RED; E11 재현 —
# `component new` on the PRE-LINK product must fail (chain order is recipe-enforced, §6.2).
# Shapes: calc + deep only — the shared shape stays core-linked (a shared memory inside a
# component wrapper is outside the design's gate set; §6.1 component entry contract is argc=0).
# O-16c′ L-5: componentization E2E — kebab→lift→embed/new→§6.2 checks→component runs + negatives
wasm-aot-component: wasm-aot-link-e2e
    #!/usr/bin/env bash
    set -euo pipefail
    out=hosts/otter-aot-e2e/target/link-e2e
    wit=hosts/otter-aot-link/wit
    link=hosts/otter-aot-link/target/release/otter-aot-link
    e2e=hosts/otter-aot-e2e/target/release/otter-aot-e2e
    log="$out/component-e2e.log"; : > "$log"
    census() { # EH census (wasm-aot-link-e2e verbatim; wasm-tools print handles components)
        local wat_tmp; wat_tmp="$(mktemp)"
        "{{wasm-tools-bin}}" print "$1" > "$wat_tmp"
        grep -cE '\b(try|try_table|catch|catch_all|catch_ref|throw|throw_ref|rethrow|delegate)\b' "$wat_tmp" || true
        rm -f "$wat_tmp"
    }
    componentize() { # <name> <world> <run-shape> [host-lift]
        local name="$1" world="$2" shape="$3" lift="${4:-}"
        # kebab export rename (§6.1) — rename self-asserts export-mode invariance + Validator
        "$link" rename --mode export --kebab --input "$out/$name.final.wasm" \
            --output "$out/$name.kebab.wasm" | tee -a "$log"
        local lifted="$out/$name.kebab.wasm"
        if [ -n "$lift" ]; then
            # host-lift import rename (§2-4-3): otter_host.* → $root.<kebab> (measured convention)
            lifted="$out/$name.lifted.wasm"
            "$link" rename --mode import --input "$out/$name.kebab.wasm" --output "$lifted" \
                --map-module 'otter_host=$root' \
                --map-name h_add7=h-add7 --map-name hc_xor=hc-xor | tee -a "$log"
        fi
        # embed --world + new (§6.2: NEVER component new directly — the E10 empty-world trap)
        "{{wasm-tools-bin}}" component embed "$wit" --world "$world" "$lifted" -o "$out/$name.embedded.wasm"
        "{{wasm-tools-bin}}" component new "$out/$name.embedded.wasm" -o "$out/$name.component.wasm"
        # §6.2 native checks (wasmparser component-model): empty-world, world shape, EH census
        "$link" check --component --manifest "$out/$name.manifest.json" "$out/$name.component.wasm" | tee -a "$log"
        # CLI EH-free validate (the L-4 line verbatim — measured to accept components) + census
        "{{wasm-tools-bin}}" validate --features=all,-exceptions,-legacy-exceptions "$out/$name.component.wasm"
        local c; c=$(census "$out/$name.component.wasm")
        [ "$c" = "0" ] || { echo "wasm-aot-component: $name.component EH census = $c, want 0"; exit 1; }
        # execution differential (host wiring for calc — the [추정]② execution leg)
        "$e2e" run-component "$shape" "$out/$name.component.wasm" | tee -a "$log"
    }
    componentize calc aot-artifact-host calc host-lift
    componentize deep aot-artifact      deep
    # ---- negative probe 1 (E10 재현): new-without-embed = empty world; §6.2 check must be RED
    "{{wasm-tools-bin}}" component new "$out/deep.final.wasm" -o "$out/deep.noworld.component.wasm"
    if "$link" check --component --manifest "$out/deep.manifest.json" \
        "$out/deep.noworld.component.wasm" 2> "$out/noworld.err"; then
        echo "wasm-aot-component: empty-world component UNEXPECTEDLY green"; exit 1
    fi
    grep -q 'EMPTY WORLD' "$out/noworld.err" || { echo "wasm-aot-component: empty-world red did not name the E10 pitfall"; cat "$out/noworld.err"; exit 1; }
    echo "NEGPROBE-EMPTY-WORLD-OK new-without-embed red via check --component" | tee -a "$log"
    # ---- negative probe 2 (E11 재현): componentizing the PRE-LINK product must fail loudly.
    # L-6 (the L-5 review's carried-over note): qualify the red — a nonzero exit alone could be
    # any unrelated failure (bad path, broken pin); the E11 pitfall is specifically the bare
    # core import that `component new` cannot resolve, so the stderr must NAME it.
    if "{{wasm-tools-bin}}" component new "$out/calc.wasm" -o /dev/null 2> "$out/prelink.err"; then
        echo "wasm-aot-component: component new on the PRE-LINK product UNEXPECTEDLY green"; exit 1
    fi
    grep -q 'failed to resolve import' "$out/prelink.err" \
        || { echo "wasm-aot-component: pre-link red did not name the unresolved bare core import (E11)"; cat "$out/prelink.err"; exit 1; }
    echo "NEGPROBE-PRELINK-COMPONENT-OK bare core imports refused by component new" | tee -a "$log"
    # ---- marker gate ----
    for m in AOT48-E2E-7-COMPONENT-CALC-OK AOT48-E2E-7-COMPONENT-DEEP-OK \
             NEGPROBE-EMPTY-WORLD-OK NEGPROBE-PRELINK-COMPONENT-OK; do
        grep -q "$m" "$log" || { echo "wasm-aot-component: missing marker $m"; exit 1; }
    done
    grep -q 'host-wired=h-add7,hc-xor' "$log" || { echo "wasm-aot-component: host-lift wiring marker missing"; exit 1; }
    echo "AOT48-E2E-7-COMPONENT-OK" | tee -a "$log"
    echo "✓ wasm-aot-component: O-16c′ L-5 — host-lift SUCCEEDED (\$root convention), calc component host-wired oracle-identical + deep 2M bounded in-component, §6.2 native checks + EH-free + empty-world/pre-link negatives all green"

# O-17 smoke gate (design note otter-o17-shared-memory-l0.md §5): the atomics/shared-memory
# variant of the demo guest (wasm-atomics-rustflags — the spike-measured V4 recipe) through the
# SAME reference host. Proves the full L0 passive-compat chain: +atomics guest links & runs →
# engine emits the SHARED memory-import shape (cfg!(target_feature = "atomics")) → host accepts
# it (threads feature + Config::shared_memory(true) + Extern::SharedMemory branches) → tier-up,
# oracle bit-identity and in-wasm catch_unwind (exnref × atomics coexistence) all hold. The
# HOST-SHARED-MEMORY stderr marker pins that the guest memory really was shared (a silently
# non-shared rebuild would tier up too — without the marker the gate could go green vacuously).
# Cache/coexistence note: this build shares the demo's target dir with wasm-e2e's builds — cargo
# keys units by RUSTFLAGS (measured in the O-17 spike target dir: per-crate fingerprint dirs per
# flag set), so the two flavors coexist without invalidating each other.
# O-17 gate: atomics(+shared linear memory) guest E2E — build, run, and grep the core markers
wasm-e2e-atomics toolchain=wasm-nightly: (wasm-toolchain toolchain)
    #!/usr/bin/env bash
    set -euo pipefail
    demo=demos/otter-wasm-demo/target/wasm32-wasip1/release
    host=hosts/otter-host-wasmtime/target/release/otter-host-wasmtime
    (cd hosts/otter-host-wasmtime && cargo build --locked --release)
    # feature-ON guest under the §7 atomics recipe (V4). --locked: same lockfile as wasm-e2e.
    (cd demos/otter-wasm-demo && RUSTFLAGS="{{wasm-atomics-rustflags}}" cargo +{{toolchain}} build --locked --release --target wasm32-wasip1 -Zbuild-std=std,panic_unwind --features wasm-emit)
    cp "$demo/otter-wasm-demo.wasm" "$demo/otter-wasm-demo-atomics.wasm"
    out="$demo/e2e-atomics.log"
    # 2>&1: the HOST-SHARED-MEMORY marker is a host stderr diagnostic; everything else is stdout.
    # Guest mode is `tierup` (the demo's normal-run mode — same §8(c) run-1 semantics as wasm-e2e).
    "$host" "$demo/otter-wasm-demo-atomics.wasm" tierup 2>&1 | tee "$out"
    # Core markers (O-17 acceptance): boot / tier-up / oracle bit-identity / in-wasm catch_unwind
    # (exnref × atomics coexistence) / the full-run seal / the shared-memory proof.
    for m in E2E-1-BUILD-OK E2E-2-TIERUP-OK E2E-3-ORACLE-OK E2E-4-TRAP-OK E2E-7-GROW-OK E2E-8B-CROSSTIER-OK E2E-ALL-OK HOST-SHARED-MEMORY; do grep -q "$m" "$out" || { echo "wasm-e2e-atomics: missing marker $m"; exit 1; }; done
    # Same compile accounting as wasm-e2e run 1: every hot function compiled exactly once THROUGH
    # THE SHARED PATH (a sticky-Refused degradation would leave COUNT below EXPECT).
    exp=$(grep -oE 'E2E-EXPECT-COMPILES=[0-9]+' "$out" | cut -d= -f2)
    req=$(grep -oE 'HOST-COMPILE-REQUESTS=[0-9]+' "$out" | cut -d= -f2)
    cnt=$(grep -oE 'HOST-COMPILE-COUNT=[0-9]+' "$out" | cut -d= -f2)
    [ -n "$exp" ] && [ "$req" = "$exp" ] && [ "$cnt" = "$exp" ] || { echo "wasm-e2e-atomics: compile accounting mismatch (expect=$exp requests=$req count=$cnt)"; exit 1; }
    echo "✓ wasm-e2e-atomics ({{toolchain}}): O-17 shared-memory L0 — atomics guest boot/tierup/oracle/catch_unwind + shared-path compile accounting green"

# O-16f gate (design note otter-o16f-p3-bridge.md §7): the p3-BRIDGE E2E — mode doc §1.1 path 12
# ("core-module guest + p3-bridge host") productized. One synchronous Store hosts BOTH the §5.1
# otter_jit contract (unchanged, live tier-up) and a p3-world component (WASI 0.3.0 clocks+random
# via wasmtime_wasi::p3), with `otter_p3.fetch_stamp` proxying guest→component calls. Gate props:
#   · host built with cargo feature `p3-bridge` into its OWN target dir (target/p3, the
#     wasm-bprime pattern) — the stock-host artifact used by wasm-e2e/-atomics is never displaced
#     and the stock build graph stays free of the semver-EXEMPT wasmtime-wasi p3 module;
#   · guest built with --features wasm-emit,p3-bridge under the verbatim §7 flags (same pipeline,
#     one extra feature-gated import), copied aside like every other flavor;
#   · markers: E2E-9-P3BRIDGE-OK (3 pairwise-distinct stamps around 2 tier-ups) +
#     HOST-P3-STORE-SYNC-OK (§5.5 sync-norm latch probe: async_required never tripped) +
#     dual accounting (compiles == expected, bridge calls == expected);
#   · negative probe (non-vacuousness): the same host WITHOUT --p3-bridge must refuse the bridge
#     guest at instantiation with an unresolved `otter_p3` import — proving the marker run really
#     depended on the bridge wiring.
# O-16f gate: p3-bridge E2E — component+p1 guest in ONE sync Store, bridge×tier-up, latch probe
wasm-e2e-p3 toolchain=wasm-nightly: (wasm-toolchain toolchain)
    #!/usr/bin/env bash
    set -euo pipefail
    demo=demos/otter-wasm-demo/target/wasm32-wasip1/release
    hostdir=hosts/otter-host-wasmtime
    host=$hostdir/target/p3/release/otter-host-wasmtime
    # feature-gated host build + its own fmt/clippy/test pass (workspace-excluded crate rule; the
    # p3 unit gate p3_stamp_component_sync_round_trip_and_no_latch rides in `cargo test` here)
    (cd "$hostdir" && cargo build --locked --release --features p3-bridge --target-dir target/p3)
    (cd "$hostdir" && cargo fmt --check && cargo clippy --locked --release --features p3-bridge --target-dir target/p3 -- -D warnings && cargo test --locked --release --features p3-bridge --target-dir target/p3)
    # bridge guest: verbatim §7 pipeline + the p3-bridge feature, copied aside (shared target dir
    # coexistence: cargo keys units by feature set exactly as it does by RUSTFLAGS — O-17 note)
    (cd demos/otter-wasm-demo && cargo fmt --check && RUSTFLAGS="{{wasm-eh-rustflags}}" cargo +{{toolchain}} build --locked --release --target wasm32-wasip1 -Zbuild-std=std,panic_unwind --features wasm-emit,p3-bridge)
    cp "$demo/otter-wasm-demo.wasm" "$demo/otter-wasm-demo-p3.wasm"
    # ---- run: --p3-bridge + mode p3bridge ----
    out="$demo/e2e-p3.log"
    "$host" --p3-bridge "$demo/otter-wasm-demo-p3.wasm" p3bridge | tee "$out"
    for m in E2E-1-BUILD-OK E2E-9-P3BRIDGE-OK E2E-ALL-OK HOST-P3-STORE-SYNC-OK; do grep -q "$m" "$out" || { echo "wasm-e2e-p3: missing marker $m"; exit 1; }; done
    # compile accounting (tier-up × bridge coexistence: exactly one compile per promoted fn,
    # through the same Store the component lives in)
    exp=$(grep -oE 'E2E-EXPECT-COMPILES=[0-9]+' "$out" | cut -d= -f2)
    req=$(grep -oE 'HOST-COMPILE-REQUESTS=[0-9]+' "$out" | cut -d= -f2)
    cnt=$(grep -oE 'HOST-COMPILE-COUNT=[0-9]+' "$out" | cut -d= -f2)
    [ -n "$exp" ] && [ "$req" = "$exp" ] && [ "$cnt" = "$exp" ] || { echo "wasm-e2e-p3: compile accounting mismatch (expect=$exp requests=$req count=$cnt)"; exit 1; }
    # bridge accounting (exactly one host bridge call per guest bridge_stamp)
    bexp=$(grep -oE 'E2E-EXPECT-BRIDGE-CALLS=[0-9]+' "$out" | cut -d= -f2)
    bgot=$(grep -oE 'HOST-BRIDGE-CALLS=[0-9]+' "$out" | cut -d= -f2)
    [ -n "$bexp" ] && [ "$bgot" = "$bexp" ] || { echo "wasm-e2e-p3: bridge accounting mismatch (expect=$bexp calls=$bgot)"; exit 1; }
    # ---- negative probe: same host, NO --p3-bridge → unresolved otter_p3 import must refuse ----
    neg="$demo/e2e-p3-neg.log"
    if "$host" "$demo/otter-wasm-demo-p3.wasm" p3bridge > "$neg" 2>&1; then
        echo "wasm-e2e-p3: bridge guest UNEXPECTEDLY ran without --p3-bridge (marker vacuous?)"; exit 1
    fi
    grep -q "otter_p3" "$neg" || { echo "wasm-e2e-p3: bridge-less red did not name the unresolved otter_p3 import"; cat "$neg"; exit 1; }
    echo "NEGPROBE-P3-UNWIRED-OK bridge guest refused without --p3-bridge (unresolved otter_p3)" | tee -a "$out"
    echo "✓ wasm-e2e-p3 ({{toolchain}}): O-16f — bridge round trips pre/between/post tier-up, dual accounting, sync-latch untouched, unwired negative probe red"
