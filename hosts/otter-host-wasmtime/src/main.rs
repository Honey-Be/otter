//! O-13 §5.5 **reference host** for the `otter_jit` dynamic-compilation contract
//! (design doc `.claude-research-library/otter-o13-wasm-emit-design.md` §5).
//!
//! ```text
//! otter-host-wasmtime [--reject-compiles] [--no-otter-jit] <guest.wasm> [guest args...]
//! ```
//!
//! - WASI preview1 is wired through `wasmtime-wasi` (`p1::add_to_linker_sync`), so the guest is a
//!   plain wasip1 command module; trailing CLI args are forwarded to it verbatim (argv[0] = the
//!   guest path). The guest's `proc_exit` code is propagated as this process's exit code.
//! - `otter_jit.compile(ptr, len) -> i64` implements the §5.3 normative procedure to the letter —
//!   the eight steps are labeled `§5.3-1` .. `§5.3-8` in [`otter_jit_compile`] below.
//! - `--reject-compiles` (§8(c)-5 acceptance): every compile request is answered with `-1`
//!   without compiling anything — the engine must degrade to sticky-`Refused` permanent T0, and
//!   the request counter proves "exactly one compile call per hot function, no retry loop".
//! - `--no-otter-jit` (§8(c)-6 acceptance): the `otter_jit` import is NOT wired at all, modeling
//!   a WASI runtime **with wasm exception-handling support** (a §7-built guest carries exnref
//!   `try_table`/`throw_ref` even feature-off, so "any WASI runtime" always means EH-capable;
//!   on wasmtime 48 that is `Config::wasm_exceptions(true)` — default-on since 47, still set
//!   explicitly per design §5.5). A feature-off guest (no
//!   `otter_jit` import) must instantiate and run as pure T0; a feature-ON guest correctly fails
//!   instantiation (§5.1 — that is the contract).
//!
//! On exit the host prints (stdout, machine-greppable — consumed by `just wasm-e2e`):
//!
//! ```text
//! HOST-COMPILE-REQUESTS=<n>   # otter_jit.compile invocations (§8(c)-5: exactly one per hot fn)
//! HOST-COMPILE-COUNT=<n>      # successful compile→instantiate→table-register round trips
//! ```
//!
//! The two counters split the §8(c)-5 evidence: under `--reject-compiles`, `COUNT` must be 0
//! (nothing was ever compiled) while `REQUESTS` must equal the number of functions that crossed
//! the tier-up threshold exactly once each (sticky refusal ⇒ no retries); on a normal run the two
//! are equal (every request succeeded, once per function).
//!
//! **Shared-memory guests (O-17 L0, design §5.2 v1.1)**: guests built under the §7 atomics
//! recipe export a SHARED linear memory. This host accepts them via the wasmtime `threads`
//! crate feature (validation) + `Config::shared_memory(true)` (instantiation) + the
//! `Extern::SharedMemory` branches in [`otter_jit_compile`] (read + wiring). The single-agent
//! norm is unchanged — nothing here ever runs guest code concurrently.
//!
//! **Synchronous-Store-only** (W-0 disposition A#4, §5.5 norm): everything here is the sync API —
//! `Store::new`, `Linker::func_wrap`, `Instance::new`, `p1::add_to_linker_sync` — and
//! `Config::async_support` is never enabled. An async embedder must reimplement this contract
//! with `func_wrap_async`/`Instance::new_async`; reusing this shim's sync `Instance::new` inside
//! an async store bails, surfacing as every compile returning `-2` (silent full-T0 degradation).
//!
//! **No anyhow** (W-0 disposition R#3/C#6): wasmtime provides its own
//! `Error`/`Result`/`Context`/`bail` (measured on 45, unchanged on 48); this crate has no anyhow
//! dependency.
//!
//! **p3-bridge mode (O-16f, cargo feature `p3-bridge` + CLI `--p3-bridge`)**: the mode-doc §1.1
//! path-12 productization — the same synchronous `Store<Host>` additionally hosts a p3-world
//! demo COMPONENT (imports `wasi:clocks/monotonic-clock@0.3.0` + `wasi:random/random@0.3.0`,
//! served by `wasmtime_wasi::p3::add_to_linker`), and the core guest gets one extra import,
//! `otter_p3.fetch_stamp(dst: u32) -> i32 (status)`, whose host closure calls the component's
//! `stamp: func() -> u64` export re-entrantly (guest frame on the stack, same Store) and writes
//! the value LE at `dst` in guest linear memory. The `otter_jit` §5.1/§5.3 contract is UNCHANGED
//! and runs in the same Store/run (`just wasm-e2e-p3` drives tier-up between bridge calls). The
//! bridge surface is deliberately restricted to **sync-WIT p3 functions** — the S2 spike measured
//! that async-WIT imports (stream/`wait-for`-class) latch the Store out of the §5.5 sync norm;
//! the post-run `HOST-P3-STORE-SYNC-OK` probe asserts on every run that the latch was never
//! tripped. See `.claude-research-library/otter-o16f-p3-bridge.md` and [`p3_bridge`].

use std::process::ExitCode;

use wasmtime::error::Context as _;
use wasmtime::{Caller, Config, Engine, Extern, Instance, Linker, Module, Ref, Result, Store, ValType, bail};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::{I32Exit, WasiCtxBuilder};

const USAGE: &str = "usage: otter-host-wasmtime [--reject-compiles] [--no-otter-jit] [--p3-bridge] <guest.wasm> [guest args...]\n  (--p3-bridge needs a host built with --features p3-bridge — O-16f)";

/// Per-run `otter_jit` state, stored in the `Store` data next to the WASI context.
#[derive(Default)]
struct JitState {
    /// §5.3-6: add-only keep-alive of every emitted instance — instances are NEVER dropped and
    /// table slots are never rebound while the store lives (matches the engine's add-only
    /// invariant; freeing emitted code does not exist in the v1 contract).
    emitted: Vec<Instance>,
    /// Total `otter_jit.compile` invocations (successful or not). §8(c)-5's "exactly once per
    /// function, no retry loop" is asserted against this by the E2E recipe.
    requests: u64,
    /// Successful §5.3 round trips (a table index was actually handed out).
    compiled: u64,
    /// `--reject-compiles`: answer every request with `-1` (acceptance §8(c)-5).
    reject: bool,
}

/// Store data: WASI preview1 context + the jit contract state (+ the O-16f bridge state when
/// the `p3-bridge` feature is compiled in — one WASI ctx backs BOTH the p1 core wiring and the
/// p3 component wiring, the p1-on-p2 precedent measured at the type level in S2).
struct Host {
    wasi: WasiP1Ctx,
    jit: JitState,
    #[cfg(feature = "p3-bridge")]
    bridge: p3_bridge::BridgeState,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("otter-host-wasmtime: {e:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    // ---- CLI ------------------------------------------------------------------------------
    let mut reject = false;
    let mut no_otter_jit = false;
    #[cfg(feature = "p3-bridge")]
    let mut p3_bridge_on = false;
    let mut guest_path: Option<String> = None;
    let mut guest_args: Vec<String> = Vec::new();
    for arg in std::env::args().skip(1) {
        if guest_path.is_some() {
            guest_args.push(arg); // everything after the guest path is forwarded verbatim
        } else {
            match arg.as_str() {
                "--reject-compiles" => reject = true,
                "--no-otter-jit" => no_otter_jit = true,
                // O-16f: the flag is recognized ONLY when the feature is compiled in; a stock
                // build refuses it loudly (never silently runs bridge-less — the bridge guest
                // would then fail instantiation with a confusing unresolved-import error).
                "--p3-bridge" => {
                    #[cfg(feature = "p3-bridge")]
                    {
                        p3_bridge_on = true;
                    }
                    #[cfg(not(feature = "p3-bridge"))]
                    bail!(
                        "--p3-bridge requires a host built with --features p3-bridge \
                         (O-16f; wasmtime-wasi p3 compile-time opt-in)\n{USAGE}"
                    );
                }
                // Panel #3 C#4: an unknown `--*` before the guest path is a usage error, not a
                // guest path — a typo like `--reject-compiles` must not silently run the guest in
                // normal mode (flags AFTER the guest path still forward verbatim, first branch).
                s if s.starts_with("--") => bail!("unknown flag {s}\n{USAGE}"),
                _ => guest_path = Some(arg),
            }
        }
    }
    let guest_path = guest_path.context(USAGE)?;

    // ---- Engine/module --------------------------------------------------------------------
    // §5.5 EH config: on family 48 exceptions are ON by default when the `gc` feature is
    // compiled in (default-on since 47; measured on 48: WasmFeatures::EXCEPTIONS defaults to
    // cfg!(feature = "gc") in upstream config.rs) — so this line is no longer load-bearing the
    // way it was on pin 45 (where exceptions were off-by-default and omission broke exnref
    // parsing, W-0 M2 measured). It is RETAINED DELIBERATELY per design §5.5's rule that the
    // explicit setting stays even after the 47+ default flip: it pins the requirement against any future
    // upstream default change and documents that this host owes the guest EH acceptance.
    // Requires crate features `gc` + `gc-drc` (Cargo.toml — the API is `gc`-gated on 48).
    // NOTE: Config::async_support is deliberately never called — synchronous Store only (A#4).
    let mut config = Config::new();
    config.wasm_exceptions(true);
    // O-17 L0 (design §5.2 v1.1): accept guests built with a SHARED linear memory (the §7
    // atomics recipe). Two-layer requirement, both measured on 48: (1) the crate feature
    // `threads` (Cargo.toml) gates VALIDATION of atomics + shared memories (default-ON once
    // compiled in — no wasm_threads() call needed); (2) this call gates INSTANTIATION —
    // Config::shared_memory is a long-standing default-FALSE knob (present with the same
    // signature and default at least since 45 — NOT new in 48; upstream docs: shared-memory
    // support "has enough deficiencies" to ship disabled; threads is tier-2, no
    // CVEs issued — recorded in the release security-gate list, O-17 note), and without it a
    // shared guest passes validation but fails Linker::instantiate with "shared memory support
    // is disabled for this engine". Single-agent norm unchanged: the engine never runs
    // concurrently (§5.2 v1.1) — this host merely LINKS shared memories, it never spawns.
    config.shared_memory(true);
    let engine = Engine::new(&config)?;
    let module = Module::from_file(&engine, &guest_path)
        .with_context(|| format!("loading guest module {guest_path}"))?;

    // ---- Linker ---------------------------------------------------------------------------
    let mut linker: Linker<Host> = Linker::new(&engine);
    // Real WASI preview1 (design §5.5 W-5): the full witx surface, synchronous bindings.
    p1::add_to_linker_sync(&mut linker, |h: &mut Host| &mut h.wasi)?;

    if no_otter_jit {
        // §8(c)-6 mode: model a plain WASI runtime — the import is simply absent. A feature-off
        // guest never imports `otter_jit` and runs as T0; a feature-ON guest fails instantiation
        // below with a precise unresolved-import error (that failure IS the §5.1 contract).
        eprintln!("host: --no-otter-jit: otter_jit.compile NOT wired (plain-WASI-runtime mode)");
    } else {
        linker.func_wrap(
            "otter_jit",
            "compile",
            |mut caller: Caller<'_, Host>, ptr: u32, len: u32| -> i64 {
                caller.data_mut().jit.requests += 1;
                if caller.data().jit.reject {
                    // §8(c)-5 acceptance mode: refuse everything. `-1` is the §5.1 "validation/
                    // compile failure (including host-implementation limits)" code — the engine
                    // must install sticky Refused and never ask again for this function.
                    let n = caller.data().jit.requests;
                    eprintln!("host: --reject-compiles: refusing compile request #{n} with -1");
                    return -1;
                }
                let idx = otter_jit_compile(&mut caller, ptr, len);
                if idx > 0 {
                    caller.data_mut().jit.compiled += 1;
                }
                idx
            },
        )?;
    }
    // O-16f: the bridge import rides in the SAME core Linker (same Store type parameter — the
    // S2-measured coexistence). Wired only under the flag: without it a bridge guest fails
    // instantiation with a precise unresolved `otter_p3` import error (the wasm-e2e-p3 recipe's
    // negative probe), and stock guests never see the name at all.
    #[cfg(feature = "p3-bridge")]
    if p3_bridge_on {
        p3_bridge::wire_bridge_import(&mut linker)?;
    }

    // ---- Store + instantiate + run --------------------------------------------------------
    let wasi = WasiCtxBuilder::new()
        .inherit_stdio()
        .arg(&guest_path) // argv[0]
        .args(&guest_args)
        .build_p1();
    let mut store: Store<Host> = Store::new(
        &engine,
        Host {
            wasi,
            jit: JitState {
                reject,
                ..JitState::default()
            },
            #[cfg(feature = "p3-bridge")]
            bridge: p3_bridge::BridgeState::default(),
        },
    );
    // O-16f: instantiate the p3-world demo component FIRST, in the same synchronous Store the
    // guest is about to run in (S2-E1 order (b); the component's export handle lands in the
    // Store data so the bridge closure can reach it via Caller at guest run time).
    #[cfg(feature = "p3-bridge")]
    if p3_bridge_on {
        p3_bridge::instantiate_demo_component(&engine, &mut store)?;
    }
    let instance = linker.instantiate(&mut store, &module).context(
        "instantiating guest (a feature-ON guest under --no-otter-jit fails here by design — §5.1)",
    )?;
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .context("guest lacks a wasip1 command export `_start`")?;
    let outcome = start.call(&mut store, ());

    // Counters print on every path that reached the guest (E2E recipe greps these).
    let jit = &store.data().jit;
    println!("HOST-COMPILE-REQUESTS={}", jit.requests);
    println!("HOST-COMPILE-COUNT={}", jit.compiled);
    // O-16f: bridge accounting + the §5.5 sync-norm latch probe (both grepped by wasm-e2e-p3).
    #[cfg(feature = "p3-bridge")]
    if p3_bridge_on {
        println!("HOST-BRIDGE-CALLS={}", store.data().bridge.calls);
        p3_bridge::assert_store_still_sync(&engine, &mut store)?;
    }

    match outcome {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            // Guest exit-code propagation: wasip1 `proc_exit(n)` surfaces as an I32Exit error.
            if let Some(I32Exit(n)) = e.downcast_ref::<I32Exit>() {
                if *n == 0 {
                    return Ok(ExitCode::SUCCESS);
                }
                eprintln!("host: guest exited with code {n}");
                return Ok(ExitCode::from(u8::try_from(*n).unwrap_or(1)));
            }
            eprintln!("host: guest trapped/errored: {e:?}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The §5.3 normative procedure, steps 1-8 in order. Returns a shared-funcref-table index (`> 0`)
/// or a §5.1 negative error code; every failure path prints the underlying cause verbatim to
/// stderr (§5.1 A#6: the SIGN is normative, the specific negative value is a diagnostic SHOULD —
/// this shim implements the full documented split).
///
/// **§13-4 / §5.3-7 (MUST NOT — load-bearing for the engine's scratch/borrow argument)**: this
/// function never calls ANY export of the calling instance. Its only interactions with the guest
/// are (a) READING `[ptr, ptr+len)` from the exported memory (step 1) and (b) growing the shared
/// funcref table with the new function as the init value (step 5). It never writes guest linear
/// memory, and no guest code can run inside this call (together with §5.2's allowed-section
/// whitelist — enforced in step 2, which bans start/element/data sections and thereby every
/// instantiation-time side effect — the guest stack above us stays frozen and the shared
/// memory/table stay untouched for the whole call).
fn otter_jit_compile(caller: &mut Caller<'_, Host>, ptr: u32, len: u32) -> i64 {
    // ---- §5.3-1: read the emitted bytes from the caller's exported memory, via a FRESH view
    // (Caller::get_export each call — never a cached data pointer; memory.grow moves the base).
    // W-0 M4: get_export reaches both exports directly (measured on wasmtime 45, unchanged on
    // 48) — no Store-data fallback.
    // O-17 L0 (§5.3-1 + §5.3-3 shared branch): a SHARED guest linear memory surfaces as
    // `Extern::SharedMemory`, NOT `Extern::Memory` (measured on 48) — `into_memory` alone would
    // misreport a §7-atomics guest as "no exported memory". Both arms are handled everywhere the
    // memory is touched: size check, byte read (below) and otter.memory wiring (§5.3-3).
    enum GuestMem {
        Plain(wasmtime::Memory),
        Shared(wasmtime::SharedMemory),
    }
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => GuestMem::Plain(m),
        Some(Extern::SharedMemory(m)) => {
            // stderr marker consumed by `just wasm-e2e-atomics` (proves the whole run really
            // exercised the shared path, not a silently non-shared rebuild).
            eprintln!("host: HOST-SHARED-MEMORY guest \"memory\" export is shared (O-17 L0 path)");
            GuestMem::Shared(m)
        }
        _ => {
            eprintln!("host: compile: caller has no exported \"memory\" — cannot read emitted bytes (-1)");
            return -1;
        }
    };
    let Some(tbl) = caller
        .get_export("__indirect_function_table")
        .and_then(Extern::into_table)
    else {
        // §5.3-3 wiring is impossible without the shared table. Most likely cause (§5.3 embedder
        // build requirements, W-0 A#1): the final-artifact link lacked --export-table.
        eprintln!(
            "host: compile: caller has no exported \"__indirect_function_table\" (-2) — \
             was the guest linked with -C link-arg=--export-table -C link-arg=--growable-table?"
        );
        return -2;
    };
    // Bounds check BEFORE the copy-buffer allocation (panel #3 lens B#3): `len` is guest-
    // controlled, and `vec![0u8; len]` first would let a hostile `compile(0, 0xFFFF_FFFF)` drive
    // a 4 GiB host allocation — an abort (SIGABRT) under memory pressure — for a request the
    // read below would reject anyway. Fail -1 on any [ptr, ptr+len) not inside the CURRENT
    // memory size; only then allocate.
    let size = match &mem {
        GuestMem::Plain(m) => m.data_size(&*caller),
        GuestMem::Shared(m) => m.data_size(),
    };
    let in_bounds = (ptr as u64)
        .checked_add(len as u64)
        .is_some_and(|end| end <= size as u64);
    if !in_bounds {
        eprintln!(
            "host: compile: [ptr={ptr}, +{len}) exceeds guest memory size {size} — \
             rejected before allocation (-1)"
        );
        return -1;
    }
    let mut bytes = vec![0u8; len as usize];
    match &mem {
        GuestMem::Plain(m) => {
            if let Err(e) = m.read(&mut *caller, ptr as usize, &mut bytes) {
                // Out-of-range [ptr, ptr+len) read → -1 (W-0 A#5: folded into the -1 class;
                // defense in depth behind the pre-allocation bounds check above).
                eprintln!("host: compile: reading [ptr={ptr}, +{len}) from guest memory failed: {e:?} (-1)");
                return -1;
            }
        }
        GuestMem::Shared(m) => {
            // O-17 L0: SharedMemory exposes its bytes as &[UnsafeCell<u8>] (no read() helper).
            // The single-agent norm (§5.2 v1.1: the engine NEVER runs concurrently, and §5.3-7/
            // §13-4 freeze the guest stack above us for the whole call) makes this per-byte copy
            // race-free — there is no other thread that could write. Bounds: the pre-allocation
            // check above proved ptr+len <= data_size, and a shared memory can never SHRINK, so
            // indexing stays in range. SAFETY: reading through UnsafeCell with no concurrent
            // writer (single-agent norm) and no live Rust borrow of the same bytes.
            let data = m.data();
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = unsafe { *data[ptr as usize + i].get() };
            }
        }
    }

    // ---- §5.3-2: compile (validation included). Failure/limits → -1.
    let module = match Module::new(caller.engine(), &bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("host: compile: Module::new on emitted bytes failed: {e:?} (-1)");
            return -1;
        }
    };
    // §5.2 allowed-section whitelist, shim-MUST (W-0 disposition A#2, widened by panel #3 lens
    // B#1/B#2): instantiation (step 3) must be side-effect-free on the shared resources, so ANY
    // section outside the §5.2 emitted-module shape rejects with -1 — not just start(8). An
    // active element segment would make Instance::new silently REBIND pre-existing slots of the
    // shared funcref table (§5.3-5 "new slot only" violation), and an active data segment would
    // WRITE guest linear memory (§5.3-7 MUST violation); datacount(12) only rides with data. The
    // backend already emits none of these (§5.2); this is the host-side enforcement the
    // reference shim owes the contract. Scanned on the raw bytes AFTER Module::new accepted them
    // (framing known well-formed; residual scan trouble rejects, fail-closed).
    if !emitted_sections_whitelisted(&bytes) {
        eprintln!(
            "host: compile: emitted module carries a section outside the §5.2 whitelist \
             (custom/type/import/function/export/code) — rejected (-1, §5.2/§5.3-2 shim MUST)"
        );
        return -1;
    }

    // ---- §5.3-3: instantiate, wiring the caller's memory/table as otter.memory/otter.table.
    // This is a REENTRANT instantiation (the guest is executing right now) and deliberately the
    // synchronous Instance::new — this shim is sync-Store-only (A#4, module doc).
    let mut imports: Vec<Extern> = Vec::new();
    for imp in module.imports() {
        match (imp.module(), imp.name()) {
            // O-17: wire whichever flavor the guest actually exports. The emitted module's
            // declared sharedness (§5.2 v1.1: the engine keys it on cfg!(target_feature =
            // "atomics")) must match, or Instance::new below fails -2 (limits subtyping never
            // coerces shared↔unshared — O-17 M5) and the engine degrades to sticky-Refused T0.
            ("otter", "memory") => imports.push(match &mem {
                GuestMem::Plain(m) => Extern::Memory(*m),
                GuestMem::Shared(m) => Extern::SharedMemory(m.clone()),
            }),
            ("otter", "table") => imports.push(Extern::Table(tbl)),
            (m, n) => {
                // §5.2: emitted modules import memory+table ONLY (function imports are zero by
                // design) — anything else cannot be wired and fails instantiation's premise.
                eprintln!("host: compile: emitted module has unexpected import {m}::{n} (-2)");
                return -2;
            }
        }
    }
    let emitted = match Instance::new(&mut *caller, &module, &imports) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("host: compile: instantiating emitted module failed: {e:?} (-2)");
            return -2;
        }
    };

    // ---- §5.3-4: export "f" + the MUST type check, (i32, i32, i64) -> i64 (the wasm32
    // CompiledFn shape, §2.2). W-0 R#3: wasmtime's ValType has no PartialEq (subtyping-aware
    // types; measured on 45, still true on 48) — the check is pattern matching, not ==.
    let Some(f) = emitted.get_func(&mut *caller, "f") else {
        eprintln!("host: compile: emitted module lacks export \"f\" (-3)");
        return -3;
    };
    let ty = f.ty(&*caller);
    let params: Vec<ValType> = ty.params().collect();
    let results: Vec<ValType> = ty.results().collect();
    let ty_ok = matches!(params.as_slice(), [ValType::I32, ValType::I32, ValType::I64])
        && matches!(results.as_slice(), [ValType::I64]);
    if !ty_ok {
        eprintln!("host: compile: export \"f\" has wrong type {params:?} -> {results:?} (-3)");
        return -3;
    }

    // ---- §5.3-5: register in the shared funcref table. W-5 form (W-0 disposition A#7):
    // grow-with-init in ONE step — the new slot is born holding `f`, so no null-funcref window
    // ever exists and "write only to the freshly grown slot" holds constructively. Failure → -4
    // (frequent cause, W-0 A#1: --growable-table missing from the embedder link — a non-growable
    // table links fine and fails only here, on the first tier-up).
    let idx = match tbl.grow(&mut *caller, 1, Ref::Func(Some(f))) {
        Ok(prev_size) => prev_size, // previous size == the newly initialized slot's index
        Err(e) => {
            eprintln!(
                "host: compile: table.grow failed: {e:?} (-4) — non-growable table? \
                 (--growable-table missing from the guest's final link?)"
            );
            return -4;
        }
    };
    // Contract sanity: index 0 is the null slot and may never be handed out (§5.1). Unreachable
    // with a real otter main module (its own table is non-empty), but fail closed, not with 0.
    let Ok(idx_i64) = i64::try_from(idx) else {
        eprintln!("host: compile: table index {idx} does not fit i64 (-4)");
        return -4;
    };
    if idx_i64 == 0 {
        eprintln!("host: compile: table.grow returned slot 0 (null-funcref slot) — refusing (-4)");
        return -4;
    }

    // ---- §5.3-6: add-only keep-alive (never dropped, never rebound).
    caller.data_mut().jit.emitted.push(emitted);

    // ---- §5.3-7: nothing to do — this implementation never writes guest memory (see the
    // function doc's MUST NOT paragraph; the WASI imports that legitimately write guest memory
    // are entirely separate host functions).

    // ---- §5.3-8: return the index.
    idx_i64
}

/// Does this core wasm module consist ONLY of §5.2 emitted-module sections? Pure whitelist walk
/// over the raw section framing (no payload decoding): a §5.2 emitted module has exactly
/// type(1)/import(2)/function(3)/export(7)/code(10) — plus tool-inserted custom(0) sections
/// (e.g. `name`), which are semantically inert. Every other id returns `false` (caller rejects
/// with -1), in particular:
///
/// - **start(8)** — instantiation would run guest-supplied code on top of the frozen guest
///   stack (W-0 disposition A#2, the original ban);
/// - **element(9)** — an ACTIVE element segment makes `Instance::new` silently rebind
///   pre-existing slots of the imported shared funcref table (§5.3-5 "set only the freshly
///   grown slot" violation — panel #3 lens B#1);
/// - **data(11)** — an ACTIVE data segment makes `Instance::new` write into the imported guest
///   linear memory (§5.3-7 "compile never writes guest memory" MUST violation — lens B#2);
/// - **datacount(12)** — only exists to accompany data segments;
/// - table(4)/memory(5)/global(6)/tag(13) — outside the §5.2 shape; nothing legitimate emits
///   them, so fail closed rather than reason about each.
///
/// **Boundary of this defense**: the whitelist seals the INSTANTIATION window only — with no
/// start/element/data sections, `Instance::new` cannot run code, rebind existing table slots, or
/// touch guest memory. What it can NOT seal is CALL-TIME abuse: once a registered `f` is
/// invoked, code inside it could still execute `table.set`/`store` on the imported shared
/// table/memory. That surface is the backend's §13-7 invariant domain (the emitter never
/// produces such instructions for the shared resources) — a host shim has no lever there.
///
/// Called only AFTER `Module::new` accepted the bytes, so the framing is known well-formed — any
/// residual scan irregularity (truncated/over-long LEB, section overrunning the buffer) returns
/// `false` to stay fail-closed (§5.2/A#2).
fn emitted_sections_whitelisted(bytes: &[u8]) -> bool {
    const ALLOWED: [u8; 6] = [0, 1, 2, 3, 7, 10]; // custom/type/import/function/export/code
    if bytes.len() < 8 {
        return false; // no room for magic + version: impossible on validated bytes; fail closed
    }
    let mut p = 8usize; // skip 4-byte magic + 4-byte version (validated by Module::new)
    while p < bytes.len() {
        let id = bytes[p];
        p += 1;
        if !ALLOWED.contains(&id) {
            return false;
        }
        // LEB128 u32 section size.
        let mut size: u64 = 0;
        let mut shift = 0u32;
        loop {
            let Some(&b) = bytes.get(p) else {
                return false; // truncated size field on validated bytes: impossible; fail closed
            };
            p += 1;
            size |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift >= 35 {
                return false; // over-long LEB on validated bytes: impossible; fail closed
            }
        }
        p = match p.checked_add(size as usize) {
            Some(n) if n <= bytes.len() => n,
            _ => return false, // size overruns the buffer: impossible on validated bytes
        };
    }
    true
}

/// O-16f p3-bridge (mode doc §1.1 path 12 productization; design note
/// `.claude-research-library/otter-o16f-p3-bridge.md`; S2 smoke = the measurement basis).
///
/// Shape: ONE synchronous `Store<Host>` hosts (a) the core-module guest with the frozen
/// `otter_jit` §5.1/§5.3 wiring and (b) a p3-world demo component whose WASI 0.3.0 imports
/// (`wasi:clocks/monotonic-clock@0.3.0 now`, `wasi:random/random@0.3.0 get-random-u64`) are
/// served by `wasmtime_wasi::p3::add_to_linker` — the SAME `WasiP1Ctx` backs both wirings
/// (`WasiP1Ctx` itself implements the p2/p3 `WasiView`, the p1-on-p2 precedent). The bridge is
/// one sync host function, `otter_p3.fetch_stamp`, that re-entrantly calls the component export
/// through `Caller` → `AsContextMut` while the guest frame is on the stack (path 12's proxy —
/// the MEASURED direction is core→component only, S2-E3; the reverse direction (a component
/// import routed back into the core instance) is the same mechanism by inference but is
/// unproductized and unmeasured — design note §2, a spike gates it if ever needed).
///
/// **Surface whitelist (host-side contract)**: only **sync-WIT** p3 functions may be reached
/// from the bridge. S2-E5 measured that async-WIT imports (`wait-for`, streams — and therefore
/// p3 stdin/stdout/filesystem streams and most of sockets) latch `async_required` on the Store
/// (even a FAILED sync instantiate latches: latch-then-validate order) and poison every sync
/// entrypoint including §5.3-3's `Instance::new` — i.e. they would silently degrade tier-up to
/// sticky-Refused T0. Crossing that line requires the full async rewire (S2-E4, the recorded
/// fallback path) plus an async variant of the §5.3 contract — a separate normative step, not a
/// config flip. [`assert_store_still_sync`] pins the no-latch fact after every gated run.
#[cfg(feature = "p3-bridge")]
mod p3_bridge {
    use wasmtime::component::{Component, Func as ComponentFunc, Linker as ComponentLinker, Val};
    use wasmtime::error::Context as _;
    use wasmtime::{Caller, Engine, Extern, Instance, Linker, Module, Result, Store};
    use wasmtime_wasi::{WasiCtxView, WasiView};

    use super::Host;

    /// One WASI ctx, two wirings (S2 type-level measurement): `WasiP1Ctx` implements the p3
    /// `WasiView` itself, so delegating is all the p3 component linker needs from `Host`.
    impl WasiView for Host {
        fn ctx(&mut self) -> WasiCtxView<'_> {
            self.wasi.ctx()
        }
    }

    /// Per-run bridge state, stored in the `Store` data next to the WASI ctx + jit state.
    #[derive(Default)]
    pub(super) struct BridgeState {
        /// The demo component's `stamp: func() -> u64` export handle (`component::Func` is
        /// `Copy`), installed by [`instantiate_demo_component`] before the guest runs.
        stamp: Option<ComponentFunc>,
        /// Total `otter_p3.fetch_stamp` invocations (successful or not) — printed as
        /// `HOST-BRIDGE-CALLS=` and cross-checked against the guest's own
        /// `E2E-EXPECT-BRIDGE-CALLS=` by `just wasm-e2e-p3`.
        pub(super) calls: u64,
    }

    /// The p3-world demo service, embedded as WAT (S2-E2/E3 fixture, verbatim): imports two
    /// plain **sync-WIT** functions of the WASI 0.3.0 packages shipped inside wasmtime-wasi's
    /// p3 module and exports `stamp: func() -> u64` = `(now << 1) ^ random` — so a successful
    /// bridge round trip proves the whole chain guest → host closure → component → p3 host
    /// wiring → component → guest memory, not just a host-side constant.
    const P3_STAMP_COMPONENT: &str = r#"
    (component
      (import "wasi:clocks/monotonic-clock@0.3.0" (instance $clk
        (export "now" (func (result u64)))))
      (import "wasi:random/random@0.3.0" (instance $rnd
        (export "get-random-u64" (func (result u64)))))
      (core func $now (canon lower (func $clk "now")))
      (core func $rand (canon lower (func $rnd "get-random-u64")))
      (core module $m
        (import "env" "now" (func $inow (result i64)))
        (import "env" "rand" (func $irand (result i64)))
        (func (export "stamp") (result i64)
          (i64.xor (i64.shl (call $inow) (i64.const 1)) (call $irand))))
      (core instance $ci (instantiate $m
        (with "env" (instance
          (export "now" (func $now))
          (export "rand" (func $rand))))))
      (func (export "stamp") (result u64)
        (canon lift (core func $ci "stamp"))))
    "#;

    /// Wire `otter_p3.fetch_stamp(dst: u32) -> i32` into the CORE linker. Status result: `0`
    /// success (u64 stamp written LE at `dst`), negative = error class (sign-normative like
    /// §5.1; specifics are diagnostics): `-1` no component installed, `-2` component call
    /// failed, `-3` unexpected result type, `-4` guest memory shape unsupported/missing,
    /// `-5` out-of-bounds/failed write.
    pub(super) fn wire_bridge_import(linker: &mut Linker<Host>) -> Result<()> {
        linker.func_wrap(
            "otter_p3",
            "fetch_stamp",
            |mut caller: Caller<'_, Host>, dst: u32| -> i32 { fetch_stamp(&mut caller, dst) },
        )?;
        eprintln!("host: p3-bridge: otter_p3.fetch_stamp wired (sync func_wrap -> component export)");
        Ok(())
    }

    /// Build the p3 component linker, SYNC-instantiate the demo component in the run Store, and
    /// install its `stamp` export handle. Plain sync path throughout — S2-E2a measured that the
    /// sync-WIT p3 surface needs neither an async store nor `wasm_component_model_async`.
    pub(super) fn instantiate_demo_component(engine: &Engine, store: &mut Store<Host>) -> Result<()> {
        let mut clinker: ComponentLinker<Host> = ComponentLinker::new(engine);
        wasmtime_wasi::p3::add_to_linker(&mut clinker)
            .context("p3-bridge: wasmtime_wasi::p3::add_to_linker")?;
        let component = Component::new(engine, P3_STAMP_COMPONENT)
            .context("p3-bridge: compiling the embedded p3-stamp demo component")?;
        let cinst = clinker.instantiate(&mut *store, &component).context(
            "p3-bridge: SYNC instantiate of the p3-stamp component failed — if the error names \
             async_required/*_async, a non-sync-WIT p3 import crept into the fixture (surface \
             whitelist violated, module doc)",
        )?;
        let stamp = cinst
            .get_func(&mut *store, "stamp")
            .context("p3-bridge: component lacks export `stamp`")?;
        store.data_mut().bridge.stamp = Some(stamp);
        eprintln!(
            "host: p3-bridge: demo component instantiated in the run Store \
             (wasi:clocks + wasi:random @0.3.0 via p3 host wiring, sync)"
        );
        Ok(())
    }

    /// The bridge body: re-entrant component call on the same Store (guest frame live), result
    /// written into guest linear memory at `dst` (LE u64). NOTE: `Func::post_return` is a
    /// deprecated no-op on 48 (upstream func.rs: "no longer needs to be called") — deliberately
    /// NOT called (a call would trip -D warnings via the deprecation lint).
    fn fetch_stamp(caller: &mut Caller<'_, Host>, dst: u32) -> i32 {
        caller.data_mut().bridge.calls += 1;
        let Some(f) = caller.data().bridge.stamp else {
            eprintln!("host: p3-bridge: fetch_stamp called but no component installed (-1)");
            return -1;
        };
        let mut results = [Val::Bool(false)];
        if let Err(e) = f.call(&mut *caller, &[], &mut results) {
            eprintln!("host: p3-bridge: component stamp() call failed: {e:?} (-2)");
            return -2;
        }
        let Val::U64(v) = results[0] else {
            eprintln!("host: p3-bridge: stamp() returned unexpected {results:?} (-3)");
            return -3;
        };
        // Demo-surface scope (recorded limitation, design note §4): plain linear memory only.
        // A shared (O-17 atomics) guest memory would need the UnsafeCell write path — the
        // bridge × atomics combination is not part of this increment's gate and fails loudly.
        let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
            eprintln!(
                "host: p3-bridge: guest \"memory\" export missing or not a plain memory \
                 (shared/atomics guests are outside the O-16f demo surface) (-4)"
            );
            return -4;
        };
        let in_bounds = (dst as u64)
            .checked_add(8)
            .is_some_and(|end| end <= mem.data_size(&*caller) as u64);
        if !in_bounds {
            eprintln!("host: p3-bridge: [dst={dst}, +8) exceeds guest memory (-5)");
            return -5;
        }
        if let Err(e) = mem.write(&mut *caller, dst as usize, &v.to_le_bytes()) {
            eprintln!("host: p3-bridge: writing stamp at dst={dst} failed: {e:?} (-5)");
            return -5;
        }
        0
    }

    /// §5.5 sync-norm probe (48 reality: the per-Store one-way `async_required` latch — see the
    /// design note §5): after the whole mixed run, the Store must still accept synchronous
    /// entrypoints. A trivial sync `Instance::new` is exactly the §5.3-3 shape that a tripped
    /// latch rejects. Prints the `HOST-P3-STORE-SYNC-OK` marker consumed by `just wasm-e2e-p3`.
    pub(super) fn assert_store_still_sync(engine: &Engine, store: &mut Store<Host>) -> Result<()> {
        let empty = Module::new(engine, &b"\0asm\x01\x00\x00\x00"[..])
            .context("p3-bridge: building the empty probe module")?;
        Instance::new(&mut *store, &empty, &[]).context(
            "p3-bridge: post-run sync latch probe FAILED — the Store tripped async_required \
             (§5.5 sync norm violated; an async-flavored p3 surface must have been linked)",
        )?;
        println!(
            "HOST-P3-STORE-SYNC-OK post-run Store still accepts sync entrypoints \
             (async_required latch untouched)"
        );
        Ok(())
    }

    /// Unit gates (run by `just wasm-e2e-p3` via `cargo test --features p3-bridge`): the S2-E2a
    /// staged probe as a regression test — p3 wiring + component compile + SYNC instantiate +
    /// two sync calls, under the PRODUCT Config (wasm_exceptions + shared_memory both on, the
    /// run() shape — this is also the components × Config::shared_memory(true) combination's
    /// standing witness), then the latch probe.
    #[cfg(test)]
    mod tests {
        use super::*;
        use wasmtime::Config;
        use wasmtime_wasi::WasiCtxBuilder;

        fn product_engine() -> Engine {
            let mut config = Config::new();
            config.wasm_exceptions(true);
            config.shared_memory(true);
            Engine::new(&config).expect("engine")
        }

        fn host() -> Host {
            Host {
                wasi: WasiCtxBuilder::new().build_p1(),
                jit: super::super::JitState::default(),
                bridge: BridgeState::default(),
            }
        }

        #[test]
        fn p3_stamp_component_sync_round_trip_and_no_latch() {
            let engine = product_engine();
            let mut store: Store<Host> = Store::new(&engine, host());
            instantiate_demo_component(&engine, &mut store).expect("sync p3 instantiate");
            let f = store.data().bridge.stamp.expect("stamp installed");
            let mut r = [Val::Bool(false)];
            f.call(&mut store, &[], &mut r).expect("sync stamp call #1");
            let Val::U64(v1) = r[0] else {
                panic!("stamp #1 type: {r:?}")
            };
            f.call(&mut store, &[], &mut r).expect("sync stamp call #2");
            let Val::U64(v2) = r[0] else {
                panic!("stamp #2 type: {r:?}")
            };
            assert!(
                v1 != 0 && v2 != 0 && v1 != v2,
                "suspicious stamps {v1:#x} {v2:#x}"
            );
            assert_store_still_sync(&engine, &mut store).expect("latch probe");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::emitted_sections_whitelisted;

    /// The §5.2 emitted-module shape the backend actually produces (same shape as the E2E demo's
    /// emitted modules and the adversarial guests' "good" module): imports otter.memory +
    /// otter.table, exports one function "f" of the CompiledFn type, sections
    /// type/import/function/export/code only.
    const GOOD: &str = r#"(module
        (import "otter" "memory" (memory 0))
        (import "otter" "table" (table 0 funcref))
        (func (export "f") (param i32 i32 i64) (result i64) i64.const 42))"#;

    fn assert_sections(bytes: &[u8], expect: &[u8]) {
        // Sanity for the fixtures themselves: confirm which section ids are actually present.
        let mut ids = Vec::new();
        let mut p = 8usize;
        while p < bytes.len() {
            ids.push(bytes[p]);
            p += 1;
            let mut size = 0u64;
            let mut shift = 0u32;
            loop {
                let b = bytes[p];
                p += 1;
                size |= u64::from(b & 0x7f) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            p += size as usize;
        }
        assert_eq!(ids, expect, "fixture does not have the intended sections");
    }

    #[test]
    fn good_emitted_module_shape_passes() {
        let bytes = wat::parse_str(GOOD).unwrap();
        assert_sections(&bytes, &[1, 2, 3, 7, 10]);
        assert!(emitted_sections_whitelisted(&bytes));
    }

    #[test]
    fn custom_section_is_allowed() {
        // Tools (and the backend's debug config) may append custom sections such as `name`;
        // they are semantically inert and stay allowed. Appended raw: id 0, size 2, name "x".
        let mut bytes = wat::parse_str(GOOD).unwrap();
        bytes.extend_from_slice(&[0x00, 0x02, 0x01, b'x']);
        assert!(emitted_sections_whitelisted(&bytes));
    }

    #[test]
    fn start_section_rejected() {
        // W-0 A#2 original ban: instantiation would run guest code on the frozen guest stack.
        let bytes = wat::parse_str(
            r#"(module
                (import "otter" "memory" (memory 0))
                (import "otter" "table" (table 0 funcref))
                (func $s)
                (func (export "f") (param i32 i32 i64) (result i64) i64.const 43)
                (start $s))"#,
        )
        .unwrap();
        assert!(!emitted_sections_whitelisted(&bytes));
    }

    #[test]
    fn element_section_rejected() {
        // Panel #3 lens B#1 exploit shape (adversarial guest exit 101): an ACTIVE element
        // segment into the IMPORTED shared table rebinds pre-existing slot 1 at Instance::new.
        let bytes = wat::parse_str(
            r#"(module
                (import "otter" "memory" (memory 0))
                (import "otter" "table" (table 0 funcref))
                (func $f (param i32 i32 i64) (result i64) i64.const 222)
                (elem (i32.const 1) func $f)
                (export "f" (func $f)))"#,
        )
        .unwrap();
        assert!(!emitted_sections_whitelisted(&bytes));
    }

    #[test]
    fn data_section_rejected() {
        // Panel #3 lens B#2 exploit shape (adversarial guest exit 100): an ACTIVE data segment
        // into the IMPORTED guest memory writes "PWNED" at address 0 at Instance::new.
        let bytes = wat::parse_str(
            r#"(module
                (import "otter" "memory" (memory 0))
                (import "otter" "table" (table 0 funcref))
                (func (export "f") (param i32 i32 i64) (result i64) i64.const 44)
                (data (i32.const 0) "PWNED"))"#,
        )
        .unwrap();
        assert!(!emitted_sections_whitelisted(&bytes));
    }

    #[test]
    fn datacount_section_rejected() {
        // Raw bytes: magic + version + datacount(12) section (size 1, count 0). `wat` won't
        // emit a lone datacount, so this fixture is hand-framed.
        let bytes: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
            0x0c, 0x01, 0x00, // id 12, size 1, count 0
        ];
        assert!(!emitted_sections_whitelisted(bytes));
    }

    #[test]
    fn malformed_leb_fails_closed() {
        // Over-long size LEB (5 continuation bytes → shift 35): reject, never loop/panic.
        let overlong: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
            0x01, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00, // type section, over-long size
        ];
        assert!(!emitted_sections_whitelisted(overlong));
        // Truncated size field (id then EOF): reject.
        let truncated: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01];
        assert!(!emitted_sections_whitelisted(truncated));
        // Declared size overruns the buffer: reject.
        let overrun: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
            0x01, 0x7f, 0x00, // type section claims 127 bytes, 1 present
        ];
        assert!(!emitted_sections_whitelisted(overrun));
        // Shorter than magic + version: reject.
        assert!(!emitted_sections_whitelisted(&[0x00, 0x61, 0x73, 0x6d]));
    }

    /// O-19 design §6-4 (vi) — pin the J-mode host's refusal of a memory64-SHAPED emitted
    /// module (a shape the J-mode backend can never emit — EmitOpts has no memory64; reaching
    /// this seam requires a forged/foreign guest). The pinned facts, measured on 48:
    /// (1) `Module::new` PARSES it (memory64 is default-on — parse is NOT the defense),
    /// (2) the §5.2 section whitelist PASSES it (it keys on section ids only — an i64 memory
    ///     import rides the whitelisted import section; also not the defense),
    /// (3) the single load-bearing layer is `Instance::new`'s import type match: the host's
    ///     32-bit memory extern does not satisfy a `(memory i64 0)` import, so the compile
    ///     path returns the -2 class (NOT the design's originally assumed -1 — recorded in
    ///     design §6-4 (vi) with this measurement).
    /// If a wasmtime bump ever loosens that import match, this test goes loud instead of the
    /// defense silently vanishing.
    #[test]
    fn memory64_shaped_module_is_refused_at_instantiation_not_parse() {
        use wasmtime::{Config, Engine, Memory, MemoryType, Module, Ref, RefType, Store, Table, TableType};
        let m64 = wat::parse_str(
            r#"(module
                (import "otter" "memory" (memory i64 0))
                (import "otter" "table" (table 0 funcref))
                (func (export "f") (param i32 i32 i64) (result i64) i64.const 42))"#,
        )
        .unwrap();
        // (2) whitelist passes — sections are exactly the §5.2 set.
        assert_sections(&m64, &[1, 2, 3, 7, 10]);
        assert!(
            emitted_sections_whitelisted(&m64),
            "the whitelist is sections-only by design; it must NOT be what rejects memory64"
        );
        // Product Config shape (wasm_exceptions + shared_memory on, as run() sets them).
        let mut config = Config::new();
        config.wasm_exceptions(true);
        config.shared_memory(true);
        let engine = Engine::new(&config).unwrap();
        // (1) parse succeeds — memory64 is default-on on 48.
        let module = Module::new(&engine, &m64).expect("parse alone must not be the defense");
        // (3) instantiation against the host's 32-bit externs is the load-bearing refusal.
        let mut store = Store::new(&engine, ());
        let mem = Memory::new(&mut store, MemoryType::new(1, None)).unwrap();
        let tbl = Table::new(
            &mut store,
            TableType::new(RefType::FUNCREF, 4, None),
            Ref::Func(None),
        )
        .unwrap();
        let refused = wasmtime::Instance::new(&mut store, &module, &[mem.into(), tbl.into()]);
        assert!(
            refused.is_err(),
            "a memory64-shaped module must fail instantiation against 32-bit host externs \
             (the -2 class) — if this ever succeeds, the single-layer defense is gone"
        );
    }
}
