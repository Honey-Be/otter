# otter-helpers-wasm

The b′ **engine-library wasm** of the O-16c′ M2a static-link pipeline (design:
`.claude-research-library/otter-o16c-componentization-design.md` §4 — the L-2 promotion of the
spike wrapper, source essentially unchanged). It serves the three helper imports of an O-14 AOT
artifact (`otter_helpers.{otter_binop,otter_convert,otter_unop}` — thin verbatim delegation to
the canonical `crates/otter` helpers, §12.3 export duty) and owns the linear memory. That is
the *whole* gate-profile M2a engine library (SPK B1: otter crate alone suffices, otter-engine
is mechanically excluded by its O-12 `compile_error!` under panic=abort).

Built by **`just wasm-bprime`**: repo-pinned stable via plain `cargo` (rust-toolchain.toml is
the single pin authority — §4.2 panel T-S2), `--locked` against the committed `Cargo.lock`
(§4.2 panel M1: b′ IS the gate artifact — unpinned resolution would silently move the bytes
and void the sidecar SHA binding), two profiles, then the sidecar two-stage
(`lib-manifest --extract` pre-strip → `wasm-tools strip` (pinned, non-PATH) →
`lib-manifest --finalize` binding the strip-final SHA-256), the EH-free validate
(`--features=all,-exceptions,-legacy-exceptions`) + EH census 0, the panel-M1 heap_floor
constant assertion, and the B2–B5 replay (`otter-aot-e2e bprime-diff` — the spike's
40,000-case differential, absorbed there per the §7 L-2 row).

## Placement decision (design §4.1 — recorded verbatim)

`hosts/` is a **choice, not a rule** (the v1 "hosts/'s de-facto rule" generalization was
retracted, panel T-N3). Chosen because b′ is the guest-side half of the A-product deployment
chain and belongs under the same directory discipline as its chain pair (`otter-aot-link`,
`otter-aot-e2e`) — recipe/CI rule reuse is maximized. **crates/ rejected**: workspace members
share the root profile and b′ requires its own `[profile.release] panic="abort"` (profiles are
root-owned — inexpressible as a member); a wasm32-wasip1-only artifact also has no business in
`--workspace` builds. **demos/ rejected**: a product constituent, not a demo.

## Measured artifact shapes (L-2 re-measurement, PROFILE-SPLIT — §2-1/§4.2 B5 re-statement)

The spike's "data segments 0 · start 0" line was a **non-shared-profile fact over-generalized**
(panel M3); the corrected, per-profile record measured on the products of `just wasm-bprime`
(stable 1.96, wasm-tools 1.246.2 pinned; sizes are the stripped link inputs):

| profile | size (gzip) | data segments | start | memory | heap_floor |
|---|---|---|---|---|---|
| non-shared | 4,158 B (1,676 B) | **0** | **0** | `(memory 16)` | `0x100000` (= `__stack_pointer` = min×64Ki) |
| +atomics | 4,682 B (2,015 B) | **1 — passive `$.rodata`** (~300 B panic strings; no static address — placement is baked into start's `i32.const 0x100000; memory.init`) | **1 — `(start $__wasm_init_memory)`** (memory.init + data.drop + atomic init guard) | `(memory 17 65536 shared)` | `0x110000` (= min×64Ki — the §2-2 `memory_min×65536` term absorbs the passive blind spot, panel M1) |

Both profiles: imports **0**, exports exactly `memory` + the 3 helpers (§12.3 verbatim,
B2-gated), EH census **0**, EH-excluded validate green. The +atomics profile additionally
carries the `$__tls_base` global. The trap-allocator "no-allocation" property is profile- and
marker-independent (B3's 40,000×0 replay would trip it instantly). The +atomics build emits
rustc's "unstable feature … `atomics`" **warning** on stable — a pass without guarantee,
registered on the MODE doc §1.4 O-W watchlist (row 7): a warning→error flip re-opens the
shared profile only (nightly/build-std re-discussion), the non-shared gate path is unaffected.

**Representative-region injected TEST artifact** (verification-only — the recipe ships the
UNINJECTED master + sidecar; `inject-marker` is a per-product L-4 link step, §4.2 panel C-S7):
region `[0x110000, 0x110010)` (W=16) → 4,193 B, data segments **2** (`OTTRSCR0`@base +
`OTTRSCR1`@end−8), memory min **16 → 18** (constructive raise), `check --scratch` all three
green, still EH-free. I.e. an injected b′ is *no longer* "data 0" — the §2-1 mandated
narrative update to the spike's B5 line.

## Products (under `target/bprime/`, rebuilt by the recipe)

- `otter_helpers.wasm` + `otter_helpers.manifest.json` — non-shared master + finalized sidecar
- `otter_helpers.atomics.wasm` + `otter_helpers.atomics.manifest.json` — +atomics pair
- `otter_helpers.rep-injected.wasm`, `rep-region.manifest.json`, `bprime-diff.log` — gate
  by-products (verification-only)

Embedders using the master directly: `inject-marker` + `check` are MANDATORY before deployment
(O-14 §12.2 (i)(ii), toolized — see `hosts/otter-aot-link`).
