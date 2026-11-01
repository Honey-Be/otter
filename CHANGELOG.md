# Changelog

Notable changes to the otter workspace crates. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow the workspace SemVer
(`Cargo.toml` `workspace.package.version`).

## [Unreleased]

## [1.0.0-testing.2] - 2026-11-01

Second testing-channel cut. Carries the post-`testing.1` H-package (stack double-guard, `endbr64`,
stack probes, `NEVER` contract) plus a **security dependency refresh and toolchain raise**. Release
security gate: second assessment in `RELEASE-GATE.md`.

- **SECURITY / dependency refresh** — the `testing.1` host train was pinned to `wasmtime`/
  `wasmtime-wasi` **48.0.1**, which upstream has since flagged by 23 + 7 advisories (RUSTSEC-2026-
  0314..0364, 0378/0379, 0420..0424; incl. GHSA-mghf-wjfp-p5xm CVE-2026-115893, CRITICAL, aarch64
  `br_table`). Hosts (`otter-host-wasmtime`, `otter-aot-e2e`) now pin **wasmtime / wasmtime-wasi
  50.0.1** (lockfile-pinned, `--locked`); `tokio` 1.53.1 → 1.54.1 (RUSTSEC-2026-0386). The workspace
  dev-only AOT differential oracle (`otter-wasmemit` dev-dep) moves **45 → 50** as well. No otter
  source change was needed for the 48 → 50 host move. **Embedders consuming the wasmtime-48 host
  shape must re-pin.** The `wasmtime-wasi` `p3` surface is still semver-exempt (RELEASE-GATE (d2)).
- **Toolchain / MSRV raised 1.96 → 1.99** (`rust-toolchain.toml` = stable **1.99.1**,
  `rust-version = "1.99"` on the workspace and every host/demo). Required by wasmtime 50 (MSRV 1.97)
  and aligned across the neosurf and tsavorite consumers by owner decision (2026-11-01).
- **Carried from the post-`testing.1` hardening package** (previously "Unreleased"):
  - **BREAKING-adjacent (H5)** — the `ModuleEngine`/`Engine` promotion threshold `u32::MAX` is now
    the `NEVER` sentinel: that tier is NEVER promoted to (old semantics: a saturating counter that
    fired after 2^32−1 calls). The tier-order assert is relaxed to `t1 <= t2 || t1 == NEVER`,
    adding a T2-direct mode (`t1 = NEVER`, real `t2` — no T1 RX page is ever emitted), and
    `ModuleEngine::interpreter_only(isa, module)` is a new constructor for the T0-pinned shape.
  - **New default behavior (H1a)** — the `ModuleEngine` dispatch path is double stack-guarded by
    default: a byte budget (`stack_budget`, 1 MiB) plus a recursion-depth backstop (`depth_limit`,
    4096). Deep non-tail recursion now yields a RECOVERABLE `Trap` (see `TrapReason`) instead of a
    native stack-overflow abort; for calibration (thread stack sizes, compiler headroom, wasm32),
    see the `set_stack_budget`/`set_depth_limit` rustdoc.
  - **Artifact bytes changed (H2/H1b/H1c)** — every T1 function now begins with `endbr64`
    (+4 bytes at the head of both prologues); T1 and T2 emit stack probes for large frames
    (`frame > 4096`): T1 a sub-then-touch page walk, T2 Cranelift inline probestack.
