# otter

A reusable, project-agnostic **meta-VM framework** in Rust: you define an ISA (`Isa`/`OpDef` —
effects + expressions over a register-based, 3-address IR with `Value = i64`), and otter gives every
function in it a tiered execution ladder with **zero feature gaps between tiers** (performance gaps
are fine and expected; semantic gaps are release-blocking by policy).

| tier | crate | what | where |
|---|---|---|---|
| T0 | `otter` (`interp`) | reference interpreter — the semantic canon | every arch, `no_std` |
| T1 | `otter-baseline` | hand-emitted baseline JIT | x86-64 hosted |
| T2 | `otter-cranelift` | Cranelift JIT (+ `CallMode::Table` AOT-style path) | hosted, Cranelift-supported arches |
| Twasm | `otter-wasmemit` (+ `otter-engine`) | runtime wasm-emit JIT: IR → fresh wasm module → host `otter_jit.compile` (the Jiterpreter pattern) | wasm32 guests, EH-capable WASI hosts / browsers |
| A-mode | `otter-wasmemit::compile_module` | build-time whole-module AOT → static link → **standard wasm component** | any wasm 3.0 host, stable toolchain, memory64 capable |

`otter-engine::ModuleEngine` is the adaptive dispatcher that ties the JIT tiers together
(hot-count tier-up, fail-closed trap channel, `try_dispatch`/`clear_trap`, `push_func`).

## Layout

- `crates/` — the five workspace members above. Core crates are `#![no_std]` + `forbid(unsafe_code)`
  (unsafe is concentrated in the engine's FFI seams, documented per-site).
- `hosts/` — workspace-EXCLUDED tooling (own lockfiles, heavy deps): `otter-host-wasmtime` (the
  `otter_jit` contract reference host, wasmtime 48), `otter-aot-link` (rename/inject-marker/check/
  shrink/manifest link tool), `otter-aot-e2e` (deployment-shape E2E + differentials),
  `otter-helpers-wasm` (the b′ engine library, stable+abort), `otter-glue-js` (browser glue).
- `demos/otter-wasm-demo` — the in-guest acceptance suite (§8(c)).
- `.claude-research-library/` — the decision/contract canon these crates cite by path (design docs,
  spike results, mode-architecture record). Contract entry points: `otter-o13-wasm-emit-design.md`
  §5 (the `otter_jit` J-mode contract, v1.1) and `otter-o14-aot-core-design.md` §12 +
  `otter-o16c-componentization-design.md` (the A-mode artifact/link contracts).

## Building / gates

Stable pin `rust-toolchain.toml` (1.96). `just ci` = fmt + clippy(-D) + workspace tests +
no_std check. The wasm lanes (`just wasm-e2e`, `wasm-e2e-atomics`, `wasm-e2e-p3`, `wasm-aot-e2e`,
`wasm-aot-link-e2e`, `wasm-aot-component`, `wasm64-smoke`) need the pinned nightly for guest builds
(`just wasm-toolchain`) and the SHA-pinned external tools (`just wasm-aot-tools`); J-mode guest
builds are the only nightly consumers — A-mode artifacts are stable-toolchain products end to end.

## Release channels & versioning

- **`main` = unstable** — development trunk.
- **`testing`** — pre-stable channel; current cut **`v1.0.0-testing.1`** (owner-approved
  2026-09-04). The `-testing.N` pre-release identifier is the channel marker.
- **stable** — a future tag dropping the pre-release suffix, gated by the release checklist
  (`RELEASE-GATE.md`) and the tracked-limitations register (`docs/`).
- Major-bump rule (owner, 2026-09-04): completing/deciding ALL items in the v2 register
  (`docs/otter-v2-major-bump-register.md`) triggers an immediate major version bump.

Provenance: extracted as-is from the `neosurf` monorepo at commit `3201f32` (2026-09-04). The tool
cache path `~/.cache/neosurf/wasm-aot-tools` is retained verbatim from the source tree (a cache
location only). crates.io publishing is a separate, not-yet-started track (`publish = false`).
