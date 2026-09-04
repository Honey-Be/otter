// otter-glue.mjs — O-13 §5.4 browser JS glue for the `otter_jit` dynamic-compilation contract.
//
// Reference implementation of the SAME contract `hosts/otter-host-wasmtime` implements natively
// (design doc: .claude-research-library/otter-o13-wasm-emit-design.md §5.1-§5.4). Per §8(c) this
// glue is NOT a CI gate — browser verification is manual, documented in the README next to this
// file; the file itself is syntax-checked (`node --check`) only.
//
// §5.3 normative-step compliance map (comments at each site):
//   §5.3-1 fresh view        — bytes are read through a NEW Uint8Array over `memory.buffer` on
//                              EVERY call, then `.slice()`d: `memory.grow` detaches the old JS
//                              ArrayBuffer, so caching a view (or the buffer) is forbidden (§5.4).
//   §5.3-2 sync compile      — `new WebAssembly.Module(bytes)` (synchronous, §5.4). Main-thread
//                              caveat: Chrome limits sync compile to 8 MB since 115 (4 KB before —
//                              raised, NOT abolished); Firefox/Safari have no limit. Per-function
//                              emitted modules are hundreds of bytes to a few KB, so this is
//                              headroom, but a host MAY reject on size with -1 and the engine
//                              treats it as a sticky Refusal (§5.1 degradation path).
//   §5.3-2′ section whitelist / start-section ban — shim MUST for the wasmtime REFERENCE shim,
//                              but MAY for browser glue (§5.2/§5.3-2), and this glue OMITS it:
//                              the emitted-module producer is the in-sandbox backend, and a
//                              browser page that wants the hardening can add a raw-byte section
//                              walk here without changing the contract.
//   §5.3-3 wire + instantiate — imports = { otter: { memory, table } } from the main instance's
//                              exports; failure → -2n.
//   §5.3-4 type check        — MUST only where the host CAN introspect types; plain JS glue
//                              cannot (no type reflection assumed), so this glue takes the MAY:
//                              a signature mismatch surfaces as a deterministic trap on first
//                              call, converging on the §2.4 discard path.
//   §5.3-5 table registration — one-step grow-with-init `table.grow(1, f)`: the new slot is born
//                              holding `f`, no null-funcref window, and "write only to the
//                              freshly grown slot" holds constructively (W-5/W-0 A#7 form).
//   §5.3-6 add-only           — every emitted Instance is pushed to `keepAlive` and never
//                              released; table slots are never rebound.
//   §5.3-7 no writes / no re-entry — this function never writes guest linear memory, never
//                              passes the scratch address anywhere, and (§13-4 MUST NOT) never
//                              calls ANY export of the calling instance: only the step-1 byte
//                              read and the step-5 table grow touch shared state.
//   §5.3-8 return             — the table index as a BigInt (the import's wasm type is i64).
//
// Error codes (§5.1 — the SIGN is normative, the specific value a diagnostic SHOULD):
//   -1n compile/validation failure (host size limits included), -2n instantiation failure,
//   -3n export "f" missing, -4n table.grow failure, -5n feature wired-but-not-operational —
//   this glue produces it in exactly one window: `bind()` was never called (imports wired, no
//   main instance to serve). A fully UNWIRED host is different: it omits the import entirely,
//   which fails the main module's instantiation = the feature-off contract (§5.1).
//   The engine treats every value outside 0 < idx <= u32::MAX as a sticky Refusal (§5.1).

/**
 * Build the `otter_jit` import object for an otter main module (wasm32, feature `wasm-emit`).
 *
 * Late-binding: the import object must exist BEFORE the main instance does, so the main
 * instance is supplied afterwards via `bind(inst)`. Usage:
 *
 *   const jit = makeOtterJit();
 *   const { instance } = await WebAssembly.instantiate(mainBytes, {
 *     ...wasiImports,                // the embedder's WASI (or other) imports
 *     otter_jit: jit.imports.otter_jit,
 *   });
 *   jit.bind(instance);             // REQUIRED before guest code first tiers up
 *
 * The main module must be linked with `--export-table --growable-table` (§5.3 embedder build
 * requirements); missing flags surface as -2n (no table export) or -4n (non-growable table,
 * deferred to the first tier-up) — both documented in §5.1.
 */
export function makeOtterJit() {
  /** @type {WebAssembly.Instance | null} */
  let mainInst = null;
  /** §5.3-6: add-only keep-alive store — emitted instances are NEVER released. */
  const keepAlive = [];

  const imports = {
    otter_jit: {
      /**
       * §5.1 `otter_jit.compile(ptr: u32, len: u32) -> i64`.
       * @param {number} ptr  wasm i32 param (arrives sign-extended; normalized with >>> 0)
       * @param {number} len  wasm i32 param (same)
       * @returns {bigint}    table index (> 0n) or a negative error code
       */
      compile(ptr, len) {
        if (mainInst === null) return -5n; // bind() not called: wired but not operational
        const p = ptr >>> 0; // u32 normalization (i32 params arrive as signed Numbers)
        const n = len >>> 0;

        // §5.3-1: fresh view EVERY call (memory.grow detaches the old buffer — never cache),
        // then copy out, so no live view outlasts this synchronous function.
        let bytes;
        try {
          const memory = mainInst.exports.memory;
          bytes = new Uint8Array(memory.buffer, p, n).slice();
        } catch (_e) {
          return -1n; // out-of-bounds ptr/len or missing memory export
        }

        // §5.3-2: SYNCHRONOUS compile (see the 8 MB main-thread caveat in the header comment).
        let mod;
        try {
          mod = new WebAssembly.Module(bytes);
        } catch (_e) {
          return -1n; // CompileError, or the browser's size-based sync-compile rejection
        }

        // §5.3-3: wire the SHARED memory + funcref table, instantiate (synchronously —
        // instantiation runs while guest code is suspended in this import; §13-4: nothing here
        // calls back into any export of the calling instance).
        let inst;
        try {
          inst = new WebAssembly.Instance(mod, {
            otter: {
              memory: mainInst.exports.memory,
              table: mainInst.exports.__indirect_function_table,
            },
          });
        } catch (_e) {
          return -2n; // LinkError etc. — incl. the missing `--export-table` signature
        }

        // §5.3-4 (MAY arm): no type reflection — a mismatched "f" traps deterministically on
        // first call instead (§2.4). Only presence is checked.
        const f = inst.exports.f;
        if (typeof f !== "function") return -3n;

        // §5.3-5: one-step grow-with-init — no null-slot window, never rebinds existing slots.
        let idx;
        try {
          const table = mainInst.exports.__indirect_function_table;
          idx = table.grow(1, f); // returns the PREVIOUS size = the new slot's index
        } catch (_e) {
          return -4n; // non-growable table (missing `--growable-table`) or resource exhaustion
        }

        keepAlive.push(inst); // §5.3-6: add-only, permanent
        return BigInt(idx); // §5.3-8 (idx > 0 always: slot 0 predates any grow and stays null)
      },
    },
  };

  return {
    imports,
    /** Late-bind the instantiated main module. Call once, before guest code first tiers up. */
    bind(instance) {
      mainInst = instance;
    },
    /** Diagnostic only: how many emitted instances are being kept alive (add-only, §11 risk 11). */
    emittedCount() {
      return keepAlive.length;
    },
  };
}
