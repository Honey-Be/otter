# otter-wasm-demo

O-13 §8(b)/(c) guest binary (design doc `.claude-research-library/otter-o13-wasm-emit-design.md`,
§9 row `otter-wasm-demo` / panel C#8). As of **W-5** this is the E2E acceptance suite itself: run
under the reference host (`hosts/otter-host-wasmtime`) it executes the §8(b) in-guest cross-tier
differential and prints one greppable marker per §8(c) acceptance item — `just wasm-e2e` (repo
root) builds both feature configs and gates all three runs:

| mode (WASI argv[1]) | build | host flags | acceptance markers |
|---|---|---|---|
| `tierup` | `--features wasm-emit` | (none) | `E2E-1-BUILD-OK` `E2E-2-TIERUP-OK` `E2E-3-ORACLE-OK` `E2E-4-TRAP-OK` `E2E-7-GROW-OK` `E2E-8B-CROSSTIER-OK` + `E2E-EXPECT-COMPILES=` |
| `reject` | `--features wasm-emit` | `--reject-compiles` | `E2E-1-BUILD-OK` `E2E-5-REJECT-OK` + `E2E-EXPECT-COMPILE-REQUESTS=` |
| `t0` | (feature off) | `--no-otter-jit` | `E2E-1-BUILD-OK` `E2E-6-FEATUREOFF-OK` |
| `p3bridge` | `--features wasm-emit,p3-bridge` | `--p3-bridge` | `E2E-1-BUILD-OK` `E2E-9-P3BRIDGE-OK` + `E2E-EXPECT-COMPILES=` `E2E-EXPECT-BRIDGE-CALLS=` (O-16f, gated by `just wasm-e2e-p3`, not `wasm-e2e`) |

The `p3bridge` row is the O-16f p3-bridge demo surface (design note
`.claude-research-library/otter-o16f-p3-bridge.md`): with the `p3-bridge` cargo feature the guest
gains exactly one extra import, `otter_p3.fetch_stamp(dst: *mut u64) -> i32`, which the reference
host's `--p3-bridge` mode serves by calling a real component (WASI 0.3.0 clocks+random through
`wasmtime_wasi::p3`) in the same synchronous Store; the mode interleaves three bridge round trips
with two real otter_jit tier-ups. Feature OFF (every other row) keeps the frozen import surface
`{wasi_snapshot_preview1, otter_jit}` byte-exactly — §5.1 stays untouched.

Every successful run ends with `E2E-ALL-OK`; assertion failures print `E2E-FAIL: …` and exit
non-zero (the host propagates the guest exit code). The `E2E-EXPECT-*` lines are the guest's own
compile accounting, which the recipe cross-checks against the host's `HOST-COMPILE-REQUESTS=`/
`HOST-COMPILE-COUNT=` counters (§8(c)-5: exactly one `otter_jit.compile` per hot function, no
retry loop; under `--reject-compiles` COUNT must be 0 while REQUESTS stays once-per-function).

## Canonical build (one command, repo root)

```sh
just wasm-demo-build
```

which expands to (pin + flags owned by the `justfile`; bump procedure: design doc §7.1):

```sh
rustup toolchain install nightly-2026-08-31 --profile minimal \
  --component rust-src --target wasm32-wasip1   # idempotent; BOTH prerequisites are required
cd demos/otter-wasm-demo
RUSTFLAGS="-Cpanic=unwind -Cllvm-args=-wasm-use-legacy-eh=false \
  -C link-arg=--export-table -C link-arg=--growable-table" \
  cargo +nightly-2026-08-31 build --release --target wasm32-wasip1 \
  -Zbuild-std=std,panic_unwind --features wasm-emit
```

Artifact: `target/wasm32-wasip1/release/otter-wasm-demo.wasm`.

Flag inventory (all load-bearing, design §7 / §5.3):

| piece | why |
|---|---|
| `nightly-2026-08-31` + `rust-src` + `rustup target add wasm32-wasip1` | `-Zbuild-std` needs nightly + the std sources; the self-contained wasi-libc rides in the target's rust-std component — omit the target and the build fails with EH-unrelated-looking link errors (`crt1-command.o` / `-lc` / `-lunwind`) |
| `-Zbuild-std=std,panic_unwind` + `-Cpanic=unwind` | shipped wasm std is abort-only; this is the only configuration where in-wasm `catch_unwind` works, i.e. the O-12 fail-closed contract holds (owner decision §12-1) |
| `-Cllvm-args=-wasm-use-legacy-eh=false` | exnref pin — a no-op on the current pin (LLVM 23 emits standard exnref by default); kept defensively, re-evaluated at every pin bump (§7.1) |
| `-C link-arg=--export-table -C link-arg=--growable-table` | §5.3 embedder build requirement: exports `__indirect_function_table` and removes its max so the host can `table.grow` — must be on the FINAL artifact link (this crate), not an rlib |
| `--features wasm-emit` | opts into the `otter_jit.compile` import surface (§5.1); feature-off must keep running as plain-WASI T0 (O-7; §8(c)-6 — gated by `just wasm-e2e`'s third run, which builds this crate WITHOUT the feature and runs it under the host's `--no-otter-jit` mode) |
| `[profile.release]` = opt-level 3 / thin LTO / 1 CGU | C#7: deployment-representative profile (the W-0 spike guest was opt-level=1/no-LTO; this crate closes that combination gap) |

## Why this crate is workspace-excluded

The workspace pins stable 1.99.1 (`rust-toolchain.toml`) and must stay entirely
stable-buildable; this crate's deliverable requires the nightly build-std pipeline above. As a
workspace member it would either drag that requirement into `cargo build/test --workspace` or sit
permanently broken. So it lives outside the root `members` list and carries an empty
`[workspace]` table in its `Cargo.toml`, which stops cargo's upward workspace discovery.
**Source stays stable-compatible** — nightly is only the build pipeline (§12-1); do not introduce
nightly-only language features here.

## Why explicit `cargo +nightly-...` instead of a per-directory `rust-toolchain.toml`

Decision (W-4): the toolchain override is carried by the **command** (justfile recipe + this
README), not by a directory-local `rust-toolchain.toml`. Rationale:

1. **A toolchain file alone can never make `cargo build` correct here** — the §7 configuration
   also needs `RUSTFLAGS` + `-Zbuild-std`, so the one canonical build command must exist
   regardless. Putting the toolchain into that same command keeps a single source of truth
   instead of splitting the config across a file and a command.
2. **One pin location fewer.** The nightly date pin already lives in the `justfile`
   (`wasm-nightly`) and the CI lane; a directory toolchain file would be a third copy to bump in
   lockstep (§7.1 keeps the bump-touch list short).
3. **No surprise shadowing.** A directory-local file would silently override the repo's pinned
   stable for every tool that wanders into this directory (editors, `cargo metadata`, casual
   `cargo check`), and rustup would auto-download a full nightly on first touch. Explicit `+`
   keeps stable-by-default semantics everywhere in the repo.

(Trade-off accepted: a bare `cargo build` in this directory builds the *host*-target skeleton on
stable — which is harmless and even useful for quick type-checking; the real artifact is only
ever produced by the canonical command.)

## Verifying the artifact

```sh
wasm-tools print target/wasm32-wasip1/release/otter-wasm-demo.wasm | grep -m1 'otter_jit'
# expect: (import "otter_jit" "compile" (func ...))  — feature-on link surface, §5.1
```

Running it (W-5): under the reference host —

```sh
just wasm-e2e            # repo root: builds host + both guest configs, runs and greps all 3 modes
# or by hand, after `just wasm-demo-build` and a host build:
hosts/otter-host-wasmtime/target/release/otter-host-wasmtime \
  demos/otter-wasm-demo/target/wasm32-wasip1/release/otter-wasm-demo.wasm tierup
```

Plain `wasmtime run` will refuse to instantiate the feature-on artifact (unresolved
`otter_jit.compile` import) — that is the §5.1 contract, not a bug. The feature-off build runs on
any WASI runtime **with wasm exception-handling support** as T0 (that is exactly what the `t0`
mode run asserts): the §7 build carries exnref (`try_table`/`throw_ref`) even feature-off, so an
EH-less runtime rejects the module at parse time — on the reference host (wasmtime 48) that is
`Config::wasm_exceptions(true)` (default-on since 47, still set explicitly — design §5.5),
exactly what the reference host's `--no-otter-jit` mode models.
