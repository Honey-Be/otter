# otter-aot-e2e — O-14 A-5 host-train E2E gate

Design: `.claude-research-library/otter-o14-aot-core-design.md` §8.2 / §9 A-5 (serving
interface: §12).

Instantiates and runs `otter_wasmemit::compile_module` AOT artifacts on the **deployment host
shape** — wasmtime family **48** with the reference host's exact feature set
(`runtime,cranelift,gc,gc-drc,std,threads`) and `Config` (`wasm_exceptions(true)` +
`shared_memory(true)`, hosts/otter-host-wasmtime verbatim). The artifact is EH-free by
construction (§3.3); the EH/threads knobs are set anyway because the gate is defined as "the real
deployment shape, as-is" (§8.2 실배포 형상 정합).

Canonical run (repo root, marker-grep gated, wired into the CI `wasm-eh-pinned` lane):

```
just wasm-aot-e2e
```

What it proves (one marker per phase, `AOT48-E2E-ALL-OK` seals the run):

1. **ENTRY** — non-shared artifact, `otter_entry` called in the §2.4/§12.4 v1 canonical shape
   `(ctx=0, args=0, argc=0)`, bit-identical to the native T0 oracle; every import row exercised
   (memory + all 3 `otter_helpers.*` + 2 `otter_host.*` + a marshaled non-tail `Call`).
2. **SHARED-LINK** — same program with the `(memory 0 65536 shared)` import shape linked against a
   real `wasmtime::SharedMemory`.
3. **TAILBOUND** — [스파이크] M2 re-confirmed on the 48 train: 2,000,000/2,000,001-deep mutual
   `TailCall` recursion completes in a 64 KiB wasm stack.
4. **SHARED-TAILBOUND** — the §2.7 combination the design left as [추정]: shared memory import ×
   declared table × `return_call_indirect`, measured as one artifact.
5. **NEGCTRL** — the same shape with ordinary `Call` dies with `Trap::StackOverflow`, isolating
   3/4's boundedness to `return_call_indirect` itself.

## O-16c′ modes (L-2 + L-4)

Beyond the A-5 phases, the binary carries the O-16c′ subcommand modes (design
`otter-o16c-componentization-design.md` §7 L-2/L-4 rows; the binary does **execution and
diffing only** — external tools are the recipes'):

- `bprime-diff <lib.wasm>` (L-2): the spike b′ differential absorbed as a gate — B2 exact
  surface (import 0 / export 4종), B3 40,000-case helper differential vs the native canonical
  helpers (0 mismatches), B4 panic-path probes. Driven by `just wasm-bprime`.
- `emit-aot <dir>` (L-4): writes the three REAL `compile_module` products (calc / deep /
  calc_shared) + JSON manifest sidecars serialized through otter-aot-link's `schema` module
  (single schema owner, producer side — §3.1/§0-7).
- `run-linked calc|deep|calc-shared <linked.wasm>` (L-4): instantiates a LINKED artifact
  (b′-owned memory inside; only residual `otter_host.*` resolved, by name) and proves
  oracle-identity (calc/calc-shared) or the 2M-bounded parity contract (deep, 64 KiB stack).
  NOTE: fuel is armed BEFORE `Instance::new` — the shared linked artifact runs its init-only
  `__wasm_init_memory` start at instantiation (measured: 0-fuel instantiation traps).
- `linked-diff <linked.wasm>` (L-4): the 40k differential + B4 probes once through a
  self-contained linked artifact's export-union helper exports (§5.3 corpus reinforcement).
- `assert-instantiate <wasm>` (L-4): the wasmtime instantiation assertion moved out of L-1
  (§0-5 — otter-aot-link stays wasmtime-free); used on the marker-injected b′ gate copies,
  shared profile included.

L-4 constants raise (design §2-2 파급 (b)/(c)): `SCRATCH_BASE` 0x10000 → **0x110000** (the
+atomics heap_floor; the old base sat inside the b′ shadow stack) and `MEMORY_PAGES` 16 →
**18** (coverage companion). Driven end-to-end by `just wasm-aot-link-e2e`.

Why a separate crate (not a reference-host subcommand): 2-product separation (MODE §4.1 — the
reference host serves the J-mode `otter_jit.compile` contract, this binary the A-mode artifact
contract), lockfile hygiene (the reference host's Cargo.lock is the host-train pin authority), and
disjoint needs (this gate generates the artifact at run time via `compile_module` — the
single-lowering-source rule — and needs no WASI, no guest file). Workspace-EXCLUDED with its own
lockfile, exactly like hosts/otter-host-wasmtime and demos/otter-wasm-demo: the wasmtime-48 tree
must not ride into `cargo test --workspace` (the workspace harness stays pinned to family 45 via
cranelift-0.132 lockstep). Builds on the repo's pinned stable — no nightly involved anywhere.
