# otter-glue-js

O-13 §5.4 **browser JS glue**, reference implementation of the `otter_jit` dynamic-compilation
contract (design doc `.claude-research-library/otter-o13-wasm-emit-design.md` §5) — the browser
counterpart of the native reference host `hosts/otter-host-wasmtime`. One file, no dependencies:
[`otter-glue.mjs`](./otter-glue.mjs) exports `makeOtterJit()`.

```js
import { makeOtterJit } from "./otter-glue.mjs";

const jit = makeOtterJit();
const { instance } = await WebAssembly.instantiate(mainModuleBytes, {
  ...wasiImports, // the embedder's own WASI/host imports
  otter_jit: jit.imports.otter_jit,
});
jit.bind(instance); // late-bind: required before guest code first tiers up
```

## Norm compliance (§5.3) and the MAY items this glue deliberately omits

Every §5.3 step is labeled at its site in the source. Two contract items are **shim-MUST for the
wasmtime reference shim but MAY for browser glue**, and this glue takes the MAY (omits them):

- **§5.3-2′ section whitelist / start-section ban** — the wasmtime shim always rejects modules
  with a start section or any section outside the {custom, type, import, function, export, code}
  whitelist. Browser glue MAY skip this (§5.2/§5.3-2); a page wanting the hardening can add a
  raw-byte section walk without changing the contract.
- **§5.3-4 export "f" type check** — MUST only where the host can introspect types; plain JS has
  no type reflection here, so presence-only is checked and a signature mismatch surfaces as a
  deterministic trap on first call (§2.4 discard path — the contract explicitly accepts this).

Everything else is implemented in its normative form: fresh memory view per call (grow-detach,
§5.3-1), synchronous compile (§5.3-2), shared memory + table wiring (§5.3-3), one-step
grow-with-init table registration — no null-funcref window (§5.3-5), add-only instance keep-alive
(§5.3-6), no guest-memory writes and no re-entry into any calling-instance export (§5.3-7/§13-4),
BigInt i64 return (§5.3-8), §5.1 negative error codes (sign normative; `-5n` reserved — produced
only if `bind()` was never called, i.e. wired-but-not-operational).

## Main-thread sync-compile caveat (8 MB)

Chrome limits main-thread synchronous `new WebAssembly.Module()` to **8 MB since Chrome 115**
(raised from 4 KB — an extension, NOT an abolition; design §5.4 correction, panel C#2).
Firefox/Safari have no limit. otter's per-function emitted modules are hundreds of bytes to a few
KB, so this never binds in practice; if a host still rejects on size it returns `-1` and the
engine degrades that function to sticky-Refused permanent T0 (§5.1) — correct, just slower.

## Verification status

Per design §8(c), the browser glue is **not a CI gate**: browser verification is manual and
documented here when performed (none yet as of W-6 — the contract itself is fully gated by the
native `just wasm-e2e` acceptance run through `otter-host-wasmtime`, which implements the same
§5.3 steps). This file is syntax-checked with `node --check otter-glue.mjs` (part of the W-6
gate). Note the engine side is host-agnostic: everything the E2E proves about the contract
(sticky Refusal on negative codes, exactly-one compile request per hot function, add-only table
discipline) applies unchanged under this glue.
