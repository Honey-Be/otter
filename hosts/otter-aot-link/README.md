# otter-aot-link

The b″ link driver of the O-16c′ M2a static-link pipeline (design:
`.claude-research-library/otter-o16c-componentization-design.md`, §3.1/§5/§6). This crate owns
the **logic** of the chain — renaming, gates, marker injection and sidecars — while
justfile recipes own chain composition and the only external tools (`wasm-merge`, wasm-tools
CLI — §3.2). Everything here is deterministic and network-free, and the crate has **no wasmtime
dependency** (§0-5: unit-level verification uses the wasmparser `Validator`; instantiation
assertions belong to the L-4 e2e gate).

## Placement decision (design §3.1 — recorded verbatim, A-5 precedent)

`hosts/` is a **choice, not a rule** — the v1 "hosts/'s de-facto rule = workspace-excluded
consumer-chain crates" generalization was retracted (panel T-N3: `demos/otter-wasm-demo` is also
a workspace-excluded consumer-chain crate and lives in `demos/`). Chosen because this crate is
the tool half of the A-product consumer chain and belongs next to its pair `otter-aot-e2e`
(same directory, same recipe/CI discipline — the "workspace-excluded ⇒ gate via justfile
recipe" rule is reused as-is). A new top-level directory was rejected as gold-plating (loses
the recipe/CI rule reuse). Workspace exclusion here is for **lockfile hygiene and gate-location
explicitness**, not a wasmtime train conflict (empty `[workspace]` table + own committed
`Cargo.lock`, `--locked` builds; serde/serde_json stay isolated in this crate's lockfile).

## L-1a surface (design §7 L-1a row)

```
otter-aot-link rename --mode import|export --input IN.wasm --output OUT.wasm
                      [--map-module OLD=NEW]... [--map-name OLD=NEW]... [--kebab]
otter-aot-link check --imports MANIFEST.json [--require-self-contained] ARTIFACT.wasm
otter-aot-link check --rename-invariance ORIGINAL.wasm --mode import|export ARTIFACT.wasm
```

- **`rename`** (§2-6/§6.1): a **general mapping-table** rename — arbitrary module-string pairs
  (import mode) and name pairs (import field names / export names), no hardcoded `otter_*`
  strings, so whatever core module-string convention the L-5 host-lift measurement reveals
  costs zero rework here (panel G-SF-2). `--kebab` (export mode only) performs the §6.1
  `_`→`-` conversion with the three **fail-closed rejections**: ① source name contains `-`
  (reverse-uniqueness would be lost), ② post-substitution name collision, ③ result is not a
  valid WIT identifier (uppercase, non-ASCII, leading digit, empty word). Implementation rule
  (SPK §4-3): **only the target section's bytes are rewritten** — every other section is copied
  through verbatim; the tool then self-asserts the §2-6 invariance and validates the product
  with the wasmparser `Validator` (well-formedness; the EH-census/semantic checks are
  `check --linked`'s, L-4). Empty mappings and mapping keys that match nothing are loud errors.
- **`check --imports`** (§5.1/§2-4): the residual core import set of a linked artifact must
  **exactly equal** (`=`, names AND types, both directions) the `otter_host` entry set of the
  artifact-manifest JSON sidecar; everything else (`otter.memory`, `otter_helpers.*`, …) is 0.
  `⊆` was rejected (panel G-SF-3): a proper subset means a **lost** import — silent-pass would
  violate fail-closed. Detects lost, excess, and **type-mismatched** residuals (expected types
  from the canonical O-14 §2.3 HOST1/HOST2/HOSTCTX2 mapping). `--require-self-contained` is
  the empty-set emphasis mode for host-free artifacts.
- **`check --rename-invariance`** (§2-6): import mode may change only the import section's
  bytes; export mode only the export section's — asserted on raw full-section bytes (size LEBs
  included), custom sections inside the perimeter.
- **`schema` module** (§3.1/§0-7): the single owner of the JSON sidecar types — producers
  (compile_module callers, e.g. `otter-aot-e2e`) path-dep this crate and serialize the same
  types (`TryFrom<&AotManifest>`), so the schema never forks into a second truth. Carries both
  the artifact-manifest sidecar (L-1a) and the b′ library sidecar (L-1b).

## L-1b surface (design §7 L-1b row)

```
otter-aot-link lib-manifest --extract UNSTRIPPED.wasm --output SIDECAR.json
                            --otter-version V --rustc-version V
otter-aot-link lib-manifest --finalize STRIPPED.wasm --sidecar SIDECAR.json
otter-aot-link inject-marker --input BPRIME.wasm --output OUT.wasm
                             --manifest M.json --lib-manifest L.json
otter-aot-link check --scratch --manifest M.json --lib-manifest L.json
                     --lib-artifact BPRIME.wasm ARTIFACT.wasm
```

- **`lib-manifest`** (§2-2, two stages in the §2-2 generation order): `--extract` runs on the
  **pre-strip** artifact (the `__stack_pointer` is identified by NAME via the name section —
  when that fails, the largest-mutable-i32 heuristic is deliberately a **failure, not a silent
  fallback**, §8-6) and computes `heap_floor = max(__stack_pointer, max active-data end,
  memory_min × 65536)` — the `memory_min` term absorbs the passive-segment blind spot (panel
  M1). `--finalize` runs after `wasm-tools strip` and binds `artifact_sha256` over the
  strip-final master bytes (§4.3 content-hash truth), after cross-checking memory/exports
  against the sidecar (wrong-pair refusal). Version fields are record-only (§4.3 honesty note).
- **`inject-marker`** (§2-1): width-branched marker injection — W≥16: `OTTRSCR0`@base +
  `OTTRSCR1`@end−8; W==8: merged `OTTRSCRM`; W==0: explicit no-op branch (logged, never
  silent) — plus the **constructive memory-min raise** to `max(min, ⌈end/65536⌉)` (a raise
  past the declared max is a loud failure). The input's SHA-256 is compared against the
  library sidecar BEFORE injecting (fail-closed, no bypass); region source is the per-product
  artifact manifest (§4.2: injection is a per-product link step). Surgery is limited to the
  memory/data/data-count sections, self-asserted, and the product is Validator-green.
- **`check --scratch`** (§5.2, all three): (i) marker match per width branch **+ non-marker
  active data ∩ region = ∅** (passive segments are the stated blind spot — no static address;
  W==0 is EXEMPT from the ∩ sub-check — an empty region contains no bytes to intersect, L-2
  hardening (a));
  (ii) coverage `min ≥ ⌈end/65536⌉` (trivially green on W==0); (iii) library floor
  `base ≥ sidecar.heap_floor` (the only static shadow-stack defense — SPK §5-2; red recovery =
  re-emit with a new base, O-14 §2.5-iv). Requires `--lib-artifact` to re-verify the §4.3
  sidecar pair (check does not trust that inject-marker already did).

## L-2 hardening (the L-1b review's four carried-over prescriptions, folded in L-2)

- **(a) W==0 ∩-exemption**: the overlap sub-check no longer false-reds a segment spanning an
  EMPTY region's base point (`[base, base)` reserves nothing) — twin test in `l1b_gates.rs`.
- **(b)+(c) memory64 / custom-page-size refusal at the COMMON seam** (`facts::owned_memory`):
  every consumer path (extract / finalize / inject-marker / check --scratch) routes through
  `owned_memory`, so both refusals live there — one wiring point for one invariant
  (check-completeness discipline; per-path rejections were judged and rejected). Before:
  only `--extract` refused memory64 (the other three proceeded fail-open, and inject-marker's
  memory re-encode hardcoded `memory64: false` — a silent-rewrite class, now a faithful
  carry); custom page sizes broke the 64 KiB-page floor/coverage math silently. Each of the
  four paths has a refusal test.
- **(d) finalize re-bind branch**: the "already finalized × same metadata × different bytes"
  refusal (only the bound content hash can catch it) is now test-covered.

## L-4 surface (design §7 L-4 row — moved here from L-1 by panel G-SF-5)

```
otter-aot-link check --linked --manifest M.json ARTIFACT.wasm
otter-aot-link shrink-exports --input IN.wasm --output OUT.wasm --keep NAME...
```

- **`check --linked`** (§5.3): the **semantic-level** post-link gate — byte gates are
  impossible after wasm-merge's IR round-trip re-encode (SPK §4-4). Items: single memory
  DEFINITION (library-owned shape), `otter_entry`/`otter_table` exports, one declared table
  with `min == max == manifest.table_size`, one active element segment on table 0 at offset
  `i32.const 0` with `table_size` items, a machine **EH census** over every code body (current
  + legacy operator sets, plus tag-section infrastructure) — and the §6.3 profile-split
  **start check**: absent on non-shared artifacts; on shared artifacts, only the wasm-ld
  `__wasm_init_memory` init shape passes (a body-operator whitelist with ZERO call-class
  operators). **Recorded deviations** from the §5.3 whitelist wording, both measured on the
  real +atomics b′ start body: `br_table` (the cmpxchg guard dispatch — same non-call control
  class as `br`/`br_if`) and `i64.const` (the atomic-wait timeout operand); `atomic.fence`
  stays excluded (not in the design families, not in the measured body).
- **`shrink-exports`** (§5.4): keep-list export-surface reduction (the recipe passes
  `otter_entry` + `otter_table` + `memory`); the keep list is a CALLER input (the rename
  mapping-table generality rule). Fail-closed: empty keep list, a keep name matching no
  export, and a missing export section are all loud errors; the rewrite touches only the
  export section (export-mode invariance self-asserted) and is Validator-green.

The wasmtime **instantiation assertions** stay OUT of this crate (§0-5: wasmparser-only):
they live in `otter-aot-e2e`'s L-4 modes (`assert-instantiate` / `run-linked`), driven by
`just wasm-aot-link-e2e`.

## L-5 surface (design §7 L-5 row — componentization)

```
otter-aot-link check --component --manifest M.json COMPONENT.wasm
```

- **`wit/aot-artifact.wit`** (§6.1): the CANONICAL WIT world file — `aot-artifact`
  (spike-verbatim: `export otter-entry: func(ctx: u32, args: u32, argc: u64) -> s64`,
  canonical ABI lowering measured `(i32,i32,i64)->i64` exactly) plus `aot-artifact-host`
  (the §2-4-3 host-lift world: the gate corpus' `otter_host` set as component-level imports).
  **L-5 measured convention** ([추정]⑤ resolved, pinned wasm-tools 1.246.2): world-level
  function imports bind to core imports under the module string **`$root`** with the kebab
  field name — the chain rewrites `otter_host.h_add7` → `$root.h-add7` with the GENERAL
  mapping-table `rename` (zero tool rework, exactly as panel G-SF-2 planned). An unrenamed
  module string fails loudly at `component new` ("requires an import interface named
  `otter_host`").
- **`check --component`** (§6.2, panel T-S4): **NATIVE** wasmparser `component-model` checks
  (explicit Cargo.toml feature), never a CLI text-dump comparison — ① native validation under
  the EH-excluded feature set (validates the whole component INCLUDING nested core modules
  with EH structurally rejected), ② **empty-world detection** (the E10 trap: core exports are
  never auto-promoted — `component new` without `embed --world` yields an exports-empty
  component, exit 0), ③ world-shape **exact** match: exports == `[otter-entry (func)]`,
  imports == the kebab image of the manifest's `otter_host` set, both directions (the §5.1
  `=`-not-`⊆` discipline at the component boundary), ④ EH census over every depth-0 nested
  core module (operator set shared with `check --linked`).

The embed/new chain, the E10/E11 negative probes and the component EXECUTION differential
(host wiring included — `otter-aot-e2e run-component`) are `just wasm-aot-component`'s.

## Gates

`just wasm-aot-link-check` — build (`--locked`), fmt, clippy (`-D warnings`), and the unit
gates (hand wasm fixtures):

- L-1a: invariance both modes (positive + negative), residual-import `=`
  (lost/excess/type-mismatch negatives), kebab rejections ①②③, rename products
  Validator-green under the EH-excluded feature set, schema roundtrip + `deny_unknown_fields`
  refusal, CLI exit-code smoke.
- L-1b: every `check --scratch` item (i)/(ii)/(iii) positive AND negative (panel G-SF-7),
  the overlap-segment injection negative (§5.6-⑩'s unit twin), W ∈ {0, 8, ≥16} three-shape
  inject→check fixtures, the **shared-memory active-marker fixture at the wasmparser level**
  (design [추정 ④] stage-1 measurement — green; engine acceptance is L-4-③), library-sidecar
  schema roundtrip, §4.3 pair seams (forged hash / unfinalized sidecar / wrong pair), §8-6
  loud extract failure, data-count bump, CLI chain smoke.
- L-4 (`tests/l4_gates.rs`): every `check --linked` item positive AND negative (start
  profile-split + call-in-start whitelist red, imported/double memory, missing/wrong-kind
  exports, table/element shape, EH operator + tag-section census) and the `shrink-exports`
  refusals + the composition check (post-shrink artifact still `check --linked`-green).
- L-5 (`tests/l5_gates.rs`): `check --component` green (hand-built `ComponentBuilder`
  fixtures, self-contained + host-lifted) and the negatives — empty world (E10 machine twin),
  core-module input (chain-order violation), wrong export name, LOST host lift, EXCESS
  component import, EH-in-nested-module (native EH-free validation rejection direction).

The REAL linked artifacts (b′ × compile_module products through pinned wasm-merge) are gated
by `just wasm-aot-link-e2e` — the L-4 chain recipe (three shapes oracle-identical, EH-free
final artifacts, check (iii)/start-check live runs, wasm-merge feature-flag census, the 40k
differential once through a linked artifact) — and the REAL components by
`just wasm-aot-component` — the L-5 chain recipe (kebab → host-lift rename → embed/new with
the pinned wasm-tools → `check --component` → EH-free validate/census → `run-component`
execution differentials: calc host-wired oracle-identical + deep 2M bounded in-component,
plus the E10 empty-world and E11 pre-link negatives).

## Dependency pins

`wasmparser`/`wasm-encoder` 0.258 are in **manual lockstep** with the workspace pin
(`crates/otter-wasmemit/Cargo.toml` — same wasm-tools release train); bump only together.
`serde`/`serde_json` are isolated in this crate's own lockfile (no workspace ripple).
`sha2` is a **recorded L-1b deviation** from the §3.1 dependency list (see Cargo.toml):
§2-2/§4.3 need in-crate SHA-256, and both hand-rolled crypto and PATH `sha256sum` fall into
classes the design rejects (over-self-implementation / silent system-tool substitution).
