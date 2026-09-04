# otter-host-wasmtime

O-13 §5.5 **reference host** for the `otter_jit` dynamic-compilation contract (design doc
`.claude-research-library/otter-o13-wasm-emit-design.md` §5): a native wasmtime-48 embedding that
runs an otter `wasm32-wasip1` guest with real WASI preview1 wiring plus the
`otter_jit.compile(ptr, len) -> i64` import. This is the W-5 product-quality successor to the W-0
spike host (`spikes/o13-w0-jit-contract/host`, kept verbatim as the committed measurement); it is
what `just wasm-e2e` (repo root) drives the §8(c) acceptance suite through.

## Usage

```sh
otter-host-wasmtime [--reject-compiles] [--no-otter-jit] <guest.wasm> [guest args...]
```

- Trailing args are forwarded to the guest verbatim as WASI argv (argv[0] = the guest path).
- The guest's `proc_exit` code is propagated as this process's exit code.
- `--reject-compiles` — §8(c)-5 acceptance mode: every `otter_jit.compile` request is answered
  with `-1` (nothing is ever compiled); the engine must degrade to sticky-Refused permanent T0.
- `--no-otter-jit` — §8(c)-6 acceptance mode: the `otter_jit` import is not wired at all,
  modeling a plain WASI runtime. A feature-off guest runs as pure T0; a feature-ON guest fails
  instantiation with a precise unresolved-import error (that failure IS the §5.1 contract).
- `--p3-bridge` — O-16f mode, **only in a `--features p3-bridge` build** (a stock build refuses
  the flag loudly): instantiates a p3-world demo component (WASI 0.3.0 clocks+random via
  `wasmtime_wasi::p3::add_to_linker`) in the same synchronous Store and wires
  `otter_p3.fetch_stamp` as a sync bridge from the core guest to the component's `stamp` export.
  Extra exit lines: `HOST-BRIDGE-CALLS=<n>` + the `HOST-P3-STORE-SYNC-OK` latch probe (§5.5 sync
  norm held for the whole mixed run). Gate: `just wasm-e2e-p3`; design note:
  `.claude-research-library/otter-o16f-p3-bridge.md`. The bridge surface is whitelisted to
  **sync-WIT p3 functions** — async-WIT/stream p3 interfaces would latch the Store out of the
  sync norm (measured; see the design note §5) and are out of scope by contract.

On exit (machine-greppable, consumed by `just wasm-e2e`):

```
HOST-COMPILE-REQUESTS=<n>   # otter_jit.compile invocations (§8(c)-5: exactly one per hot fn)
HOST-COMPILE-COUNT=<n>      # successful compile→instantiate→table-register round trips
```

The two counters split the §8(c)-5 evidence: under `--reject-compiles`, `COUNT` must be `0` while
`REQUESTS` equals the number of hot functions (one sticky-refused request each — no retry loop);
on a normal run the two are equal (each promoted function compiled exactly once).

## Contract compliance summary (§5.3 normative steps ↔ implementation)

All in `src/main.rs::otter_jit_compile`, labeled `§5.3-1` … `§5.3-8` in source:

| §5.3 step | implementation | error |
|---|---|---|
| 1 fresh-view byte read | `Caller::get_export("memory")` each call (never a cached base pointer; W-0 M4 main path) + `Memory::read` of `[ptr, ptr+len)` | read failure / missing memory export → `-1` (A#5) |
| 2 compile (validated) | synchronous `Module::new` | failure/limits → `-1` |
| 2′ start-section ban | raw-byte section walk AFTER `Module::new` accepted them; any module with a start section (id 8) is rejected — the shim-MUST half of §5.2 (W-0 disposition A#2: the general MAY is pinned to always-reject in this reference implementation) | → `-1` |
| 3 wire memory/table + instantiate | caller's exported `memory`/`__indirect_function_table` mapped to `otter.memory`/`otter.table`; **synchronous, reentrant** `Instance::new` (guest frame live) | missing table export / unexpected import / instantiation failure → `-2` |
| 4 export "f" MUST type check | `get_func("f")`, then **pattern matching** on params/results `(i32,i32,i64)->i64` — wasmtime's `ValType` has no `PartialEq` (W-0 R#3; measured on 45, still true on 48) | missing → `-3`, mismatch → `-3` |
| 5 table registration | **grow-with-init in one step** — `Table::grow(store, 1, Ref::Func(Some(f)))` (W-0 disposition A#7): the new slot is born holding `f`, no null-funcref window, "write only to the freshly grown slot" holds constructively | grow failure → `-4` |
| 6 add-only keep-alive | emitted `Instance` pushed to a store-data `Vec`, never dropped, slots never rebound | — |
| 7 no guest-memory writes | the implementation never writes guest linear memory; **§13-4 MUST NOT**: it never calls any export of the calling instance — only the step-1 read and the step-5 table grow touch shared state (stated at the function's doc, load-bearing for the engine's scratch/borrow argument) | — |
| 8 return index | `idx as i64 > 0` (defensive: slot 0 / non-i64-representable → `-4`, never `0`) | — |

Error-code discipline (§5.1/A#6): the **sign** is normative; the specific negative value is a
diagnostic SHOULD — every failure path here also prints the underlying wasmtime error verbatim to
stderr, including the two documented embedder-link-flag failure signatures (missing
`--export-table` → `-2` at step 3's premise, missing `--growable-table` → `-4` at first tier-up;
W-0 A#1).

## Synchronous-Store-only (W-0 disposition A#4 — §5.5 norm)

Everything is the sync API: `Store::new`, `Linker::func_wrap`, `Instance::new`,
`p1::add_to_linker_sync`; `Config::async_support` is never enabled. An async embedder must
reimplement the contract with `func_wrap_async`/`Instance::new_async` — reusing sync
`Instance::new` in an async store bails, surfacing as every compile returning `-2` (silent full-T0
degradation). Note `wasmtime-wasi`'s `p1` feature chain enables wasmtime's `async`/
`component-model` **compile-time** features transitively; that is API surface only — the runtime
configuration here stays fully synchronous.

48-family precision (O-16f measurement; norm wording update proposed in the design note §5):
`Config::async_support` is a deprecated no-op on 48 — the real mechanism is the **per-Store
one-way `async_required` latch**, tripped only when an async-flavored element is actually linked
into the Store. The operative norm is therefore "never trip the latch"; the `--p3-bridge` mode
asserts it after every run (`HOST-P3-STORE-SYNC-OK`).

## EH configuration

`Config::wasm_exceptions(true)` with crate features `gc` + `gc-drc`. On family 48 exceptions are
**on by default** when `gc` is compiled in (default-on since 47; measured on 48:
`WasmFeatures::EXCEPTIONS` defaults to `cfg!(feature = "gc")`), so the explicit call is no longer
load-bearing the way it was on pin 45 (off-by-default there — omission broke exnref parsing, W-0
M2 measured). It is **retained deliberately** per design §5.5: it pins the requirement against
future upstream default flips and documents that this host owes the guest EH acceptance. `gc` +
`gc-drc` stay required (`wasm_exceptions` is `gc`-feature-gated; `gc` alone has no collector —
W-0 A#3). Collector selection on 48: `Collector::Auto` resolves via the compiled-in fallback
chain `gc-copying` → `gc-drc` → `gc-null`, so with `gc-drc` only it picks the DRC collector —
the same collector as pin 45 (`gc-copying` is deliberately not enabled; upstream documents the
copying collector as not yet functional).

## Version pin (host train — O-16a)

The wasmtime pin is **family 48** (`wasmtime` + `wasmtime-wasi` in lockstep with each other).
This is the HOST train, bumped independently of the workspace's cranelift lockstep (design
§5.5/§7.1): `crates/otter-wasmemit`'s dev-dep wasmtime 45 rides the cranelift-0.132 transitive
pin and stays there until a cranelift bump. This crate is workspace-excluded with its own
lockfile, so the divergence is by design. 47's removal of wasi-threads/`wasi-common` is a no-op
here (zero deps on either — verified at the bump).

## No anyhow (W-0 dispositions R#3/C#6)

wasmtime ships its own `Error`/`Result`/`Context`/`bail` (measured on 45, unchanged on 48;
`func_wrap` closures return
`Result<T, wasmtime::Error>`); this crate has no anyhow dependency — closing out the spike host's
leftover `anyhow` line at the product level.

## Building / running the E2E

```sh
cd hosts/otter-host-wasmtime && cargo build --locked --release   # repo-pinned stable 1.96.0
just wasm-e2e                                                    # repo root: full §8(c) run
```

This crate is workspace-excluded (empty `[workspace]` table) for the same reason as
`demos/otter-wasm-demo`: its heavy native dependency tree (wasmtime + wasmtime-wasi) must not
ride into `cargo build/test --workspace`.
