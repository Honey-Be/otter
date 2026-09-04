# Changelog

Notable changes to the otter workspace crates. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow the workspace SemVer
(`Cargo.toml` `workspace.package.version`).

## [Unreleased]

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
