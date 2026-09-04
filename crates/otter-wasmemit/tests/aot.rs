//! O-14 A-3 gate suite (design `otter-o14-aot-core-design.md` §9 A-3) for [`compile_module`]:
//! call-lowering inversion — `Call`/`TailCall` now guard (`i64.ge_u` against `nfuncs`) then
//! `call_indirect`/`return_call_indirect` the callee word DIRECTLY (§3.1/§3.2), retiring A-2's
//! two provisional `otter_helpers.{dispatch,tail}` trampoline imports/slots WHOLE.
//!
//! * **N-in-1 differential, now trampoline-FREE** — every function of ONE artifact is reachable
//!   through the artifact's OWN declared `otter_table` export at its §2.3 identity slot; since
//!   A-3 the harness needs NO host-side dispatch/tail stub and NO two-stage wiring at all (A-2's
//!   circularity — the stubs needed the module's OWN `otter_table` export to dispatch through —
//!   is gone because there is no stub anymore: `TailCall` chains resolve ENTIRELY inside wasm via
//!   `return_call_indirect`, so one host-level `TypedFunc::call` is the whole story, tail-deep
//!   recursion included). Call-free corpus (the fuzz streams shared with `differential.rs`) AND
//!   the call-carrying corpus (`fact`/`countdown`/`even-odd`/marshal/zero-argc) both execute
//!   end-to-end, same as A-2, now through the FINAL call-site shape.
//! * **record==live, T-normalized, R1 (call-free, full text) + R2 (call-carrying, window-local)**
//!   (§5.2) — R1 is UNCHANGED (A-3 never touches Bin/Un/Host* lowering); R2 is NEW this
//!   increment: Table-mode and Dispatch-mode bodies now diverge STRUCTURALLY at every `Call`/
//!   `TailCall` site (guard+direct-call_indirect vs helper-call_indirect), not merely in a
//!   type-index immediate, so the gate compares OPERATOR sequences, matching everywhere outside
//!   each call site's window and independently verifying both sides' window shape against its own
//!   mode's §3.1/§3.2 literal (gate ⑥).
//! * **2M-deep bounded tail recursion + negative control (gate ①/②)** — mutual `TailCall`
//!   recursion through the declared table, in a 64 KiB wasm stack, completes at depth
//!   2,000,000/2,000,001 ([스파이크] M2's exact figures); the SAME shape lowered with an ordinary
//!   (non-tail) `Call` instead overflows that same 64 KiB stack — isolating the boundedness claim
//!   to `return_call_indirect` specifically (not "there is a guard", which the fail-closed gate
//!   covers separately).
//! * **Fail-closed 3 shapes, Call AND TailCall (gate ③)** — OOB (`>= nfuncs`), plain negative
//!   (`-1`), and the two FULL-WIDTH shapes A-C1 exists for (`2^32 + k`, `-2^32`, neither
//!   representable by a single sign-extended `MOV_IMM` — built from `Bin`/`Neg` at I64 instead) —
//!   every shape yields `fill`, never a trap, on both call-site kinds.
//! * **`if (result i64)` block-type shape (gate ④)** — validated AND executed, structurally
//!   pinned via the R2 window-shape assertion.
//! * **Recursive/nested marshaling differential (gate ⑤)** — self-recursive `Call` (`fact`),
//!   order-sensitive wide-argc `Call` (`marshal_pair`), mutual `TailCall` (`even`/`odd`), and a
//!   mixed Call-then-TailCall-tree (`mixed`, ported from `calls.rs`'s engine-shape corpus) all
//!   match the multi-function T0 oracle.
//! * **`validate_aot_artifact` (§2.1)** — the AOT pre-link whitelist + the §2.3 defensive
//!   element-target check, exercised on a legitimate artifact (must pass) and on hand-built
//!   adversarial fixtures (mutation ⑥: element targets an IMPORTED table; a disallowed section).
//! * **Mutation ③** — element offset 0→1 injected into a real artifact's bytes: the identity
//!   mapping breaks (either instantiation itself rejects the shifted-out-of-bounds segment, or it
//!   succeeds and every slot answers with the WRONG function — either outcome is "red" against the
//!   un-mutated differential, and both are asserted for).
//! * **AotError regression** (§8.4) — unchanged from A-1/A-2 (the per-function acceptance
//!   predicate and its ordering are untouched by A-3's call-lowering change).
//!
//! Structure + manifest invariants now pin the FINAL-CONTRACT A-3 shape: import list = memory + 3
//! helpers + N hosts (no provisional trampolines), declared table `min=max=table_size` (no more
//! `+2`), `host_slots` formula `nfuncs+3+k` (the §2.3 canonical form, restored).

mod common;

use std::cell::{Cell, RefCell};

use common::{
    CALL, HALT, HOST1, HOST2, HOSTCTX2, JUMP_IF, MOV_IMM, RET, TAILCALL, bin_opcode, body_ops, func, gen_cfg,
    gen_straightline, lcg, neg_opcode, oracle,
};
use otter::interp::run_module;
use otter::{FuncDef, Inst, Isa, Module as IrModule, Value};
use otter_wasmemit::{
    AotArtifact, AotError, AotImportKind, AotOpts, AotValidationError, EmitOpts, HelperIdx, HelperKind,
    HostImportName, MAX_WASM_FUNCTION_LOCALS, Refusal, compile_dispatch_fn_with, compile_module,
    scratch_words, validate_aot_artifact,
};
use wasm_encoder::{
    ConstExpr, ElementSection, Elements, EntityType, ExportKind, ExportSection, FunctionSection,
    ImportSection, Module as WasmModule, TableSection, TableType as WasmTableType, TypeSection,
    ValType as WasmValType,
};
use wasmparser::{Operator, Parser, Payload, TypeRef, Validator};
use wasmtime::{
    AsContextMut, Caller, Config, Engine, Extern, Func, Instance, Memory, MemoryType, Module, Ref, Store,
    StoreContextMut, Table, Trap, TypedFunc,
};

// --- fixture constants ---------------------------------------------------------------------------

/// [`AotOpts::scratch_base`] for the whole suite — inside the 16-page mock memory, clear of the
/// staging area.
const SCRATCH_BASE: u64 = 0x1_0000;
/// Where the harness's LIFO argument-staging bump pointer starts (top-level entry AND every
/// nested `Call`/`TailCall` marshal here — calls.rs's `STAGING_BASE` pattern, disjoint from every
/// function's own §2.5 scratch).
const STAGING_BASE: u32 = 0x8_0000;
const MEMORY_PAGES: u32 = 16;
/// A junk ctx word — §2.4: the `ctx` param is dead in the A-mode artifact (forwarded at most).
const CTX_JUNK: i32 = 0x7EAD;
/// Per-call fuel budget (same rationale as every other harness in this crate: runaway loops must
/// trap loudly, not hang CI). Since A-3 a `TailCall` chain resolves ENTIRELY inside ONE wasm-level
/// call (`return_call_indirect`, no host-mediated trampoline loop to re-arm fuel between steps —
/// contrast `calls.rs`'s per-activation re-arming, which existed FOR that trampoline), so this
/// single budget must cover an entire call's worth of work — generous for the normal corpus.
const FUEL: u64 = 100_000_000;
/// Fuel for the gate ①/② 2M-deep tests specifically (§9 A-3 gate row): 2,000,001 tail steps, each
/// a handful of fuel-consuming instructions (guard, marshal, `return_call_indirect`) — comfortably
/// under this budget with orders of magnitude of headroom, while still tripping on a genuinely
/// runaway loop instead of hanging CI.
const DEEP_FUEL: u64 = 50_000_000_000;
/// [스파이크] M2's exact wasm stack size ("64 KiB 스택") — deliberately tiny so gate ①'s
/// boundedness claim and gate ②'s negative control (native, non-tail-call stack growth) are both
/// decisive: wasmtime's default is 512 KiB (`Config::max_wasm_stack`'s doc), which would let the
/// negative control get needlessly (and slowly) deep before overflowing.
const TINY_WASM_STACK: usize = 65_536;

const FILL: Value = 0x5EED_F00D_5EED_F00D_u64 as Value;
const HOST_CTX: Value = 0x7E57_C0DE_7E57_C0DE_u64 as Value;

/// The host-import NAME map — what a real embedder's `resolve_host` knows (§2.2). The harness's
/// ptr→slot map is derived from the resolver CALL ORDER (see [`compile_with_log`]), exercising the
/// §2.3 norm "first-resolution order = slot order" instead of duplicating it.
fn host_name(p: usize) -> Option<&'static str> {
    let map: [(usize, &'static str); 4] = [
        (common::h_add7 as otter::HostFn1 as usize, "h_add7"),
        (common::h_mix as otter::HostFn2 as usize, "h_mix"),
        (common::hc_ret_ctx as otter::HostFnCtx1 as usize, "hc_ret_ctx"),
        (common::hc_mix as otter::HostFnCtx2 as usize, "hc_mix"),
    ];
    map.iter().find(|(a, _)| *a == p).map(|(_, n)| *n)
}

// --- compile helpers -----------------------------------------------------------------------------

/// `compile_module` with a resolution-order-LOGGING resolver. Returns the artifact plus the
/// distinct host-fn pointers in resolver-call order — which the §2.3/§4.1 norms make the
/// `manifest.host_slots` order, so `zip(order, host_slots)` IS the harness's ptr→slot map.
/// Asserts the §4.1 exactly-once rule on the way (one call per distinct pointer, ever).
fn compile_with_log(isa: &Isa, m: &IrModule, opts: &AotOpts) -> (AotArtifact, Vec<usize>) {
    let order: RefCell<Vec<usize>> = RefCell::new(Vec::new());
    let resolve = |p: usize| -> Option<HostImportName> {
        let name = host_name(p)?;
        order.borrow_mut().push(p);
        Some(HostImportName(name.to_string()))
    };
    let artifact = compile_module(isa, m, &resolve, opts).expect("corpus module must compile");
    let order = order.into_inner();
    let mut dedup = order.clone();
    dedup.sort_unstable();
    dedup.dedup();
    assert_eq!(
        dedup.len(),
        order.len(),
        "§4.1 resolver-determinism: exactly ONE resolve_host call per distinct pointer"
    );
    assert_eq!(
        order.len(),
        artifact.manifest.host_slots.len(),
        "one slot per resolved host fn"
    );
    (artifact, order)
}

/// Compile one artifact per entry choice — an ENTRY-INVARIANCE regression only (design §2.4: the
/// `otter_entry` export names a DIFFERENT function index per variant, but nothing else about the
/// artifact may depend on `m.entry`). Since A-2 this is no longer the execution access path (every
/// function of ONE artifact is already reachable through its own `otter_table` — see [`Nin1`]),
/// but the invariance itself remains a real, useful regression to pin.
fn compile_entry_variants(isa: &Isa, m: &IrModule) -> (Vec<AotArtifact>, Vec<usize>) {
    let mut artifacts = Vec::with_capacity(m.funcs.len());
    let mut order0: Vec<usize> = Vec::new();
    for j in 0..m.funcs.len() {
        let mut mv = m.clone();
        mv.entry = j;
        let (a, order) = compile_with_log(isa, &mv, &AotOpts::new(SCRATCH_BASE));
        if j == 0 {
            order0 = order;
        } else {
            assert_eq!(order, order0, "resolution order must be entry-invariant");
            let a0 = &artifacts[0] as &AotArtifact;
            assert_eq!(
                func_types(&a.bytes),
                func_types(&a0.bytes),
                "type section is entry-invariant"
            );
            assert_eq!(
                bodies_with_ty_imm_offsets(&a.bytes),
                bodies_with_ty_imm_offsets(&a0.bytes),
                "code section is entry-invariant"
            );
            assert_eq!(
                table_and_element_bytes(&a.bytes),
                table_and_element_bytes(&a0.bytes),
                "table + element sections are entry-invariant"
            );
            let mut man = a.manifest.clone();
            man.entry_slot = a0.manifest.entry_slot;
            assert_eq!(
                man, a0.manifest,
                "manifest differs only in entry_slot across entry variants"
            );
        }
        artifacts.push(a);
    }
    (artifacts, order0)
}

// --- artifact parsing (wasmparser — the same dev-dep the structure tests use) --------------------

/// The module's function types, structurally, in type-section order.
fn func_types(bytes: &[u8]) -> Vec<(Vec<wasmparser::ValType>, Vec<wasmparser::ValType>)> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::TypeSection(r) = payload.expect("artifact must parse") {
            for group in r {
                for ty in group.expect("type").into_types() {
                    let f = ty.composite_type.unwrap_func();
                    out.push((f.params().to_vec(), f.results().to_vec()));
                }
            }
        }
    }
    out
}

/// Every function body's bytes (locals declarations + expression, i.e. the code entry minus its
/// size prefix) and, within each, the byte offsets of every `call_indirect` TYPE-INDEX immediate.
/// All involved type indices are 1-byte LEBs (< 8 types exist — asserted), which is what makes
/// the §5.2 T-normalized comparison length-preserving.
fn bodies_with_ty_imm_offsets(bytes: &[u8]) -> Vec<(Vec<u8>, Vec<usize>)> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("artifact must parse") {
            let range = body.range();
            let (start, end) = (range.start as usize, range.end as usize);
            let slice = bytes[start..end].to_vec();
            let mut offs = Vec::new();
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                let (op, pos) = reader.read_with_offset().expect("operator");
                if let Operator::CallIndirect { type_index, .. } = op {
                    // 0x11 opcode byte, then the type-index LEB.
                    let imm = pos as usize + 1 - start;
                    assert!(type_index < 0x80, "§5.2 precondition: 1-byte LEB type indices");
                    assert_eq!(
                        u32::from(slice[imm]),
                        type_index,
                        "type-index immediate must be the single byte after the opcode"
                    );
                    offs.push(imm);
                }
            }
            out.push((slice, offs));
        }
    }
    out
}

// (H4) `body_ops` — the decoded operator list of function body `j` — moved to `tests/common`
// (`common::body_ops`), so the opcode-inventory gate reuses this same parser instead of a copy.

/// Raw content bytes of the table section and the element section (for the entry-invariance
/// comparison — neither depends on `m.entry`).
fn table_and_element_bytes(bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut table = Vec::new();
    let mut elem = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("parse") {
            Payload::TableSection(r) => {
                let range = r.range();
                table = bytes[range.start as usize..range.end as usize].to_vec();
            }
            Payload::ElementSection(r) => {
                let range = r.range();
                elem = bytes[range.start as usize..range.end as usize].to_vec();
            }
            _ => {}
        }
    }
    (table, elem)
}

// --- call-site extraction (shared by R1's call-free guard and R2's window walk) -------------------

/// One `Call`/`TailCall` site's `(base_slot, n, is_tail)`, in PROGRAM (pc-ascending) order — the
/// SAME order every lowering path visits `f.code` in, straight-line or dispatcher alike (both
/// `emit_straightline_body`/`emit_dispatcher_body` walk `for (pc, inst) in f.code.iter().enumerate()`
/// — lib.rs, structural anchor), so this independent re-derivation lines up with emission order by
/// construction. `n` mirrors lib.rs's private `site_argc` clamp (`argc > 0 ⇒ argc as usize, else 0`)
/// — the same single-source discipline `otter-wasmemit` itself follows, re-derived here rather than
/// imported (this is an integration test: it must not read lib.rs internals).
#[derive(Clone, Copy, Debug)]
struct SiteInfo {
    base_slot: i32,
    n: usize,
    is_tail: bool,
}

fn call_sites(isa: &Isa, f: &FuncDef) -> Vec<SiteInfo> {
    let mut out = Vec::new();
    for inst in &f.code {
        for eff in &isa.op(inst.op).effects {
            match eff {
                otter::Effect::Call { base, argc, .. } => {
                    let raw = inst.operand(*argc);
                    out.push(SiteInfo {
                        base_slot: inst.operand(*base),
                        n: if raw > 0 { raw as usize } else { 0 },
                        is_tail: false,
                    });
                }
                otter::Effect::TailCall { base, argc } => {
                    let raw = inst.operand(*argc);
                    out.push(SiteInfo {
                        base_slot: inst.operand(*base),
                        n: if raw > 0 { raw as usize } else { 0 },
                        is_tail: true,
                    });
                }
                _ => {}
            }
        }
    }
    out
}

// --- the §5.2 record==live gate — R1 (call-free, full-text T-normalized) --------------------------

/// **R1** (§5.2): assert every function body of `artifact` is **T-normalized byte-identical** to
/// its live twin (`compile_dispatch_fn_with` under the §5.2 canonical injection). Returns how many
/// type-index immediates had DIFFERENT raw values across the pair — the defined divergence the
/// normalization absorbed (callers use it for the non-vacuity assertion).
///
/// **Call-free functions ONLY since O-14 A-3** (§5.2 R1/R2 split): a function containing
/// `Call`/`TailCall` diverges STRUCTURALLY between `CallMode::Table` (record) and
/// `CallMode::Dispatch` (live) at every call site — not merely in a type-index immediate — so
/// full-text comparison is the wrong tool for it; use [`assert_record_equals_live_windowed`]
/// (R2) instead. Enforced defensively (not merely by corpus discipline): a call-carrying `f`
/// panics here with a pointer to R2, rather than silently mis-asserting.
fn assert_record_equals_live(isa: &Isa, m: &IrModule, artifact: &AotArtifact, order: &[usize]) -> usize {
    let man = &artifact.manifest;
    // O-19 §2.6 [결정 v2 — 옵션 (b)]: A64 is EXPLICITLY EXEMPT from record==live — `EmitOpts`
    // is deliberately not memory64-extended (§2.1), so no public live-twin source for an A64
    // body exists, and including A64 corpus here would go red on `compiled_ty_norm`'s A32
    // hardcode below anyway. This guard makes the exemption a MACHINE assertion (inclusion is
    // an immediate, attributed red, not an accidental shape mismatch); A64's structural gate is
    // the modulo-addrtype differential (tests/addrtype.rs). Re-open: J-mode A64 (§1.3 ①).
    assert!(
        !man.memory64,
        "record==live R1 is A32-only — A64 corpus is exempt (O-19 §2.6); use the \
         modulo-addrtype differential gate in tests/addrtype.rs instead"
    );
    // R*: the manifest host_slots, keyed by the logged resolution order (§5.2).
    let live_resolve =
        |p: usize| -> Option<u32> { order.iter().position(|&q| q == p).map(|k| man.host_slots[k]) };
    // H*: manifest helper slots for binop/convert/unop (§5.2 canonical injection). `dispatch`/
    // `tail` are the historical A-1/A-2 OOB-sentinel values, kept for continuity/documentation
    // ONLY — since A-3 `CallMode::Table` never bakes either field (`HelperIdx`'s own doc), and R1
    // is call-free-only (this fn's own guard below), so neither is ever emitted into any byte this
    // function compares.
    let h_star = HelperIdx {
        dispatch: 0xFFFF_FFFE,
        tail: 0xFFFF_FFFF,
        binop: man.helper_slots[0],
        convert: man.helper_slots[1],
        unop: man.helper_slots[2],
    };
    let rec_types = func_types(&artifact.bytes);
    let rec_bodies = bodies_with_ty_imm_offsets(&artifact.bytes);
    assert_eq!(rec_bodies.len(), m.funcs.len(), "one code entry per function");
    let mut raw_imm_divergences = 0usize;
    for (j, f) in m.funcs.iter().enumerate() {
        assert!(
            call_sites(isa, f).is_empty(),
            "func {j}: R1 (assert_record_equals_live) is call-free-functions-only since A-3 — \
             use assert_record_equals_live_windowed (R2) for a Call/TailCall-containing function"
        );
        // S*: the manifest scratch map. Sanity: it must agree with the public sizing helper.
        assert_eq!(
            man.scratch[j].words,
            scratch_words(isa, f).expect("corpus function"),
            "manifest words == scratch_words (single-source rule)"
        );
        let live_bytes = compile_dispatch_fn_with(
            isa,
            j,
            f,
            m.fill,
            m.host_ctx,
            h_star,
            &live_resolve,
            man.scratch[j].addr,
            EmitOpts::new().with_shared_memory(man.shared_memory),
        )
        .expect("live twin must compile (record accepted it — §6 ⊂-consistency)");
        let live_types = func_types(&live_bytes);
        let mut live_bodies = bodies_with_ty_imm_offsets(&live_bytes);
        assert_eq!(live_bodies.len(), 1);
        let (lb, lo) = live_bodies.swap_remove(0);
        let (rb, ro) = &rec_bodies[j];
        assert_eq!(
            rb.len(),
            lb.len(),
            "func {j}: T-normalization is length-preserving (all type imms 1-byte)"
        );
        assert_eq!(
            ro, &lo,
            "func {j}: call_indirect sites must line up byte-for-byte"
        );
        let mut is_imm = vec![false; rb.len()];
        for &o in ro {
            is_imm[o] = true;
        }
        for (i, (&r, &l)) in rb.iter().zip(&lb).enumerate() {
            if is_imm[i] {
                // The T-normalized comparison: re-map both immediates through their own type
                // sections and require STRUCTURAL equality of the site's type.
                let rt = &rec_types[r as usize];
                let lt = &live_types[l as usize];
                assert_eq!(rt, lt, "func {j} byte {i}: site's structural type diverges");
                if r != l {
                    raw_imm_divergences += 1;
                }
            } else {
                assert_eq!(r, l, "func {j}: byte {i} differs OUTSIDE a type-index site");
            }
        }
    }
    raw_imm_divergences
}

// --- the §5.2 record==live gate — R2 (call-carrying, window-localized) — O-14 A-3 -------------------

/// One decoded operator, T-NORMALIZED (§5.2) at the OPERATOR level — the same normalization
/// principle `bodies_with_ty_imm_offsets`' raw-byte remap uses for R1, needed here because R2
/// compares ACROSS the two call-lowering modes, whose `Call`/`TailCall` windows bake DIFFERENT
/// wasm types (`COMPILED_TY` in Table mode, the dispatch-helper type in Dispatch mode) — every
/// `CallIndirect`/`ReturnCallIndirect`'s raw `type_index` is replaced by a STRUCTURAL rendering
/// (its own crafted string, chosen once and used identically on both sides — never wasmparser's
/// own Debug format for these two variants, sidestepping any format-stability question for them);
/// every other operator keeps wasmparser's own `{:?}` (the same convention `body_ops`/
/// `has_slot_call` already rely on elsewhere in this file).
fn op_call_indirect_norm(ty: &str, table_index: u32) -> String {
    format!("CallIndirectNorm{{ty={ty}, table={table_index}}}")
}
fn op_return_call_indirect_norm(ty: &str, table_index: u32) -> String {
    format!("ReturnCallIndirectNorm{{ty={ty}, table={table_index}}}")
}
fn ty_norm_str(params: &[wasmparser::ValType], results: &[wasmparser::ValType]) -> String {
    format!("{:?}", (params.to_vec(), results.to_vec()))
}

/// Function body `j`'s operator sequence, T-normalized (see [`op_call_indirect_norm`]'s doc).
fn decode_norm_ops(
    bytes: &[u8],
    j: usize,
    types: &[(Vec<wasmparser::ValType>, Vec<wasmparser::ValType>)],
) -> Vec<String> {
    let mut bodies = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("parse") {
            let mut ops = Vec::new();
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                let op = reader.read().expect("operator");
                let s = match op {
                    Operator::CallIndirect {
                        type_index,
                        table_index,
                    } => {
                        let (p, r) = &types[type_index as usize];
                        op_call_indirect_norm(&ty_norm_str(p, r), table_index)
                    }
                    Operator::ReturnCallIndirect {
                        type_index,
                        table_index,
                    } => {
                        let (p, r) = &types[type_index as usize];
                        op_return_call_indirect_norm(&ty_norm_str(p, r), table_index)
                    }
                    other => format!("{other:?}"),
                };
                ops.push(s);
            }
            bodies.push(ops);
        }
    }
    bodies.swap_remove(j)
}

/// wasm local index of otter local `slot` — mirrors lib.rs's private `FIRST_LOCAL = 3` (params
/// `ctx=0, args=1, argc=2` — design §2.2's ABI, independently re-derived here per the same
/// "integration test must not read internals" discipline as [`call_sites`]).
const TEST_FIRST_LOCAL: u32 = 3;
const TEST_CTX_LOCAL: u32 = 0;
fn test_local(slot: i32) -> u32 {
    TEST_FIRST_LOCAL + slot as u32
}

fn op_local_get(i: u32) -> String {
    format!("LocalGet {{ local_index: {i} }}")
}
fn op_i32_const(v: i32) -> String {
    format!("I32Const {{ value: {v} }}")
}
fn op_i64_const(v: i64) -> String {
    format!("I64Const {{ value: {v} }}")
}

/// [`CallMode::Dispatch`]'s `Call` window, literal (lib.rs `emit_dispatch_call`, design §4.3) —
/// independently re-derived (not borrowed from the emitter under test).
fn dispatch_call_window(
    base_slot: i32,
    scratch_addr: u64,
    n: usize,
    helper_slot: u32,
    dispatch_ty: &str,
) -> Vec<String> {
    vec![
        op_local_get(TEST_CTX_LOCAL),
        op_local_get(test_local(base_slot)),
        op_i32_const(scratch_addr as i32),
        op_i64_const(n as i64),
        op_i32_const(helper_slot as i32),
        op_call_indirect_norm(dispatch_ty, 0),
    ]
}

/// [`CallMode::Table`]'s `Call` window, literal (design §3.1 — lib.rs `emit_table_call`).
fn table_call_window(
    base_slot: i32,
    scratch_addr: u64,
    n: usize,
    nfuncs: u32,
    fill: Value,
    compiled_ty: &str,
) -> Vec<String> {
    let b = test_local(base_slot);
    vec![
        op_local_get(b),
        op_i64_const(nfuncs as i64),
        "I64GeU".to_string(),
        "If { blockty: Type(I64) }".to_string(),
        op_i64_const(fill),
        "Else".to_string(),
        op_local_get(TEST_CTX_LOCAL),
        op_i32_const(scratch_addr as i32),
        op_i64_const(n as i64),
        op_local_get(b),
        "I32WrapI64".to_string(),
        op_call_indirect_norm(compiled_ty, 0),
        "End".to_string(),
    ]
}

/// [`CallMode::Dispatch`]'s `TailCall` window, literal (lib.rs's `Effect::TailCall` Dispatch arm).
fn dispatch_tailcall_window(
    base_slot: i32,
    scratch_addr: u64,
    n: usize,
    helper_slot: u32,
    fill: Value,
    dispatch_ty: &str,
) -> Vec<String> {
    vec![
        op_local_get(TEST_CTX_LOCAL),
        op_local_get(test_local(base_slot)),
        op_i32_const(scratch_addr as i32),
        op_i64_const(n as i64),
        op_i32_const(helper_slot as i32),
        op_call_indirect_norm(dispatch_ty, 0),
        "Drop".to_string(),
        op_i64_const(fill),
        "Return".to_string(),
    ]
}

/// [`CallMode::Table`]'s `TailCall` window, literal (design §3.2 — lib.rs `emit_table_tailcall`).
fn table_tailcall_window(
    base_slot: i32,
    scratch_addr: u64,
    n: usize,
    nfuncs: u32,
    fill: Value,
    compiled_ty: &str,
) -> Vec<String> {
    let b = test_local(base_slot);
    vec![
        op_local_get(b),
        op_i64_const(nfuncs as i64),
        "I64GeU".to_string(),
        "If { blockty: Empty }".to_string(),
        op_i64_const(fill),
        "Return".to_string(),
        "End".to_string(),
        op_local_get(TEST_CTX_LOCAL),
        op_i32_const(scratch_addr as i32),
        op_i64_const(n as i64),
        op_local_get(b),
        "I32WrapI64".to_string(),
        op_return_call_indirect_norm(compiled_ty, 0),
    ]
}

/// **R2** (§5.2 window-localized): for every function in `m` containing at least one
/// `Call`/`TailCall`, T-normalize (operator-level — [`decode_norm_ops`]) both twins and require
/// the sequences to match EXACTLY outside each call site's window, while INSIDE each window
/// independently verifying both sides against their OWN mode's §3.1/§3.2 literal shape
/// ([`dispatch_call_window`]/[`table_call_window`]/[`dispatch_tailcall_window`]/
/// [`table_tailcall_window`]). Call-free functions are silently skipped (R1's job). The window
/// divergence point is NEVER ambiguous: `local.get l(base)`'s local index is always `>= 3`
/// (`TEST_FIRST_LOCAL`), so it can never equal `local.get $ctx` (index 0) — the first op of EITHER
/// mode's window — meaning the "match outside, skip exactly one window's worth inside" walk below
/// cannot mis-fire on an accidental early coincidence. Returns the number of windows checked, for
/// the corpus-level non-vacuity assertion.
fn assert_record_equals_live_windowed(
    isa: &Isa,
    m: &IrModule,
    artifact: &AotArtifact,
    order: &[usize],
) -> usize {
    let man = &artifact.manifest;
    // O-19 §2.6: same A64 exemption guard as R1 (see `assert_record_equals_live`'s comment).
    assert!(
        !man.memory64,
        "record==live R2 is A32-only — A64 corpus is exempt (O-19 §2.6); use the \
         modulo-addrtype differential gate in tests/addrtype.rs instead"
    );
    let live_resolve =
        |p: usize| -> Option<u32> { order.iter().position(|&q| q == p).map(|k| man.host_slots[k]) };
    // Same H* as R1 (helper_slots real, dispatch/tail sentinel — UNUSED inside any window's own
    // content since R2 never asserts equality of raw dispatch/tail bytes, only the Dispatch-mode
    // window's OWN shape against `dispatch_call_window`/`dispatch_tailcall_window`, which take the
    // sentinel as an explicit `helper_slot` parameter instead of reading it off `h_star` blind).
    let h_star = HelperIdx {
        dispatch: 0xFFFF_FFFE,
        tail: 0xFFFF_FFFF,
        binop: man.helper_slots[0],
        convert: man.helper_slots[1],
        unop: man.helper_slots[2],
    };
    let rec_types = func_types(&artifact.bytes);
    let compiled_ty_norm = ty_norm_str(
        &[
            wasmparser::ValType::I32,
            wasmparser::ValType::I32,
            wasmparser::ValType::I64,
        ],
        &[wasmparser::ValType::I64],
    );
    let dispatch_ty_norm = ty_norm_str(
        &[
            wasmparser::ValType::I32,
            wasmparser::ValType::I64,
            wasmparser::ValType::I32,
            wasmparser::ValType::I64,
        ],
        &[wasmparser::ValType::I64],
    );
    let mut windows_checked = 0usize;
    for (j, f) in m.funcs.iter().enumerate() {
        let sites = call_sites(isa, f);
        if sites.is_empty() {
            continue; // R1's job
        }
        let live_bytes = compile_dispatch_fn_with(
            isa,
            j,
            f,
            m.fill,
            m.host_ctx,
            h_star,
            &live_resolve,
            man.scratch[j].addr,
            EmitOpts::new().with_shared_memory(man.shared_memory),
        )
        .expect("live twin must compile (record accepted it — §6 ⊂-consistency)");
        let live_types = func_types(&live_bytes);
        let rec_ops = decode_norm_ops(&artifact.bytes, j, &rec_types);
        let live_ops = decode_norm_ops(&live_bytes, 0, &live_types);
        let scratch_addr = man.scratch[j].addr;

        let (mut ri, mut li) = (0usize, 0usize);
        let mut sites_iter = sites.iter();
        loop {
            match (rec_ops.get(ri), live_ops.get(li)) {
                (Some(r), Some(l)) if r == l => {
                    ri += 1;
                    li += 1;
                }
                (Some(_), Some(_)) => {
                    let site = sites_iter.next().unwrap_or_else(|| {
                        panic!("func {j}: unexpected op divergence at rec[{ri}]/live[{li}] with no call site left")
                    });
                    let (live_win, rec_win) = if site.is_tail {
                        (
                            dispatch_tailcall_window(
                                site.base_slot,
                                scratch_addr,
                                site.n,
                                h_star.tail,
                                m.fill,
                                &dispatch_ty_norm,
                            ),
                            table_tailcall_window(
                                site.base_slot,
                                scratch_addr,
                                site.n,
                                man.nfuncs,
                                m.fill,
                                &compiled_ty_norm,
                            ),
                        )
                    } else {
                        (
                            dispatch_call_window(
                                site.base_slot,
                                scratch_addr,
                                site.n,
                                h_star.dispatch,
                                &dispatch_ty_norm,
                            ),
                            table_call_window(
                                site.base_slot,
                                scratch_addr,
                                site.n,
                                man.nfuncs,
                                m.fill,
                                &compiled_ty_norm,
                            ),
                        )
                    };
                    assert_eq!(
                        &live_ops[li..li + live_win.len()],
                        live_win.as_slice(),
                        "func {j} site {:?}: Dispatch-mode window shape mismatch (§3.1/§3.2/§4.3)",
                        site
                    );
                    assert_eq!(
                        &rec_ops[ri..ri + rec_win.len()],
                        rec_win.as_slice(),
                        "func {j} site {:?}: Table-mode window shape mismatch (§3.1/§3.2)",
                        site
                    );
                    ri += rec_win.len();
                    li += live_win.len();
                    windows_checked += 1;
                }
                (None, None) => break,
                (r, l) => panic!(
                    "func {j}: op-count mismatch outside any window: rec={r:?} live={l:?} at ri={ri} li={li}"
                ),
            }
        }
        assert!(
            sites_iter.next().is_none(),
            "func {j}: not every call site produced a window divergence"
        );
    }
    assert!(
        windows_checked > 0,
        "R2 gate is vacuous: no function in this corpus contains a Call/TailCall"
    );
    windows_checked
}

// --- the A-3 execution harness: table-declared N-in-1, TRAMPOLINE-FREE ------------------------------

/// Store data for the harness. `table` is the AOT module's OWN `otter_table` export, wired
/// immediately after instantiation (§2.1: instantiation runs zero guest code, so there is nothing
/// to race). Since A-3 there is no dispatch/tail signal state at all: a `TailCall` chain resolves
/// ENTIRELY inside the ONE wasm-level `TypedFunc::call` below (`return_call_indirect`), so this
/// struct shrinks to exactly what direct calls need — table/memory handles plus the LIFO staging
/// bump pointer.
struct HarnessSt {
    table: Cell<Option<Table>>,
    memory: Cell<Option<Memory>>,
    staging_top: Cell<u32>,
}

/// Bump-allocate a LIFO span in the harness memory, write `args` into it, and return its address —
/// `calls.rs::call_wasm`'s staging step (structural anchor).
fn stage_args(store: &mut StoreContextMut<'_, HarnessSt>, args: &[Value]) -> u32 {
    let (mem, addr) = {
        let st = store.data();
        let top = st.staging_top.get();
        st.staging_top.set(top + (args.len() as u32) * 8);
        (st.memory.get().expect("memory wired"), top)
    };
    assert!(
        addr as usize + args.len() * 8 <= (MEMORY_PAGES as usize) * 65_536,
        "staging overflow — bump MEMORY_PAGES"
    );
    let mut bytes = Vec::with_capacity(args.len() * 8);
    for a in args {
        bytes.extend_from_slice(&a.to_le_bytes());
    }
    mem.write(&mut *store, addr as usize, &bytes)
        .expect("staging write");
    addr
}

/// Release a LIFO span previously returned by [`stage_args`] (its `words` word count).
fn unstage(store: &mut StoreContextMut<'_, HarnessSt>, addr: u32, words: usize) {
    let top = store.data().staging_top.get();
    debug_assert_eq!(top, addr + (words as u32) * 8, "LIFO discipline violated");
    store.data().staging_top.set(addr);
}

/// One `otter_helpers.<name>` host stub, delegating to the SAME native fn the oracle runs (the
/// canonical-semantics discipline every harness in this crate shares).
fn host_stub(store: &mut Store<HarnessSt>, ptr: usize) -> Func {
    if ptr == common::h_add7 as otter::HostFn1 as usize {
        Func::wrap(&mut *store, |x: i64| -> i64 { common::h_add7(x) })
    } else if ptr == common::h_mix as otter::HostFn2 as usize {
        Func::wrap(&mut *store, |x: i64, y: i64| -> i64 { common::h_mix(x, y) })
    } else if ptr == common::hc_ret_ctx as otter::HostFnCtx1 as usize {
        Func::wrap(&mut *store, |c: i64, x: i64| -> i64 { common::hc_ret_ctx(c, x) })
    } else if ptr == common::hc_mix as otter::HostFnCtx2 as usize {
        Func::wrap(&mut *store, |c: i64, x: i64, y: i64| -> i64 {
            common::hc_mix(c, x, y)
        })
    } else {
        panic!("unknown host fn pointer in resolution log")
    }
}

/// Build the memory + 3 canonical-helper + N host externs for `artifact`, in `manifest.imports`
/// order (§2.2: `manifest.imports` IS the b′ export-requirement spec, built in the SAME order the
/// wasm import section itself lists). `order` is the host-resolution-order log
/// [`compile_with_log`] returns, zipped against `AotImportKind::Host` entries in the SAME order
/// (§2.3 norm: first-resolution order = import order). Since A-3 there is NOTHING ELSE to build —
/// no dispatch/tail trampoline stub, no two-stage wiring (crate doc: `TailCall` resolves entirely
/// inside wasm now).
fn build_externs(
    store: &mut Store<HarnessSt>,
    man: &otter_wasmemit::AotManifest,
    memory: Memory,
    order: &[usize],
) -> Vec<Extern> {
    let binop = Func::wrap(&mut *store, |a: i64, b: i64, ty: u32, op: u32| -> i64 {
        otter::otter_binop(a, b, ty, op)
    });
    let convert = Func::wrap(&mut *store, |a: i64, from: u32, to: u32| -> i64 {
        otter::otter_convert(a, from, to)
    });
    let unop = Func::wrap(&mut *store, |a: i64, ty: u32, op: u32| -> i64 {
        otter::otter_unop(a, ty, op)
    });
    let mut host_order = order.iter();
    man.imports
        .iter()
        .map(|imp| match imp.kind {
            AotImportKind::Memory => Extern::from(memory),
            AotImportKind::Helper(HelperKind::Binop) => Extern::from(binop),
            AotImportKind::Helper(HelperKind::Convert) => Extern::from(convert),
            AotImportKind::Helper(HelperKind::Unop) => Extern::from(unop),
            AotImportKind::Host { .. } => {
                let ptr = *host_order
                    .next()
                    .expect("one host import per resolved ptr, in order");
                Extern::from(host_stub(store, ptr))
            }
            // `AotImportKind` is `#[non_exhaustive]` (design §4.1) — an external crate (this
            // integration test) must still name a fallback arm even though every CURRENT variant
            // is covered above; failing loudly here (never silently skipping an import) is the
            // fail-closed reading of "no wildcard" this crate's own convention would use if it
            // could enforce exhaustiveness across the crate boundary. In particular this is what
            // would fire if a future increment reintroduced a `Trampoline`-shaped kind — a loud
            // test failure here, not a silent extern-list misalignment.
            other => panic!("unhandled AotImportKind in A-3 harness: {other:?}"),
        })
        .collect()
}

/// One instantiated N-in-1 deployment (§9 A-3): the artifact's OWN declared table (every function
/// already registered at its identity slot by the element section) — ONE instantiation, ONE stage,
/// no glue at all.
struct Nin1 {
    store: Store<HarnessSt>,
}

impl Nin1 {
    /// Instantiate `artifact` with the default fuel-metered [`Config`] (`FUEL` per call — see
    /// [`Nin1::call`]).
    fn new(artifact: &AotArtifact, order: &[usize]) -> Nin1 {
        Self::new_with(artifact, order, |_| {})
    }

    /// [`Nin1::new`], with `configure` given a chance to adjust the [`Config`] before the engine
    /// is built (gate ①/② — `Config::max_wasm_stack`/`consume_fuel` overrides for the 2M-deep
    /// bounded/negative-control tests).
    fn new_with(artifact: &AotArtifact, order: &[usize], configure: impl FnOnce(&mut Config)) -> Nin1 {
        let man = &artifact.manifest;
        let mut config = Config::new();
        config.consume_fuel(true);
        configure(&mut config);
        let engine = Engine::new(&config).expect("engine with fuel");
        let mut store = Store::new(
            &engine,
            HarnessSt {
                table: Cell::new(None),
                memory: Cell::new(None),
                staging_top: Cell::new(STAGING_BASE),
            },
        );
        let memory = Memory::new(&mut store, MemoryType::new(MEMORY_PAGES, None)).expect("memory");
        store.data().memory.set(Some(memory));

        let externs = build_externs(&mut store, man, memory, order);

        let module = Module::new(&engine, &artifact.bytes).expect("A-3 artifact must validate");
        let instance = Instance::new(&mut store, &module, &externs).expect("instantiate (2-module link)");
        let table = instance
            .get_table(&mut store, "otter_table")
            .expect("export otter_table (§2.4)");
        store.data().table.set(Some(table));

        Nin1 { store }
    }

    /// Call table slot `slot` — the registered-slot path. `argc` is decoupled from `args.len()`
    /// (the argc-boundary probe: memory may hold MORE words than `argc` declares available —
    /// `args` is marshaled in full, the call itself only claims `argc` of it). Since A-3 this is
    /// the WHOLE story even for deep `TailCall` chains — `return_call_indirect` resolves them
    /// inside this ONE `TypedFunc::call`, no host-side trampoline loop to drain afterward (compare
    /// A-2's `drain_tail`, now deleted).
    fn call(&mut self, slot: usize, args: &[Value], argc: usize) -> Value {
        self.call_with_fuel(slot, args, argc, FUEL)
    }

    /// [`Nin1::call`] with an explicit fuel budget (gate ①'s 2M-deep test — [`DEEP_FUEL`]'s doc).
    fn call_with_fuel(&mut self, slot: usize, args: &[Value], argc: usize, fuel: u64) -> Value {
        assert!(argc <= args.len());
        let mut ctx = self.store.as_context_mut();
        let table = ctx.data().table.get().expect("wired — Nin1::new");
        let slot_ref = table.get(&mut ctx, slot as u64).expect("slot exists");
        let Ref::Func(Some(f)) = slot_ref else {
            panic!("slot {slot} not registered")
        };
        let tf: TypedFunc<(i32, i32, i64), i64> =
            f.typed(&ctx).expect("CompiledFn wasm type (i32 i32 i64) -> i64");
        let addr = stage_args(&mut ctx, args);
        ctx.set_fuel(fuel).expect("set_fuel");
        let v = tf
            .call(&mut ctx, (CTX_JUNK, addr as i32, argc as i64))
            .expect("registered function must not trap");
        unstage(&mut ctx, addr, args.len());
        v
    }

    /// [`Nin1::call_with_fuel`], but returning the raw `Result` instead of `.expect()`ing success
    /// — the fail-closed/negative-control gates need to observe a TRAP (or its absence), not just
    /// a successful value.
    fn try_call_with_fuel(
        &mut self,
        slot: usize,
        args: &[Value],
        argc: usize,
        fuel: u64,
    ) -> wasmtime::Result<Value> {
        assert!(argc <= args.len());
        let mut ctx = self.store.as_context_mut();
        let table = ctx.data().table.get().expect("wired — Nin1::new");
        let slot_ref = table.get(&mut ctx, slot as u64).expect("slot exists");
        let Ref::Func(Some(f)) = slot_ref else {
            panic!("slot {slot} not registered")
        };
        let tf: TypedFunc<(i32, i32, i64), i64> =
            f.typed(&ctx).expect("CompiledFn wasm type (i32 i32 i64) -> i64");
        let addr = stage_args(&mut ctx, args);
        ctx.set_fuel(fuel).expect("set_fuel");
        let result = tf.call(&mut ctx, (CTX_JUNK, addr as i32, argc as i64));
        if result.is_ok() {
            unstage(&mut ctx, addr, args.len());
        }
        result
    }
}

/// The whole A-3 pipeline for one call-free module: compile → record==live R1 gate →
/// instantiate (2-module link, gate ①) → per-function differential vs the T0 oracle.
/// Returns the record==live raw type-imm divergence count (non-vacuity bookkeeping).
fn check_nin1(isa: &Isa, m: &IrModule, per_fn_argsets: &[Vec<Vec<Value>>]) -> usize {
    assert!(m.funcs.len() >= 2, "gate norm: multi-function modules");
    assert_eq!(per_fn_argsets.len(), m.funcs.len());
    let (artifact, order) = compile_with_log(isa, m, &AotOpts::new(SCRATCH_BASE));
    let diffs = assert_record_equals_live(isa, m, &artifact, &order);
    let mut h = Nin1::new(&artifact, &order);
    for (j, argsets) in per_fn_argsets.iter().enumerate() {
        for args in argsets {
            let got = h.call(j, args, args.len());
            let want = oracle(isa, &m.funcs[j], args, m.fill, m.host_ctx);
            assert_eq!(
                got, want,
                "N-in-1 wasm ≠ oracle: func {j} args={args:?} fill={:#x} host_ctx={:#x}\nfunc={:?}",
                m.fill, m.host_ctx, m.funcs[j]
            );
        }
    }
    diffs
}

/// A tiny call-free companion function (so single-function corpus cases still ship as ≥2-function
/// modules without disturbing their own (fill, host_ctx) binding): returns its untouched local 0,
/// i.e. `fill`.
fn probe_fn() -> FuncDef {
    func(0, 1, vec![Inst::new(HALT, 0, 0, 0)])
}

// --- gate 1: N-in-1 differential on the existing corpus (call-free) ------------------------------

/// Explicit multi-function module: dispatcher-path loop + host chain + consts folding + argc/fill
/// probe, all in ONE module (shared fill/host_ctx), each function reached through the artifact's
/// OWN declared table. Includes the argc-boundary probe (absent params stay `fill`; extras
/// ignored).
#[test]
fn nin1_explicit_multifunction_module_matches_oracle() {
    let isa = common::isa();
    // f0: sum 1..=n — backward JumpIf loop (dispatcher path).
    let loop_sum = func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 1, 0, 0),
            Inst::new(MOV_IMM, 2, 1, 0),
            Inst::new(bin_opcode(3, 0), 1, 1, 0),
            Inst::new(bin_opcode(3, 1), 0, 0, 2),
            Inst::new(JUMP_IF, 0, 2, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    );
    // f1: host chain (Host1 → Host2 → HostCtx2) — straight-line, resolver-dependent.
    let hosts = func(
        2,
        5,
        vec![
            Inst::new(HOST1, 2, 0, 0),
            Inst::new(HOST2, 3, 2, 1),
            Inst::new(HOSTCTX2, 4, 3, 1),
            Inst::new(HALT, 4, 0, 0),
        ],
    );
    // f2: consts land after params and fold correctly.
    let consts_fn = FuncDef {
        nparams: 1,
        consts: vec![10, -20, 0x0000_7FFF_FFFF_FFFF],
        nlocals: 6,
        code: vec![
            Inst::new(bin_opcode(3, 0), 4, 1, 2),
            Inst::new(bin_opcode(3, 0), 4, 4, 3),
            Inst::new(bin_opcode(3, 0), 4, 4, 0),
            Inst::new(HALT, 4, 0, 0),
        ],
    };
    // f3: returns param 2 of nparams=3 — the argc-guard probe.
    let argc_probe = func(3, 3, vec![Inst::new(HALT, 2, 0, 0)]);
    let m = IrModule {
        funcs: vec![loop_sum, hosts, consts_fn, argc_probe],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    check_nin1(
        &isa,
        &m,
        &[
            vec![vec![1], vec![7], vec![50]],
            vec![vec![1, 2], vec![-7, 1 << 40], vec![i64::MIN, i64::MAX]],
            vec![vec![5], vec![-3]],
            vec![vec![11, 22, 33]],
        ],
    );

    // argc decoupled from the marshalled words: absent params stay fill, extras are ignored.
    let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    let mut h = Nin1::new(&artifact, &order);
    let args = [11, 22, 33, 44, 55];
    for argc in [0usize, 1, 2] {
        assert_eq!(
            h.call(3, &args, argc),
            FILL,
            "param 2 must stay fill at argc={argc}"
        );
    }
    assert_eq!(h.call(3, &args, 3), 33);
    assert_eq!(h.call(3, &args, 5), 33, "extra args ignored");
}

/// The straight-line fuzz corpus, SAME generator + seed + count as
/// `differential::fuzz_straightline_matches_oracle`, each case shipped as a 2-function module
/// (case function + probe) with the case's own fill/host_ctx — corpus-identical, module shape
/// N-in-1.
#[test]
fn nin1_fuzz_straightline_same_corpus_matches_oracle() {
    let isa = common::isa();
    let mut next = lcg(0x9E37_79B9_7F4A_7C15);
    for i in 0..400 {
        let (f, args, fill, host_ctx) = gen_straightline(&mut next);
        let m = IrModule {
            funcs: vec![f, probe_fn()],
            fill,
            host_ctx,
            entry: 0,
        };
        let argsets = vec![vec![args], vec![vec![]]];
        // A failure inside check_nin1 already prints the func; tag the case index too.
        let _ = i;
        check_nin1(&isa, &m, &argsets);
    }
}

/// The random-CFG fuzz corpus, SAME generator + seed + count as
/// `differential::fuzz_cfg_matches_oracle` (irreducible shapes, termination-guaranteed), N-in-1.
#[test]
fn nin1_fuzz_cfg_same_corpus_matches_oracle() {
    let isa = common::isa();
    let mut next = lcg(0xA5F1_5EED_0D15_EA5E);
    for _ in 0..300 {
        let (f, args, fill, host_ctx) = gen_cfg(&mut next);
        let m = IrModule {
            funcs: vec![f, probe_fn()],
            fill,
            host_ctx,
            entry: 0,
        };
        check_nin1(&isa, &m, &[vec![args], vec![vec![]]]);
    }
}

/// Cross-function type-interner interaction: PAIRS of fuzz functions in one module (the case
/// where the module-scope interner's global accumulation order genuinely diverges from each
/// function's own first-use order) — a subset of the same stream, plus the non-vacuity floor:
/// across the paired corpus the T-normalization must have absorbed at least one RAW type-index
/// divergence, or the gate is not testing what §5.2 says it absorbs.
#[test]
fn nin1_fuzz_pairs_share_one_interner_and_t_normalization_is_non_vacuous() {
    let isa = common::isa();
    let mut next = lcg(0x9E37_79B9_7F4A_7C15);
    let mut raw_divergences = 0usize;
    for _ in 0..50 {
        let (fa, args_a, fill, host_ctx) = gen_straightline(&mut next);
        let (fb, args_b, _fill_b, _hc_b) = gen_straightline(&mut next);
        // The pair shares ONE module-level (fill, host_ctx) — the module owns those words.
        let m = IrModule {
            funcs: vec![fa, fb],
            fill,
            host_ctx,
            entry: 0,
        };
        raw_divergences += check_nin1(&isa, &m, &[vec![args_a], vec![args_b]]);
    }
    let mut next = lcg(0xA5F1_5EED_0D15_EA5E);
    for _ in 0..25 {
        let (fa, args_a, fill, host_ctx) = gen_cfg(&mut next);
        let (fb, args_b, _fill_b, _hc_b) = gen_cfg(&mut next);
        let m = IrModule {
            funcs: vec![fa, fb],
            fill,
            host_ctx,
            entry: 0,
        };
        raw_divergences += check_nin1(&isa, &m, &[vec![args_a], vec![args_b]]);
    }
    assert!(
        raw_divergences > 0,
        "T-normalization never fired across the paired corpus — the divergence it exists to \
         absorb (module-scope interning) has vanished; re-examine the gate"
    );
}

// --- gate ①: call-carrying execution through the declared table (NEW since A-2) ------------------

/// `fact(n)` — self-recursive `Call` (the §2.5 scratch-reuse shape).
fn fact_fn() -> FuncDef {
    let (lt, sub, mul) = (bin_opcode(3, 4), bin_opcode(3, 1), bin_opcode(3, 2));
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 5, 2, 0),
            Inst::new(lt, 1, 0, 5),
            Inst::new(JUMP_IF, 1, 9, 0),
            Inst::new(MOV_IMM, 5, 1, 0),
            Inst::new(sub, 3, 0, 5),
            Inst::new(MOV_IMM, 2, 0, 0),
            Inst::new(CALL, 4, 2, 1),
            Inst::new(mul, 4, 0, 4),
            Inst::new(RET, 4, 0, 0),
            Inst::new(MOV_IMM, 4, 1, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    }
}

/// `countdown`-shaped self/cross `TailCall` (callee index and base value parameterized).
fn tail_fn(callee_idx: i32, base_val: i32) -> FuncDef {
    let (eq, sub) = (bin_opcode(3, 5), bin_opcode(3, 1));
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(MOV_IMM, 4, 0, 0),
            Inst::new(eq, 1, 0, 4),
            Inst::new(JUMP_IF, 1, 7, 0),
            Inst::new(MOV_IMM, 4, 1, 0),
            Inst::new(sub, 3, 0, 4),
            Inst::new(MOV_IMM, 2, callee_idx, 0),
            Inst::new(TAILCALL, 2, 1, 0),
            Inst::new(MOV_IMM, 4, base_val, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    }
}

/// A wide-argc caller (`argc` marshalled words) + its order-sensitive folding callee.
fn marshal_pair(n: usize) -> Vec<FuncDef> {
    let (add, mul) = (bin_opcode(3, 0), bin_opcode(3, 2));
    let ni = n as i32;
    let mut caller_code = vec![Inst::new(MOV_IMM, ni, 1, 0)];
    for k in 0..ni {
        caller_code.push(Inst::new(common::COPY, ni + 1 + k, k, 0));
    }
    caller_code.push(Inst::new(CALL, 2 * ni + 1, ni, ni));
    caller_code.push(Inst::new(RET, 2 * ni + 1, 0, 0));
    let caller = FuncDef {
        nparams: n,
        consts: vec![],
        nlocals: 2 * n + 2,
        code: caller_code,
    };
    let mut callee_code = vec![Inst::new(MOV_IMM, ni, 0, 0), Inst::new(MOV_IMM, ni + 1, 31, 0)];
    for k in 0..ni {
        callee_code.push(Inst::new(mul, ni, ni, ni + 1));
        callee_code.push(Inst::new(add, ni, ni, k));
    }
    callee_code.push(Inst::new(RET, ni, 0, 0));
    let callee = FuncDef {
        nparams: n,
        consts: vec![],
        nlocals: n + 2,
        code: callee_code,
    };
    vec![caller, callee]
}

/// An `argc = 0` caller (the dead-`args`-operand shape: `words == 0` yet the scratch address IS
/// baked at the site) + its fill-observing callee.
fn zero_argc_pair() -> Vec<FuncDef> {
    let caller = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 2,
        code: vec![
            Inst::new(MOV_IMM, 0, 1, 0),
            Inst::new(CALL, 1, 0, 0),
            Inst::new(RET, 1, 0, 0),
        ],
    };
    let callee = func(1, 1, vec![Inst::new(RET, 0, 0, 0)]);
    vec![caller, callee]
}

/// A shallow-corpus differential over the ①-⑤ shapes gate ⑤ (design §9 A-3 row) names by name —
/// self-recursive `Call`, `TailCall`, mutual `TailCall`, wide-argc non-tail `Call`, and the
/// dead-args-operand `argc=0` shape: a helper/host-providing stub (native `Func::wrap` closures
/// registered as the AOT module's import externs) combined-instantiated with the AOT artifact
/// executes every function through the artifact's OWN declared table and matches the
/// multi-function T0 oracle. Depth stays shallow here on purpose — this test's job is corpus
/// BREADTH (every call shape at least once), not depth; the 2M-deep bound is a SEPARATE, dedicated
/// test (`mutual_tail_recursion_2m_deep_stays_bounded_in_a_64kib_wasm_stack`, gate ①/②).
#[test]
fn nin1_call_carrying_corpus_executes_through_declared_table_and_matches_oracle() {
    let isa = common::isa();
    #[allow(clippy::type_complexity)] // a one-off local corpus table, a `type` alias would obscure it
    let corpus: Vec<(&'static str, IrModule, Vec<(usize, Vec<Vec<Value>>)>)> = vec![
        (
            "fact self-recursive Call",
            IrModule {
                funcs: vec![fact_fn(), probe_fn()],
                fill: FILL,
                host_ctx: HOST_CTX,
                entry: 0,
            },
            vec![(0, vec![vec![0], vec![1], vec![2], vec![3], vec![5], vec![10]])],
        ),
        (
            "countdown self TailCall (2M-bound is a separate dedicated test — shallow here)",
            IrModule {
                funcs: vec![tail_fn(0, 42), probe_fn()],
                fill: FILL,
                host_ctx: HOST_CTX,
                entry: 0,
            },
            vec![(0, vec![vec![0], vec![1], vec![5], vec![20]])],
        ),
        (
            "even/odd mutual TailCall",
            IrModule {
                funcs: vec![tail_fn(1, 1), tail_fn(0, 0)],
                fill: FILL,
                host_ctx: HOST_CTX,
                entry: 0,
            },
            vec![
                (0, vec![vec![0], vec![1], vec![2], vec![7], vec![20]]),
                (1, vec![vec![0], vec![1], vec![2], vec![7], vec![20]]),
            ],
        ),
        (
            "marshal-8 non-tail Call, order-sensitive fold",
            {
                let m = marshal_pair(8);
                IrModule {
                    funcs: m,
                    fill: FILL,
                    host_ctx: HOST_CTX,
                    entry: 0,
                }
            },
            vec![(0, vec![vec![1, 2, 3, 4, 5, 6, 7, 8]])],
        ),
        (
            "zero-argc dead-args-operand Call",
            {
                let m = zero_argc_pair();
                IrModule {
                    funcs: m,
                    fill: FILL,
                    host_ctx: HOST_CTX,
                    entry: 0,
                }
            },
            vec![(0, vec![vec![]])],
        ),
    ];
    for (name, m, cases) in corpus {
        let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
        Validator::new()
            .validate_all(&artifact.bytes)
            .unwrap_or_else(|e| panic!("{name}: artifact must validate: {e}"));
        let mut h = Nin1::new(&artifact, &order);
        for (j, argsets) in cases {
            for args in argsets {
                let got = h.call(j, &args, args.len());
                let mut mj = m.clone();
                mj.entry = j;
                let want = run_module(&isa, &mj, &args);
                assert_eq!(got, want, "{name}: func {j} args={args:?}");
            }
        }
    }
}

// --- gate ①/②: 2M-deep bounded tail recursion + negative control — O-14 A-3 ------------------------

/// The [`tail_fn`] shape, but with an ORDINARY (non-tail) `Call` in place of `TailCall` — the
/// negative control gate ② needs: an IDENTICAL guarded-`call_indirect` shape at the wasm level
/// (both this and `tail_fn` lower through `CallMode::Table`'s `i64.ge_u` guard), but
/// `call_indirect`, never `return_call_indirect`, so every recursive step grows a NEW wasm frame.
/// This isolates the boundedness claim to the tail-vs-non-tail axis specifically: a stack overflow
/// here cannot be blamed on "there is no guard" (gate ③ covers that axis with its OWN, separate
/// corpus) — it can only be `return_call_indirect`'s absence.
fn nontail_call_fn(callee_idx: i32, base_val: i32) -> FuncDef {
    let (eq, sub) = (bin_opcode(3, 5), bin_opcode(3, 1));
    FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 6, // 0=n 1=t 2=block0(callee) 3=block1(arg) 4=res 5=tmp
        code: vec![
            Inst::new(MOV_IMM, 5, 0, 0),
            Inst::new(eq, 1, 0, 5),
            Inst::new(JUMP_IF, 1, 8, 0),
            Inst::new(MOV_IMM, 5, 1, 0),
            Inst::new(sub, 3, 0, 5),
            Inst::new(MOV_IMM, 2, callee_idx, 0),
            Inst::new(CALL, 4, 2, 1), // ORDINARY Call — the ONE structural difference from tail_fn
            Inst::new(RET, 4, 0, 0),
            Inst::new(MOV_IMM, 4, base_val, 0),
            Inst::new(RET, 4, 0, 0),
        ],
    }
}

/// Gate ① — [스파이크] M2's exact figures: mutual `TailCall` recursion (`even`/`odd`, [`tail_fn`])
/// through the artifact's declared table, in a [`TINY_WASM_STACK`]-sized (64 KiB) wasm stack,
/// completes at depth 2,000,000/2,000,001. Boundedness is BY CONSTRUCTION here (design §3.2):
/// `return_call_indirect` is itself a wasm tail call — the engine's own call stack does not grow
/// per recursive step, so no host-side trampoline loop is needed to keep it bounded (contrast
/// `calls.rs`'s JIT/Dispatch-mode 1M-deep gate, which NEEDS its `mini_dispatch` loop for exactly
/// this reason — design §4.3).
#[test]
fn mutual_tail_recursion_2m_deep_stays_bounded_in_a_64kib_wasm_stack() {
    let isa = common::isa();
    let m = IrModule {
        funcs: vec![tail_fn(1, 1), tail_fn(0, 0)],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    let mut h = Nin1::new_with(&artifact, &order, |c| {
        c.max_wasm_stack(TINY_WASM_STACK);
    });
    assert_eq!(
        h.call_with_fuel(0, &[2_000_000], 1, DEEP_FUEL),
        1,
        "even(2,000,000)"
    );
    assert_eq!(
        h.call_with_fuel(0, &[2_000_001], 1, DEEP_FUEL),
        0,
        "even(2,000,001) — odd depth, opposite parity"
    );
    // Cross-check against the T0 oracle at a SHALLOW depth only — the oracle itself is not re-run
    // at 2M here (redundant with the shallow-depth coverage
    // `nin1_call_carrying_corpus_executes_through_declared_table_and_matches_oracle` already gives
    // this SAME `tail_fn` shape; this test's OWN job is depth/boundedness, not a second
    // correctness proof).
    let mut mj = m.clone();
    mj.entry = 0;
    assert_eq!(
        h.call(0, &[20], 1),
        run_module(&isa, &mj, &[20]),
        "even(20) oracle cross-check"
    );
}

/// Gate ② — the negative control [`mutual_tail_recursion_2m_deep_stays_bounded_in_a_64kib_wasm_stack`]
/// needs: the SAME shape, SAME guard, SAME [`TINY_WASM_STACK`], but [`nontail_call_fn`] (ordinary
/// `Call`) instead of `TailCall` — must overflow the wasm stack well before matching depth,
/// demonstrating that `return_call_indirect` (not merely "there is a guard") is what gate ①'s
/// boundedness rests on. [스파이크] M2 measured the SAME contrast (design §1.1-3: "음성 대조
/// plain call_indirect판은 Trap::StackOverflow").
#[test]
fn nontail_deep_recursion_overflows_the_same_64kib_wasm_stack() {
    let isa = common::isa();
    let m = IrModule {
        funcs: vec![nontail_call_fn(1, 1), nontail_call_fn(0, 0)],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    // Sanity: this shape is otherwise CORRECT (matches the oracle) at a shallow depth — the
    // eventual overflow below is a STACK limit, not a lowering bug.
    let mut mj = m.clone();
    mj.entry = 0;
    assert_eq!(run_module(&isa, &mj, &[20]), 1, "even(20) sanity (oracle)");

    let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    let mut h = Nin1::new_with(&artifact, &order, |c| {
        c.max_wasm_stack(TINY_WASM_STACK);
    });
    let err = h
        .try_call_with_fuel(0, &[2_000_000], 1, DEEP_FUEL)
        .expect_err("2,000,000-deep NON-tail recursion must overflow a 64 KiB wasm stack");
    assert_eq!(
        *err.downcast_ref::<Trap>().expect("must be a wasmtime Trap"),
        Trap::StackOverflow,
        "the negative control's failure mode must specifically be stack exhaustion"
    );
}

// --- gate ③: fail-closed shapes, Call AND TailCall — O-14 A-3 --------------------------------------

/// The four fail-closed callee-word shapes gate ③ (design §9 A-3 row) names: OOB and plain
/// negative are representable by a single sign-extended `MOV_IMM`; the two FULL-WIDTH shapes are
/// NOT (§3.2a/A-C1's exact truncation-hazard byte patterns: `2^32 + k` and `-2^32`) — built from
/// `Bin` `Mul`/`Add` and `Un` `Neg` at I64 instead ([`push_garbage_callee`]'s doc has the
/// construction).
#[derive(Clone, Copy, Debug)]
enum GarbageShape {
    /// `99` — `>= nfuncs` for every module this suite builds (2 functions).
    Oob,
    /// `-1` — plain negative, representable by a single sign-extended `MOV_IMM`.
    Negative,
    /// `2^32 + 1` — NOT representable by a single sign-extended `MOV_IMM` (design §3.1's
    /// relevance note: this is the EXACT shape a pre-A-C1 wasm32 `as usize` truncation would have
    /// mis-dispatched to function 1, since `(2^32 + 1) mod 2^32 == 1 < nfuncs`; the wasm-native
    /// `i64.ge_u` guard this crate emits has no such cast to truncate).
    FullWidePos,
    /// `-2^32` — the negative full-width counterpart (`(-2^32) mod 2^32 == 0`, function 0's own
    /// index — the same truncation-hazard shape, other sign).
    FullWideNeg,
}

/// Append `local[2] = <garbage callee word>` per [`GarbageShape`] — locals 3/4 are scratch for the
/// full-width shapes' two-`MOV_IMM`-plus-`Bin` construction (`65536 * 65536 = 2^32`, `Mul` at
/// I64) — entirely via EXISTING, elsewhere-proven Bin/Neg lowering; no ISA changes needed.
fn push_garbage_callee(code: &mut Vec<Inst>, shape: GarbageShape) {
    let mul64 = bin_opcode(3, 2); // I64 Mul
    let add64 = bin_opcode(3, 0); // I64 Add
    let neg64 = neg_opcode(3); // I64 Neg
    match shape {
        GarbageShape::Oob => code.push(Inst::new(MOV_IMM, 2, 99, 0)),
        GarbageShape::Negative => code.push(Inst::new(MOV_IMM, 2, -1, 0)),
        GarbageShape::FullWidePos => {
            code.push(Inst::new(MOV_IMM, 3, 65536, 0));
            code.push(Inst::new(MOV_IMM, 4, 65536, 0));
            code.push(Inst::new(mul64, 2, 3, 4)); // local2 = 2^32
            code.push(Inst::new(MOV_IMM, 3, 1, 0));
            code.push(Inst::new(add64, 2, 2, 3)); // local2 = 2^32 + 1
        }
        GarbageShape::FullWideNeg => {
            code.push(Inst::new(MOV_IMM, 3, 65536, 0));
            code.push(Inst::new(MOV_IMM, 4, 65536, 0));
            code.push(Inst::new(mul64, 2, 3, 4)); // local2 = 2^32
            code.push(Inst::new(neg64, 2, 2, 0)); // local2 = -2^32
        }
    }
}

fn garbage_call_fn(shape: GarbageShape) -> FuncDef {
    let mut code = Vec::new();
    push_garbage_callee(&mut code, shape);
    code.push(Inst::new(CALL, 5, 2, 0)); // dest=5, base=2, argc=0
    code.push(Inst::new(RET, 5, 0, 0));
    FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 6,
        code,
    }
}

fn garbage_tailcall_fn(shape: GarbageShape) -> FuncDef {
    let mut code = Vec::new();
    push_garbage_callee(&mut code, shape);
    code.push(Inst::new(TAILCALL, 2, 0, 0)); // base=2, argc=0
    FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 5,
        code,
    }
}

/// Gate ③ (design §9 A-3 row): OOB (`>= nfuncs`), plain negative, and the two FULL-WIDTH shapes
/// `2^32 + k` / `-2^32` all yield `fill` — NEVER a trap — on BOTH `Call` and `TailCall`. The
/// module has exactly 2 functions (the garbage-callee fn + [`probe_fn`]), so every shape here is
/// unambiguously `>= nfuncs`.
#[test]
fn fail_closed_four_shapes_call_and_tailcall_yield_fill_never_trap() {
    let isa = common::isa();
    for shape in [
        GarbageShape::Oob,
        GarbageShape::Negative,
        GarbageShape::FullWidePos,
        GarbageShape::FullWideNeg,
    ] {
        for (name, f) in [
            ("Call", garbage_call_fn(shape)),
            ("TailCall", garbage_tailcall_fn(shape)),
        ] {
            let m = IrModule {
                funcs: vec![f, probe_fn()],
                fill: FILL,
                host_ctx: HOST_CTX,
                entry: 0,
            };
            let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
            Validator::new()
                .validate_all(&artifact.bytes)
                .unwrap_or_else(|e| panic!("{name} {shape:?}: artifact must validate: {e}"));
            let mut h = Nin1::new(&artifact, &order);
            let got = h.call(0, &[], 0);
            assert_eq!(
                got, FILL,
                "{name} {shape:?}: garbage callee must yield fill, never trap"
            );
        }
    }
}

// --- gate ⑤: mixed Call-then-TailCall-tree differential — O-14 A-3 ---------------------------------

/// The engine's `mixed_module` shape (`calls.rs::mixed_env`, ported to a `compile_module` AOT
/// artifact): `main` makes an ordinary `Call` into `helper`; `helper` SELF-tail-recurses to the
/// base case, then tail-calls INTO `leaf` (cross-function `TailCall`) — a Call-then-(self-Tail
/// Call)*-then-(cross-TailCall) tree in ONE artifact, result always 999. `Halt` appears ONLY in
/// the top frame (`main`), matching the oracle's scope caveat (`calls.rs` crate doc, §8a B#7).
fn mixed_funcs() -> Vec<FuncDef> {
    let (eq, sub) = (bin_opcode(3, 5), bin_opcode(3, 1));
    let main = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4, // 0=n 1=block0 2=block1 3=r
        code: vec![
            Inst::new(MOV_IMM, 1, 1, 0),      // block0 = helper
            Inst::new(common::COPY, 2, 0, 0), // block1 = n
            Inst::new(CALL, 3, 1, 1),         // r = helper(n) — ordinary, non-tail
            Inst::new(HALT, 3, 0, 0),         // top-frame Halt only
        ],
    };
    let helper = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5, // 0=n 1=t 2=block0 3=block1 4=tmp
        code: vec![
            Inst::new(MOV_IMM, 4, 0, 0),
            Inst::new(eq, 1, 0, 4),
            Inst::new(JUMP_IF, 1, 7, 0),
            Inst::new(MOV_IMM, 4, 1, 0),
            Inst::new(sub, 3, 0, 4),       // block1 = n - 1
            Inst::new(MOV_IMM, 2, 1, 0),   // block0 = helper (self)
            Inst::new(TAILCALL, 2, 1, 0),  // self tail call
            Inst::new(MOV_IMM, 2, 2, 0),   // block0 = leaf
            Inst::new(MOV_IMM, 3, 999, 0), // block1 = 999
            Inst::new(TAILCALL, 2, 1, 0),  // cross-function tail call
        ],
    };
    let leaf = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 1,
        code: vec![Inst::new(RET, 0, 0, 0)],
    };
    vec![main, helper, leaf]
}

/// Gate ⑤ (design §9 A-3 row: "TailCall 섞인 트리"): the mixed Call+self-TailCall+cross-TailCall
/// tree matches the multi-function T0 oracle at every tested `n`.
#[test]
fn mixed_call_then_tailcall_tree_matches_oracle() {
    let isa = common::isa();
    let m = IrModule {
        funcs: mixed_funcs(),
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    Validator::new()
        .validate_all(&artifact.bytes)
        .expect("mixed artifact must validate");
    let mut h = Nin1::new(&artifact, &order);
    for n in [0i64, 1, 5, 300] {
        let got = h.call(0, &[n], 1);
        assert_eq!(got, 999, "mixed n={n}");
        let mut mj = m.clone();
        mj.entry = 0;
        assert_eq!(got, run_module(&isa, &mj, &[n]), "oracle cross-check n={n}");
    }
}

// --- gate 2 / gate ⑥: record==live R2 (window-localized) on call-carrying modules — O-14 A-3 -------

/// **R2** (§5.2, gate ⑥) over the CALL-CARRYING corpus: every function's Table-mode/Dispatch-mode
/// twins must match EXACTLY outside each `Call`/`TailCall` site's window, and each window must
/// equal its OWN mode's §3.1/§3.2 literal shape inside — see [`assert_record_equals_live_windowed`]
/// for the full argument. The crafted call-free "interner-divergence" pair that used to live in
/// this corpus (proving T-normalization non-vacuous) is NOT re-tested here: it has zero
/// `Call`/`TailCall` sites, so it was always an R1 case wearing this test's clothes —
/// `nin1_fuzz_pairs_share_one_interner_and_t_normalization_is_non_vacuous` already covers that
/// non-vacuity claim for R1's call-free corpus.
#[test]
fn record_equals_live_r2_windowed_on_call_carrying_modules() {
    let isa = common::isa();
    let corpus: Vec<(&'static str, Vec<FuncDef>)> = vec![
        ("fact+probe", vec![fact_fn(), probe_fn()]),
        ("countdown+probe", vec![tail_fn(0, 42), probe_fn()]),
        ("even/odd", vec![tail_fn(1, 1), tail_fn(0, 0)]),
        ("marshal-8", marshal_pair(8)),
        ("marshal-257", marshal_pair(257)),
        ("zero-argc", zero_argc_pair()),
    ];
    let mut windows_total = 0usize;
    for (name, funcs) in corpus {
        let m = IrModule {
            funcs,
            fill: FILL,
            host_ctx: HOST_CTX,
            entry: 0,
        };
        let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
        Validator::new()
            .validate_all(&artifact.bytes)
            .unwrap_or_else(|e| panic!("{name}: artifact must validate: {e}"));
        windows_total += assert_record_equals_live_windowed(&isa, &m, &artifact, &order);
    }
    assert_eq!(
        windows_total, 7,
        "fact=1, countdown=1, even/odd=2, marshal-8=1, marshal-257=1, zero-argc=1 (one window per \
         Call/TailCall site — the callees in each pair are call-free, contributing 0)"
    );
}

/// Gate ④ (design §3.1): the `if (result i64)`/`else` block-type shape for `Call` (as opposed to
/// `TailCall`'s plain `if`/no result, guard-`return`ing instead) is not just spec-legal but
/// EMITTED and VALIDATED. [`assert_record_equals_live_windowed`] already pins this exactly (its
/// `table_call_window`/`table_tailcall_window` literal shapes), so this test is a short, directly
/// readable second witness — an independent human-eyeball anchor, not new coverage.
#[test]
fn call_uses_if_result_i64_tailcall_uses_plain_if_and_both_validate() {
    let isa = common::isa();
    let m = IrModule {
        funcs: vec![fact_fn(), tail_fn(0, 42)],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifact, _) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    Validator::new()
        .validate_all(&artifact.bytes)
        .expect("artifact with a Call (if-result-i64) and a TailCall (plain if) must validate");
    let fact_ops = body_ops(&artifact.bytes, 0); // fact: self-recursive Call
    let tail_ops = body_ops(&artifact.bytes, 1); // countdown: self TailCall
    assert!(
        fact_ops.iter().any(|op| op == "If { blockty: Type(I64) }"),
        "fact's Call site must use the `if (result i64)` block-type shape (§3.1)"
    );
    assert!(
        tail_ops.iter().any(|op| op == "If { blockty: Empty }"),
        "countdown's TailCall site must use the plain (no-result) `if` shape (§3.2)"
    );
    assert!(
        tail_ops.iter().any(|op| op.starts_with("ReturnCallIndirect")),
        "countdown's TailCall site must lower to `return_call_indirect` (§3.2)"
    );
}

// --- gate 3: structure + manifest ------------------------------------------------------------------

/// The A-3 FINAL-CONTRACT module shape (§2.1/§2.2/§2.3/§2.4): sections exactly
/// type/import/function/table/export/element/code, imports = memory + 3 helpers + N hosts (no
/// provisional trampolines — retired whole), every function `CompiledFn`-typed, exports
/// `otter_entry` + `otter_table`, table declared `min=max=table_size` (no more `+2`), element =
/// ONE active MVP-encoded segment with the full identity-mapping function-index list, no start —
/// and the manifest carries the §2.3 canonical layout (`helper_slots`/`host_slots`), the §2.5
/// scratch map, and the version anchor.
#[test]
fn nin1_structure_and_manifest_invariants() {
    let isa = common::isa();
    // f0: host user (1 host fn); f1: Call argc=2 (words 2); f2: call-free.
    let f0 = func(1, 2, vec![Inst::new(HOST1, 1, 0, 0), Inst::new(HALT, 1, 0, 0)]);
    let f1 = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(MOV_IMM, 1, 2, 0),
            Inst::new(common::COPY, 2, 0, 0),
            Inst::new(common::COPY, 3, 0, 0),
            Inst::new(CALL, 4, 1, 2),
            Inst::new(RET, 4, 0, 0),
        ],
    };
    let f2 = probe_fn();
    let m = IrModule {
        funcs: vec![f0, f1, f2],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 1,
    };
    let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    let man = &artifact.manifest;

    // -- wasm structure --
    let mut sections: Vec<&'static str> = Vec::new();
    let mut func_type_indices: Vec<u32> = Vec::new();
    let mut n_func_imports = 0u32;
    for payload in Parser::new(0).parse_all(&artifact.bytes) {
        match payload.expect("artifact must parse") {
            Payload::Version { .. } | Payload::End(_) => {}
            Payload::TypeSection(_) => sections.push("type"),
            Payload::ImportSection(r) => {
                sections.push("import");
                let imports: Vec<_> = r.into_imports().map(|i| i.expect("import")).collect();
                assert_eq!(
                    imports.len(),
                    1 + 3 + man.host_slots.len(),
                    "memory + 3 helpers + N hosts (no provisional trampolines since A-3)"
                );
                assert_eq!((imports[0].module, imports[0].name), ("otter", "memory"));
                match imports[0].ty {
                    TypeRef::Memory(mem) => assert!(!mem.shared && mem.initial == 0 && mem.maximum.is_none()),
                    ref other => panic!("otter.memory must be a memory import, got {other:?}"),
                }
                let expect_helpers = [
                    ("otter_helpers", "otter_binop"),
                    ("otter_helpers", "otter_convert"),
                    ("otter_helpers", "otter_unop"),
                ];
                for (i, want) in expect_helpers.iter().enumerate() {
                    assert_eq!((imports[1 + i].module, imports[1 + i].name), *want);
                    assert!(matches!(imports[1 + i].ty, TypeRef::Func(_)));
                }
                for (k, imp) in imports[4..].iter().enumerate() {
                    assert_eq!(imp.module, "otter_host");
                    assert_eq!(imp.name, host_name(order[k]).expect("known host"));
                    assert!(matches!(imp.ty, TypeRef::Func(_)));
                }
                n_func_imports = imports
                    .iter()
                    .filter(|i| matches!(i.ty, TypeRef::Func(_)))
                    .count() as u32;
            }
            Payload::FunctionSection(r) => {
                sections.push("function");
                func_type_indices = r.into_iter().map(|i| i.expect("func")).collect();
            }
            Payload::TableSection(r) => {
                sections.push("table");
                let tables: Vec<_> = r.into_iter().map(|t| t.expect("table")).collect();
                assert_eq!(tables.len(), 1);
                let ty = tables[0].ty;
                assert!(ty.element_type.is_func_ref());
                assert_eq!(
                    ty.initial,
                    u64::from(man.table_size),
                    "declared min = T, the §2.3 canonical size (no more +2 since A-3)"
                );
                assert_eq!(
                    ty.maximum,
                    Some(u64::from(man.table_size)),
                    "min=max, fixed (§2.3)"
                );
            }
            Payload::ExportSection(r) => {
                sections.push("export");
                let exports: Vec<_> = r.into_iter().map(|e| e.expect("export")).collect();
                assert_eq!(exports.len(), 2, "A-2: otter_entry + otter_table");
                assert_eq!(exports[0].name, "otter_entry");
                assert_eq!(exports[0].kind, wasmparser::ExternalKind::Func);
                assert_eq!(exports[1].name, "otter_table");
                assert_eq!(exports[1].kind, wasmparser::ExternalKind::Table);
                assert_eq!(exports[1].index, 0);
            }
            Payload::ElementSection(r) => {
                sections.push("element");
                let elems: Vec<_> = r.into_iter().map(|e| e.expect("elem")).collect();
                assert_eq!(elems.len(), 1, "ONE active segment (§2.3)");
                let e = &elems[0];
                let wasmparser::ElementKind::Active {
                    table_index,
                    offset_expr,
                } = &e.kind
                else {
                    panic!("must be an active segment")
                };
                assert!(table_index.is_none(), "MVP encoding: implicit table 0");
                let mut reader = offset_expr.get_operators_reader();
                assert_eq!(
                    format!("{:?}", reader.read().expect("offset op")),
                    "I32Const { value: 0 }",
                    "offset must be i32.const 0"
                );
                let wasmparser::ElementItems::Functions(items) = &e.items else {
                    panic!("must be a function-index list")
                };
                let got: Vec<u32> = items.clone().into_iter().map(|i| i.expect("idx")).collect();
                // Expected identity mapping (fn doc / design §2.3 canonical layout, restored).
                let nfuncs = man.nfuncs;
                let mut want = Vec::new();
                for j in 0..nfuncs {
                    want.push(n_func_imports + j);
                }
                want.extend_from_slice(&[0, 1, 2]);
                for k in 0..man.host_slots.len() as u32 {
                    want.push(3 + k);
                }
                assert_eq!(
                    got, want,
                    "element function-index list must be the exact identity mapping"
                );
            }
            Payload::CodeSectionStart { count, .. } => {
                sections.push("code");
                assert_eq!(count, 3, "one body per function");
            }
            Payload::CodeSectionEntry(_) => {}
            Payload::StartSection { .. } => panic!("no start section, ever (§2.1)"),
            other => panic!("unexpected section in A-2 artifact: {other:?}"),
        }
    }
    assert_eq!(
        sections,
        ["type", "import", "function", "table", "export", "element", "code"]
    );
    let types = func_types(&artifact.bytes);
    for &ti in &func_type_indices {
        assert_eq!(
            types[ti as usize],
            (
                vec![
                    wasmparser::ValType::I32,
                    wasmparser::ValType::I32,
                    wasmparser::ValType::I64
                ],
                vec![wasmparser::ValType::I64]
            ),
            "every function is CompiledFn-typed"
        );
    }
    Validator::new()
        .validate_all(&artifact.bytes)
        .expect("artifact must validate");
    validate_aot_artifact(&artifact.bytes).expect("compile_module's own output must satisfy the whitelist");

    // -- manifest --
    assert_eq!(man.nfuncs, 3);
    assert_eq!(man.entry_slot, 1, "identity mapping: entry slot = m.entry");
    assert_eq!(man.helper_slots, [3, 4, 5], "[nfuncs, nfuncs+1, nfuncs+2]");
    assert_eq!(
        man.host_slots,
        vec![6],
        "§2.3 canonical formula: nfuncs+3+k, restored since A-3"
    );
    assert_eq!(order, vec![common::h_add7 as otter::HostFn1 as usize]);
    assert_eq!(
        man.table_size, 7,
        "canonical T = nfuncs + 3 + nhosts (unchanged formula; ALSO the actual declared size since A-3)"
    );
    assert_eq!(
        n_func_imports, 4,
        "3 helpers + 1 host (no provisional trampolines since A-3)"
    );
    // Scratch: ascending, 8-aligned, disjoint, exact-size; region = [base, base + Σ 8*words).
    assert_eq!(man.scratch.len(), 3);
    assert_eq!(man.scratch[0].addr, SCRATCH_BASE);
    assert_eq!(man.scratch[0].words, 0, "f0 is call-free");
    assert_eq!(
        man.scratch[1].addr, SCRATCH_BASE,
        "zero-span runs do not advance the cursor"
    );
    assert_eq!(man.scratch[1].words, 2);
    assert_eq!(man.scratch[2].addr, SCRATCH_BASE + 16);
    assert_eq!(man.scratch[2].words, 0);
    assert_eq!(man.scratch_region, (SCRATCH_BASE, SCRATCH_BASE + 16));
    for e in &man.scratch {
        assert!(e.addr.is_multiple_of(8), "every run is 8-aligned");
    }
    assert!(!man.shared_memory);
    assert_eq!(man.emitter_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(man.imports.len(), 1 + 3 + 1, "memory + 3 helpers + 1 host");
    assert_eq!(
        (
            man.imports[0].module.as_str(),
            man.imports[0].name.as_str(),
            man.imports[0].kind
        ),
        ("otter", "memory", AotImportKind::Memory)
    );
    match man.imports[4].kind {
        AotImportKind::Host { arity: 1, ctx: false } => {}
        other => panic!("h_add7 (Host1) must label arity=1,ctx=false, got {other:?}"),
    }

    // -- M3-shape regression (O-17 §5.2 v1.1) — shared-memory variant changes ONLY the memory import --
    let resolve = |p: usize| host_name(p).map(|n| HostImportName(n.to_string()));
    let shared = compile_module(
        &isa,
        &m,
        &resolve,
        &AotOpts::new(SCRATCH_BASE).with_shared_memory(true),
    )
    .expect("shared variant compiles");
    assert!(shared.manifest.shared_memory);
    Validator::new()
        .validate_all(&shared.bytes)
        .expect("shared artifact must validate");
    for payload in Parser::new(0).parse_all(&shared.bytes) {
        if let Payload::ImportSection(r) = payload.expect("parse") {
            let imports: Vec<_> = r.into_imports().map(|i| i.expect("import")).collect();
            match imports[0].ty {
                TypeRef::Memory(mem) => {
                    assert!(mem.shared, "shared flag set");
                    assert_eq!(mem.maximum, Some(65_536), "max = full wasm32 space (O-17 shape)");
                }
                ref other => panic!("otter.memory must be a memory import, got {other:?}"),
            }
        }
    }
    assert_eq!(
        bodies_with_ty_imm_offsets(&shared.bytes),
        bodies_with_ty_imm_offsets(&artifact.bytes),
        "sharedness changes the memory import's declared type ONLY — never the lowering"
    );
    assert_eq!(
        table_and_element_bytes(&shared.bytes),
        table_and_element_bytes(&artifact.bytes),
        "sharedness must not touch the table/element sections either"
    );
}

// --- entry-invariance (kept as a standalone regression — §2.4) -----------------------------------

#[test]
fn entry_choice_affects_only_the_export_index_and_manifest_entry_slot() {
    let isa = common::isa();
    let m = IrModule {
        funcs: vec![probe_fn(), fact_fn(), probe_fn()],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifacts, _) = compile_entry_variants(&isa, &m);
    assert_eq!(artifacts.len(), 3);
}

// --- design §8.3 mutation ③/⑤/⑥ + validator: mutation and adversarial-fixture coverage -------------

/// Locate the ONE `[flags=0x00, i32.const, <offset LEB byte>, end]` pattern inside the element
/// section's content bytes and return the absolute byte position of `<offset LEB byte>` —
/// `compile_module`'s OWN, self-owned encoding choice (MVP active segment, offset `i32.const 0`,
/// no explicit table index — §2.3), so this is a WHITE-BOX mutation helper, not a general-purpose
/// wasm editor: it asserts its one assumption (found exactly once) rather than silently
/// mis-locating a different byte on a future encoder-version drift.
fn element_offset_byte_pos(bytes: &[u8]) -> usize {
    let mut range = None;
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::ElementSection(r) = payload.expect("parse") {
            let rg = r.range();
            range = Some((rg.start as usize, rg.end as usize));
        }
    }
    let (start, end) = range.expect("artifact must have an element section (§2.3)");
    let needle = [0x00u8, 0x41, 0x00, 0x0B]; // flags(active,table0,funcs) i32.const 0 end
    let mut hits = Vec::new();
    for i in start..end.saturating_sub(needle.len() - 1) {
        if bytes[i..i + needle.len()] == needle {
            hits.push(i);
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one MVP-encoded `offset i32.const 0` element header"
    );
    hits[0] + 2 // the LEB value byte, right after [flags, i32.const-opcode]
}

/// Mutation ③ (design §8.3): flip the active element's offset from 0 to 1 in a real artifact's
/// bytes and confirm the identity mapping breaks — "differential goes red" is satisfied by EITHER
/// observable outcome, both handled (whichever the engine takes; MEASURED on wasmtime 45, not
/// merely theorized — see the `Instance::new` match arm's comment): module VALIDATION alone does
/// NOT reject the shift (the declared table's `min`/`max` are compile-time constants, but wasm
/// does not statically bounds-check a constant-offset active element against them), so
/// `Module::new` succeeds; INSTANTIATION then traps initializing the table ("undefined element:
/// out of bounds table access" — the shifted `offset + count` exceeds `table.min`), which is the
/// outcome this specific mutation actually produces. The OTHER arm (instantiation succeeds but
/// slot 0 answers with a DIFFERENT function) is kept as a documented fallback for a differently
/// shaped mutation/engine where the trap does not fire, so the test does not silently pass
/// vacuously if that ever changes.
#[test]
fn mutation_element_offset_shift_breaks_identity_mapping() {
    let isa = common::isa();
    // Two functions with OBSERVABLY DIFFERENT behavior at slot 0 vs slot 1, so a shift is
    // detectable even if instantiation somehow still succeeds.
    let f0 = func(
        0,
        1,
        vec![Inst::new(MOV_IMM, 0, 111, 0), Inst::new(HALT, 0, 0, 0)],
    );
    let f1 = func(
        0,
        1,
        vec![Inst::new(MOV_IMM, 0, 222, 0), Inst::new(HALT, 0, 0, 0)],
    );
    let m = IrModule {
        funcs: vec![f0, f1],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
    // Baseline: un-mutated artifact answers slot 0 with 111 (its own identity mapping is intact).
    let mut baseline = Nin1::new(&artifact, &order);
    assert_eq!(baseline.call(0, &[], 0), 111);

    let pos = element_offset_byte_pos(&artifact.bytes);
    assert_eq!(artifact.bytes[pos], 0x00);
    let mut mutant = artifact.bytes.clone();
    mutant[pos] = 0x01;

    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config).expect("engine");
    match Module::new(&engine, &mutant) {
        Err(_) => {} // rejected at validate/compile time — a valid "red" outcome
        Ok(module) => {
            let mut store = Store::new(
                &engine,
                HarnessSt {
                    table: Cell::new(None),
                    memory: Cell::new(None),
                    staging_top: Cell::new(STAGING_BASE),
                },
            );
            let memory = Memory::new(&mut store, MemoryType::new(MEMORY_PAGES, None)).expect("memory");
            store.data().memory.set(Some(memory));
            // This fixture's `m` is call-free with no host fns (fn doc, f0/f1 above) — same extern
            // list `Nin1::new` would build (`build_externs`, reused here since it is a hand-mutated
            // BYTES fixture, not a fresh `compile_module` artifact `Nin1::new` could wrap directly).
            let externs = build_externs(&mut store, &artifact.manifest, memory, &order);
            match Instance::new(&mut store, &module, &externs) {
                // Empirically the outcome this shape actually takes (measured, not merely
                // theorized): the shifted segment's `offset + count > table.min` and wasmtime
                // traps at INSTANTIATION time initializing the table ("undefined element: out
                // of bounds table access") — module VALIDATION alone does not reject it (the
                // `Ok(module)` arm above is always taken in practice for this mutation).
                Err(_) => {} // trapped during element initialization — a valid "red" outcome
                Ok(instance) => {
                    let table = instance
                        .get_table(&mut store, "otter_table")
                        .expect("otter_table export");
                    store.data().table.set(Some(table));
                    let f = table.get(&mut store, 0).expect("slot 0");
                    let Ref::Func(Some(f)) = f else {
                        panic!("mutant slot 0 must still be populated to compare")
                    };
                    let tf: TypedFunc<(i32, i32, i64), i64> = f.typed(&store).expect("CompiledFn wasm type");
                    let addr = STAGING_BASE;
                    let got = tf
                        .call(&mut store, (CTX_JUNK, addr as i32, 0))
                        .expect("mutant call must not trap for this to be a meaningful comparison");
                    assert_ne!(
                        got, 111,
                        "identity mapping must be broken by the offset shift — slot 0 must NOT \
                         answer with f0's value anymore"
                    );
                }
            }
        }
    }
}

/// Locate the byte offset of an `i32.const <value>` LEB immediate that sits IMMEDIATELY before a
/// `call_indirect` in function body `j`'s bytes — a Bin/Un/Host* helper call-site's slot literal
/// (the [`emit_call_indirect`]-shaped sites `lower_expr` emits IDENTICALLY regardless of
/// [`CallMode`], design §3.4/§5.1). Same white-box discipline as [`element_offset_byte_pos`]
/// (asserts the ONE occurrence sought is found, rather than silently mis-locating a different byte
/// on encoder drift) — used as [`mutation_table_mode_noncall_lowering_turns_r1_and_r2_red`]'s
/// single-byte mutation point.
fn helper_slot_imm_byte_pos(bytes: &[u8], j: usize, value: i32) -> usize {
    let mut cur = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("parse") {
            if cur == j {
                let mut reader = body.get_operators_reader().expect("operators");
                let mut prev_imm: Option<(usize, i32)> = None;
                while !reader.eof() {
                    let (op, pos) = reader.read_with_offset().expect("operator");
                    match op {
                        Operator::I32Const { value: v } => {
                            // 0x41 opcode byte, then the LEB immediate — an ABSOLUTE index into
                            // `bytes` (unlike `bodies_with_ty_imm_offsets`'s `pos - start`, which
                            // indexes into a per-body SLICE starting at 0 — this fn indexes the
                            // full `bytes` directly, so no `start` subtraction here).
                            prev_imm = Some((pos as usize + 1, v));
                        }
                        Operator::CallIndirect { .. } => {
                            if let Some((bpos, v)) = prev_imm
                                && v == value
                            {
                                return bpos;
                            }
                            prev_imm = None;
                        }
                        _ => prev_imm = None,
                    }
                }
            }
            cur += 1;
        }
    }
    panic!("func {j}: no `i32.const {value}` immediately before a call_indirect");
}

/// Mutation ⑤ (design §8.3 — the R3 evidence §9 A-3's record==live row calls for): flip a Bin
/// site's helper-slot `i32.const` immediate from `binop_slot` to `convert_slot` in a REAL
/// `compile_module` (`CallMode::Table`) artifact's raw bytes — a mutation of NON-CALL lowering
/// (`lower_expr`'s `Bin` arm, mode-independent — §3.4/§5.1) — and confirm BOTH R1 (call-free
/// function) and R2 (a call-carrying function, where the mutated Bin site sits OUTSIDE the
/// `Call`'s own window) go RED. Not merely "the artifact differs somewhere": since the mutated
/// byte is OUTSIDE any type-index site (R1) and outside any call window (R2), this specifically
/// exercises the record==live regime's non-window/non-type-index comparison path — the DEFAULT
/// "everything else must match exactly" rule §5.1's review checklist item ③ protects.
#[test]
fn mutation_table_mode_noncall_lowering_turns_r1_and_r2_red() {
    let isa = common::isa();
    let add64 = bin_opcode(3, 0); // I64 Add — routes through the SAME `otter_binop` helper as every Bin op

    // -- R1 half: a call-free function using ONE Bin site. --
    let bin_fn = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 3,
        code: vec![Inst::new(add64, 2, 0, 1), Inst::new(HALT, 2, 0, 0)],
    };
    let m_r1 = IrModule {
        funcs: vec![bin_fn, probe_fn()],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifact_r1, order_r1) = compile_with_log(&isa, &m_r1, &AotOpts::new(SCRATCH_BASE));
    // Baseline: the UN-mutated artifact passes R1 — the MUTATION, not the gate itself, is under
    // test (mirrors the discipline every other mutation test in this suite follows).
    assert_record_equals_live(&isa, &m_r1, &artifact_r1, &order_r1);

    let binop_slot = artifact_r1.manifest.helper_slots[0] as i32;
    let convert_slot = artifact_r1.manifest.helper_slots[1] as i32;
    let pos = helper_slot_imm_byte_pos(&artifact_r1.bytes, 0, binop_slot);
    assert_eq!(artifact_r1.bytes[pos], binop_slot as u8);
    let mut mutant_bytes = artifact_r1.bytes.clone();
    mutant_bytes[pos] = convert_slot as u8; // the Bin site now calls otter_convert, not otter_binop
    let mutant_r1 = AotArtifact {
        bytes: mutant_bytes,
        manifest: artifact_r1.manifest.clone(),
    };
    let red = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_record_equals_live(&isa, &m_r1, &mutant_r1, &order_r1)
    }));
    assert!(
        red.is_err(),
        "R1 must go RED when a Table-mode non-call (Bin) helper-slot immediate is mutated"
    );

    // -- R2 half: the SAME mutation, on a function that ALSO has a Call site AFTER the Bin site —
    // the mutated byte sits OUTSIDE the Call's own window. --
    let bin_and_call_fn = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(add64, 2, 0, 1),   // Bin site (mutation target) — locals 0,1 -> 2
            Inst::new(MOV_IMM, 3, 1, 0), // callee = probe (function index 1)
            Inst::new(CALL, 4, 3, 0),    // Call site (unaffected by the mutation), argc=0
            Inst::new(RET, 2, 0, 0),
        ],
    };
    let m_r2 = IrModule {
        funcs: vec![bin_and_call_fn, probe_fn()],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let (artifact_r2, order_r2) = compile_with_log(&isa, &m_r2, &AotOpts::new(SCRATCH_BASE));
    assert_record_equals_live_windowed(&isa, &m_r2, &artifact_r2, &order_r2);
    let binop_slot2 = artifact_r2.manifest.helper_slots[0] as i32;
    let convert_slot2 = artifact_r2.manifest.helper_slots[1] as i32;
    let pos2 = helper_slot_imm_byte_pos(&artifact_r2.bytes, 0, binop_slot2);
    assert_eq!(artifact_r2.bytes[pos2], binop_slot2 as u8);
    let mut mutant_bytes2 = artifact_r2.bytes.clone();
    mutant_bytes2[pos2] = convert_slot2 as u8;
    let mutant_r2 = AotArtifact {
        bytes: mutant_bytes2,
        manifest: artifact_r2.manifest.clone(),
    };
    let red2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_record_equals_live_windowed(&isa, &m_r2, &mutant_r2, &order_r2)
    }));
    assert!(
        red2.is_err(),
        "R2 must go RED when a Table-mode non-call (Bin) helper-slot immediate is mutated, even \
         though the mutation sits OUTSIDE any Call/TailCall window"
    );
}

/// A hand-built adversarial fixture ([스파이크] M5's demonstrated shape): imports a table AND
/// declares its own, with an active element segment EXPLICITLY targeting the IMPORTED table
/// (table index 0 in the shared table-index space, since imports precede declared entities) —
/// legal core wasm, and exactly the hazard §2.1/§2.3 require `validate_aot_artifact` to reject.
fn module_with_element_targeting_imported_table() -> Vec<u8> {
    let mut types = TypeSection::new();
    types.ty().function(
        [WasmValType::I32, WasmValType::I32, WasmValType::I64],
        [WasmValType::I64],
    );

    let mut imports = ImportSection::new();
    imports.import(
        "otter",
        "table",
        EntityType::Table(WasmTableType {
            element_type: wasm_encoder::RefType::FUNCREF,
            table64: false,
            minimum: 2,
            maximum: None,
            shared: false,
        }),
    );

    let mut functions = FunctionSection::new();
    functions.function(0);

    let mut tables = TableSection::new();
    tables.table(WasmTableType {
        element_type: wasm_encoder::RefType::FUNCREF,
        table64: false,
        minimum: 1,
        maximum: Some(1),
        shared: false,
    });

    let mut exports = ExportSection::new();
    exports.export("f", ExportKind::Func, 0);

    let mut elements = ElementSection::new();
    // table_index = Some(0) — the IMPORTED table (table index space: import(0) then declared(1)).
    let offset = ConstExpr::i32_const(0);
    elements.active(
        Some(0),
        &offset,
        Elements::Functions(std::borrow::Cow::Borrowed(&[0u32])),
    );

    let mut code = wasm_encoder::CodeSection::new();
    let mut body = wasm_encoder::Function::new_with_locals_types([]);
    body.instruction(&wasm_encoder::Instruction::I64Const(0));
    body.instruction(&wasm_encoder::Instruction::End);
    code.function(&body);

    let mut module = WasmModule::new();
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&tables);
    module.section(&exports);
    module.section(&elements);
    module.section(&code);
    module.finish()
}

/// Mutation ⑥ (design §8.3): the validator must reject an element segment targeting an imported
/// table, and must ACCEPT `compile_module`'s own (never-imports-a-table) output.
#[test]
fn validator_rejects_element_targeting_imported_table() {
    let bytes = module_with_element_targeting_imported_table();
    // Sanity: this fixture is legal CORE WASM (the hazard is real, not a malformed-input artifact
    // — [스파이크] M5's point).
    Validator::new()
        .validate_all(&bytes)
        .expect("the adversarial fixture must be valid core wasm on its own terms");
    assert_eq!(
        validate_aot_artifact(&bytes),
        Err(AotValidationError::ElementTargetsImportedTable { table_index: 0 })
    );
}

/// The validator's whitelist half: a hand-built module with a `start` section (id 8, outside the
/// §2.1 AOT pre-link whitelist) must be rejected.
#[test]
fn validator_rejects_a_disallowed_section() {
    let mut types = TypeSection::new();
    types.ty().function([], []);
    let mut functions = FunctionSection::new();
    functions.function(0);
    let mut code = wasm_encoder::CodeSection::new();
    let mut body = wasm_encoder::Function::new_with_locals_types([]);
    body.instruction(&wasm_encoder::Instruction::End);
    code.function(&body);
    let mut module = WasmModule::new();
    module.section(&types);
    module.section(&functions);
    module.section(&wasm_encoder::StartSection { function_index: 0 });
    module.section(&code);
    let bytes = module.finish();
    Validator::new()
        .validate_all(&bytes)
        .expect("fixture must be valid core wasm on its own terms");
    assert_eq!(
        validate_aot_artifact(&bytes),
        Err(AotValidationError::DisallowedSection { id: 8 })
    );
}

// --- gate 4: AotError regression (§8.4, unchanged from A-1) ---------------------------------------

fn resolve_all_named(p: usize) -> Option<HostImportName> {
    host_name(p).map(|n| HostImportName(n.to_string()))
}

fn compile_err(isa: &Isa, m: &IrModule, opts: &AotOpts) -> AotError {
    compile_module(isa, m, &resolve_all_named, opts).expect_err("must fail")
}

fn module_of(funcs: Vec<FuncDef>) -> IrModule {
    IrModule {
        funcs,
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    }
}

/// A function whose `check_func` fails (empty code).
fn malformed_fn() -> FuncDef {
    FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 1,
        code: vec![],
    }
}

/// A function with an integer `Div` at pc 0 (§4.6-3).
fn integer_div_fn() -> FuncDef {
    func(
        2,
        3,
        vec![Inst::new(bin_opcode(3, 3), 2, 0, 1), Inst::new(HALT, 2, 0, 0)],
    )
}

/// `ScratchBaseMisaligned` is the FIRST check — it pre-empts even a module full of refusable
/// functions (§4.2 "input-contract violation before the function pass").
#[test]
fn scratch_base_misaligned_precedes_the_function_pass() {
    let isa = common::isa();
    let m = module_of(vec![malformed_fn()]);
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(4)),
        AotError::ScratchBaseMisaligned { base: 4 }
    );
}

/// Single-pass determinism (§4.2): the FIRST failing function in func_idx order wins, regardless
/// of the refusal kinds involved — and within one function, `check_func` precedes the §4.6 walk.
#[test]
fn first_failing_function_wins_and_check_func_precedes_the_walk() {
    let isa = common::isa();
    // f0 fine; f1 IntegerDiv; f2 malformed → the pass stops at f1.
    let m = module_of(vec![probe_fn(), integer_div_fn(), malformed_fn()]);
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(SCRATCH_BASE)),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::IntegerDiv { pc: 0 }
        }
    );
    // Swapped: f1 malformed, f2 IntegerDiv → ValidationFailed at f1.
    let m = module_of(vec![probe_fn(), malformed_fn(), integer_div_fn()]);
    match compile_err(&isa, &m, &AotOpts::new(SCRATCH_BASE)) {
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::ValidationFailed(_),
        } => {}
        other => panic!("expected Func{{1, ValidationFailed}}, got {other:?}"),
    }
    // Within ONE function: check_func (bad jump target) pre-empts the walk's IntegerDiv.
    let both = func(
        2,
        3,
        vec![
            Inst::new(bin_opcode(3, 3), 2, 0, 1), // integer Div (a walk refusal)…
            Inst::new(JUMP_IF, 0, 99, 0),         // …but the out-of-range target fails check_func first
            Inst::new(HALT, 2, 0, 0),
        ],
    );
    let m = module_of(vec![probe_fn(), both]);
    match compile_err(&isa, &m, &AotOpts::new(SCRATCH_BASE)) {
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::ValidationFailed(_),
        } => {}
        other => panic!("expected Func{{1, ValidationFailed}}, got {other:?}"),
    }
}

/// Every §4.6/§8.4 refusal shape promotes value-intact with the exact `func_idx`.
#[test]
fn each_refusal_shape_promotes_with_its_func_idx() {
    let isa = common::isa();
    let base = AotOpts::new(SCRATCH_BASE);

    // HostFnUnresolved: a resolver that knows nothing.
    let unresolved = |_: usize| -> Option<HostImportName> { None };
    let m = module_of(vec![
        probe_fn(),
        probe_fn(),
        func(1, 2, vec![Inst::new(HOST1, 1, 0, 0), Inst::new(HALT, 1, 0, 0)]),
    ]);
    assert_eq!(
        compile_module(&isa, &m, &unresolved, &base).expect_err("unresolved"),
        AotError::Func {
            func_idx: 2,
            refusal: Refusal::HostFnUnresolved { pc: 0 }
        }
    );

    // FallsOffEnd (§4.6-1).
    let m = module_of(vec![probe_fn(), func(0, 1, vec![Inst::new(MOV_IMM, 0, 7, 0)])]);
    assert_eq!(
        compile_err(&isa, &m, &base),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::FallsOffEnd { last_pc: 0 }
        }
    );

    // TrailingJumpIf (§4.6-2).
    let m = module_of(vec![probe_fn(), func(1, 2, vec![Inst::new(JUMP_IF, 0, 0, 0)])]);
    assert_eq!(
        compile_err(&isa, &m, &base),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::TrailingJumpIf { pc: 0 }
        }
    );

    // ControlEffectNotLast (§4.6-4) — needs an ISA carrying a control-not-last opcode.
    let mut bad_isa = common::isa();
    let bad_op = bad_isa.ops.len() as u16;
    bad_isa.ops.push(otter::OpDef {
        name: "ret_then_write",
        effects: vec![
            otter::Effect::Return {
                value: otter::Expr::LocalAt(0),
            },
            otter::Effect::SetLocalAt {
                reg: 0,
                value: otter::Expr::Imm(1),
            },
        ],
        hints: otter::OpHints {
            pure: false,
            terminator: true,
        },
    });
    let m = module_of(vec![probe_fn(), func(0, 1, vec![Inst::new(bad_op, 0, 0, 0)])]);
    assert_eq!(
        compile_err(&bad_isa, &m, &base),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::ControlEffectNotLast {
                pc: 0,
                effect: "Return"
            }
        }
    );

    // WasmLocalsLimitExceeded (§4.6-5): nlocals + 4 == 50_001.
    let over = MAX_WASM_FUNCTION_LOCALS - 3;
    let m = module_of(vec![
        probe_fn(),
        FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals: over,
            code: vec![Inst::new(HALT, 0, 0, 0)],
        },
    ]);
    assert_eq!(
        compile_err(&isa, &m, &base),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::WasmLocalsLimitExceeded {
                wasm_locals: over + 4,
                limit: MAX_WASM_FUNCTION_LOCALS
            }
        }
    );
}

/// `ScratchOutOfAddressSpace` under SELF-assignment (§2.5, panel-corrected condition
/// `base + Σspan > 2^32`): fires at the FIRST violating function with the ASSIGNED address; a run
/// ending exactly AT `2^32` stays legal (and yields the u64 `scratch_region` end the design's
/// (u32,u32) sketch could not represent); a later zero-fit function refuses with the saturated
/// diagnostic address.
#[test]
fn scratch_self_assignment_overflow_fires_at_the_first_violating_function() {
    let isa = common::isa();
    let call_fn = |argc: i32| -> FuncDef {
        let nlocals = (argc + 3) as usize;
        FuncDef {
            nparams: 0,
            consts: vec![],
            nlocals,
            code: vec![
                Inst::new(CALL, argc + 1, 0, argc),
                Inst::new(HALT, argc + 1, 0, 0),
            ],
        }
    };
    // base + 8*2 > 2^32 at f1 (f0 is call-free, span 0).
    let base = 0xFFFF_FFF8u64;
    let m = module_of(vec![probe_fn(), call_fn(2)]);
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(base)),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::ScratchOutOfAddressSpace {
                scratch_addr: base,
                words: 2
            }
        }
    );
    // Ending exactly at 2^32 is legal — and the manifest's u64 end says so exactly.
    let m = module_of(vec![probe_fn(), call_fn(1)]);
    let a = compile_module(&isa, &m, &resolve_all_named, &AotOpts::new(base)).expect("boundary is legal");
    assert_eq!(a.manifest.scratch[1].addr, base);
    assert_eq!(a.manifest.scratch_region, (base, 1u64 << 32));
    // Cumulative form: f1's 1000-word run + f2 ending exactly at 2^32 → Ok; adding f3 (words 1)
    // refuses at f3 with the EXACT cursor as the diagnostic address (the cursor sits AT 2^32 —
    // the pre-O-19 u32-saturated `u32::MAX` stand-in is gone, C1's u64 widening).
    let base = 0xFFFF_E0B8u64; // 2^32 - 8008, 8-aligned
    let m = module_of(vec![probe_fn(), call_fn(1000), call_fn(1)]);
    let a = compile_module(&isa, &m, &resolve_all_named, &AotOpts::new(base)).expect("exact fit");
    assert_eq!(a.manifest.scratch[2].addr, 0xFFFF_FFF8);
    assert_eq!(a.manifest.scratch_region, (base, 1u64 << 32));
    let m = module_of(vec![probe_fn(), call_fn(1000), call_fn(1), call_fn(1)]);
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(base)),
        AotError::Func {
            func_idx: 3,
            refusal: Refusal::ScratchOutOfAddressSpace {
                scratch_addr: 1u64 << 32,
                words: 1
            }
        }
    );
}

/// Module-level checks run AFTER the function pass (§4.2): a refusable function pre-empts
/// `EntryOutOfRange`; with every function fine, the entry check fires — the empty module
/// included.
#[test]
fn entry_out_of_range_is_checked_after_the_function_pass() {
    let isa = common::isa();
    let mut m = module_of(vec![probe_fn(), probe_fn()]);
    m.entry = 7;
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(SCRATCH_BASE)),
        AotError::EntryOutOfRange { entry: 7, nfuncs: 2 }
    );
    let mut m = module_of(vec![probe_fn(), integer_div_fn()]);
    m.entry = 7;
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(SCRATCH_BASE)),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::IntegerDiv { pc: 0 }
        },
        "a function failure pre-empts the module-level entry check"
    );
    let m = module_of(vec![]);
    assert_eq!(
        compile_err(&isa, &m, &AotOpts::new(SCRATCH_BASE)),
        AotError::EntryOutOfRange { entry: 0, nfuncs: 0 }
    );
}

/// Two DIFFERENT host-fn pointers resolving to the SAME name is refused at the dup's own site,
/// with both function attributions (§4.2) — same-function dups included.
#[test]
fn duplicate_host_import_name_attributes_both_sites() {
    let isa = common::isa();
    let dup_resolver = |_: usize| -> Option<HostImportName> { Some(HostImportName("dup".to_string())) };
    let host1 = func(1, 2, vec![Inst::new(HOST1, 1, 0, 0), Inst::new(HALT, 1, 0, 0)]);
    let host2 = func(2, 3, vec![Inst::new(HOST2, 2, 0, 1), Inst::new(HALT, 2, 0, 0)]);
    // First resolution in f0, dup in f2.
    let m = module_of(vec![host1.clone(), probe_fn(), host2.clone()]);
    assert_eq!(
        compile_module(&isa, &m, &dup_resolver, &AotOpts::new(SCRATCH_BASE)).expect_err("dup"),
        AotError::DuplicateHostImportName {
            name: HostImportName("dup".to_string()),
            first_func_idx: 0,
            dup_func_idx: 2
        }
    );
    // Both in ONE function.
    let both = func(
        2,
        4,
        vec![
            Inst::new(HOST1, 2, 0, 0),
            Inst::new(HOST2, 3, 2, 1),
            Inst::new(HALT, 3, 0, 0),
        ],
    );
    let m = module_of(vec![probe_fn(), both]);
    assert_eq!(
        compile_module(&isa, &m, &dup_resolver, &AotOpts::new(SCRATCH_BASE)).expect_err("dup"),
        AotError::DuplicateHostImportName {
            name: HostImportName("dup".to_string()),
            first_func_idx: 1,
            dup_func_idx: 1
        }
    );
    // …and two pointers with DISTINCT names coexist fine (the check is name-keyed, not ptr-keyed).
    let m = module_of(vec![host1, host2]);
    compile_module(&isa, &m, &resolve_all_named, &AotOpts::new(SCRATCH_BASE)).expect("distinct names");
}

/// `TooManyLeaders` cannot be constructed from a real input (≥ 2³¹ instructions — O-13's own
/// record), so per §8.4 its promotion is pinned VALUE-level: `AotError::Func` embeds any
/// `Refusal` value verbatim (no re-encoding between the two surfaces).
#[test]
fn too_many_leaders_promotion_is_value_level() {
    let refusal = Refusal::TooManyLeaders { nleaders: usize::MAX };
    let promoted = AotError::Func { func_idx: 7, refusal };
    match promoted {
        AotError::Func {
            func_idx: 7,
            refusal: Refusal::TooManyLeaders { nleaders },
        } => assert_eq!(nleaders, usize::MAX, "value-intact"),
        other => panic!("promotion must be value-intact, got {other:?}"),
    }
}

// ═══════════════════════ O-14 A-4: gate hardening (design §9 A-4 row) ═══════════════════════════
//
// This section is A-4's whole deliverable (design §9: "record==live R1/R2/R3 CI 승격 + AOT⊂JIT
// 3-방식 차등 배선(§6 — 리졸버 쌍 고정) + 뮤테이션 실행·기록"): no production-code change, only
// tests + the mutation ledger below.
//
// **CI promotion status (the gate registry)**: everything here and above already runs under
// `cargo test --workspace` — the exact command CI's `check` job `Test` step and `just test`/
// `just ci` execute — so R1/R2/R3 and the 3-way differential are CI-promoted by INCLUSION, not by
// new wiring (A-4 judgment: adding a dedicated CI lane would duplicate the workspace run —
// 과잉 배선 금지). The registry comments in `justfile` (`test` recipe) and `.github/workflows/
// ci.yml` (`Test` step) name this file as a load-bearing gate suite; the
// [`a4_gate_registry_tests_exist_and_none_are_ignored`] meta-test below is the one
// anti-silent-skip guarantee (its own doc states what it does and does not catch).
//
// **§6 comparison universe** (scope, verbatim from the design): `push_func` is EXCLUDED — an
// A-mode artifact structurally has no pushed segment (§10-8) and the JIT `in_range` index space
// (base + pushed) differs from the AOT guard boundary (`nfuncs`), so no test below models it.
// The nested-`Halt` carve-out (§6/§8(a)) is honored by corpus discipline: every explicit fixture
// keeps `Halt` in the top frame only, and the fuzz generators are call-free (any `Halt` they
// emit IS top-frame for whichever function is invoked).
//
// ─────────────────────────── A-4 MUTATION LEDGER (design §8.3 ①②④⑦) ───────────────────────────
//
// §9 A-4's "실증 기록": each mutation below was applied to `src/lib.rs` as a TEMPORARY source
// edit, the suite was run (`cargo test -p otter-wasmemit --test aot`), the red result recorded
// here verbatim-in-gist, and the edit reverted (post-revert: full suite green, `git diff` clean
// of src/). Mutations ③⑤⑥ need no ledger entry: since A-3 they are PERMANENT tests in this file
// (`mutation_element_offset_shift_breaks_identity_mapping`,
// `mutation_table_mode_noncall_lowering_turns_r1_and_r2_red`,
// `validator_rejects_element_targeting_imported_table`) — byte-level mutants rebuilt on every
// run, strictly stronger than a one-off source-mutation record.
//
// To REPRODUCE any entry: apply the quoted edit to the named function in
// `crates/otter-wasmemit/src/lib.rs`, run `cargo test -p otter-wasmemit --test aot`, observe the
// listed tests fail with the listed shapes, then `git checkout -- crates/otter-wasmemit/src/lib.rs`.
//
// ── Mutation ① — guard removal (§8.3-①: "가드 제거 → OOB 케이스가 trap화") ──
//   Edit: in `emit_table_call` AND `emit_table_tailcall`, delete the four guard instructions
//         (`local.get l(base); i64.const NFUNCS; i64.ge_u; if …`) plus the fill arm/`else`/`end`
//         bracketing, leaving marshal + direct `call_indirect`/`return_call_indirect`
//         (prepend `let _ = (nfuncs, fill);` to keep the now-unused params compiling).
//   Red (5 tests, 2026-09-04, `cargo test -p otter-wasmemit --test aot`):
//     - fail_closed_four_shapes_call_and_tailcall_yield_fill_never_trap — "registered function
//       must not trap: … wasm trap: undefined element: out of bounds table access" (the FIRST
//       shape, Oob/Call, traps: the OOB word wraps to a huge table index — EXACTLY the §8.3-①
//       advertised "OOB 케이스가 trap화" red).
//     - three_way_fail_closed_garbage_callee_jit_and_aot_agree_on_fill — same trap on the AOT
//       leg (the JIT-shape leg still fails closed via the engine loop-head check, so the modes
//       DIVERGE — §6's exact violation).
//     - record_equals_live_r2_windowed_on_call_carrying_modules — "func 0 site …: Dispatch-mode
//       window shape mismatch (§3.1/§3.2/§4.3)" (the guard's removal shifts the window walk's
//       divergence point, so R2's literal-shape pin misfires loudly).
//     - mutation_table_mode_noncall_lowering_turns_r1_and_r2_red — its GREEN baseline half
//       ("byte 81 differs OUTSIDE a type-index site") fails before the mutation half runs.
//     - call_uses_if_result_i64_tailcall_uses_plain_if_and_both_validate — the `if (result
//       i64)` shape witness no longer finds its block type.
//
// ── Mutation ② — `i64.ge_u` → `i64.gt_s` (§8.3-②: "음수-콜리 케이스 적색") ──
//   Edit: in `emit_table_call` AND `emit_table_tailcall`, replace `Instruction::I64GeU` with
//         `Instruction::I64GtS`.
//   Red (4 tests, 2026-09-04): the Oob shape (99) STAYS green under `gt_s` (99 >_s nfuncs still
//   guards) — the first red is the plain-NEGATIVE callee (`-1 >_s nfuncs` is false → the call arm
//   wraps -1 to table index 0xFFFF_FFFF): §8.3-②'s advertised "음수-콜리 케이스 적색", verbatim.
//     - fail_closed_four_shapes_call_and_tailcall_yield_fill_never_trap — "wasm trap: undefined
//       element: out of bounds table access" at Negative/Call (Oob/Call and Oob/TailCall passed
//       first, per loop order; FullWideNeg is the same class).
//     - three_way_fail_closed_garbage_callee_jit_and_aot_agree_on_fill — same, on the AOT leg.
//     - record_equals_live_r2_windowed_on_call_carrying_modules — "Table-mode window shape
//       mismatch (§3.1/§3.2)" (the window literal pins `I64GeU` itself).
//     - mutation_table_mode_noncall_lowering_turns_r1_and_r2_red — baseline half, as in ①.
//
// ── Mutation ④ — scratch-overlap injection (§8.3-④: "배정 로직 변조") ──
//   Edit: in `compile_module`'s single-pass loop, neutralize the cursor advance
//         (`cursor += 8 * words as u64;` → `cursor += 0 * words as u64;`) — every function is
//         then assigned the SAME `scratch_base` region (total overlap), manifest kept in sync
//         with what was actually baked (the faithful "assignment logic" mutation).
//   Red (3 tests, 2026-09-04):
//     - three_way_explicit_call_carrying_corpus_is_bitwise_identical — "func 1: §2.5 sequential
//       disjoint scratch assignment violated: left 65536, right 65544" (the A-4
//       `assert_scratch_runs_disjoint` pin inside the 3-way pipeline).
//     - nin1_structure_and_manifest_invariants — scratch[2].addr 65536 != 65552.
//     - scratch_self_assignment_overflow_fires_at_the_first_violating_function —
//       scratch_region end (4294967288, 4294967288) != (4294967288, 4294967296).
//   HONEST FINDING (design §8.3-④ divergence, recorded per the A-4 discipline): the advertised
//   red channel — "재귀/중첩 마셜링 차등 적색" — did NOT fire. Every pure EXECUTION differential
//   stayed green under TOTAL scratch overlap (`nin1_call_carrying_corpus…`,
//   `mixed_call_then_tailcall_tree…`, the 2M-deep gate, and the 3-way runs up to their geometry
//   pin), because under today's lowering every marshal store is consumed by the callee prologue's
//   argc-guarded loads immediately at activation entry, with only the guard + call between store
//   and load — the exact "shared-max would also be safe today" concession §2.5 itself records
//   ("공유안도 안전 논증이 성립하지만 [추정]"). The differential alone therefore CANNOT bite this
//   mutation in principle today; §2.5's load-bearing invariant ("마셜링 store와 소비 호출 사이에
//   otter-수준 재진입 가능 호출 없음", §5.1 checklist ③) is what keeps that true, and the
//   GEOMETRY pins above (incl. the A-4 one added for exactly this reason) are ④'s enforcing
//   gate. Not an implementation defect — the implementation's disjoint assignment is correct;
//   the design's advertised red channel for ④ was optimistic about what execution can observe.
//
// ── Mutation ⑦ — shared-precheck bypass (§8.3-⑦: "check_func만 수행 → AOT-수용∧JIT-거부") ──
//   Edit: in `emit_function_body`, make the §4.6 walk conditional on the mode —
//         `precheck(isa, f, resolve)?;` → `if matches!(call_mode, CallMode::Dispatch)
//         { precheck(isa, f, resolve)?; }` — so `compile_module` (Table mode) runs `check_func`
//         only while `compile_dispatch_fn_with` keeps the full predicate: the exact "AOT accepts
//         what JIT refuses" divergence §6's ⊂-direction forbids.
//   Red (4 tests, 2026-09-04):
//     - subset_refusal_pairs_aot_and_jit_refuse_identically — "§6 ⊂-direction: this fixture must
//       be AOT-refused: AotArtifact { … }" at the IntegerDiv fixture (`check_func` alone passes
//       it; the emitted body even bakes the integer-Div helper call) — the LITERAL "AOT-수용 ∧
//       JIT-거부" §8.3-⑦ names, proving the ⊂-direction gate non-vacuous in both halves
//       (`JitShape::new`'s ⊂-violation panic is the accepting-side twin of this same claim).
//     - each_refusal_shape_promotes_with_its_func_idx — "must fail: AotArtifact { … }"
//       (FallsOffEnd et al. now compile on the AOT surface).
//     - first_failing_function_wins_and_check_func_precedes_the_walk — the §4.2 deterministic
//       first-failure order collapses (f1's IntegerDiv no longer fires; f2's ValidationFailed
//       wins instead).
//     - entry_out_of_range_is_checked_after_the_function_pass — "a function failure pre-empts
//       the module-level entry check" fails (the function pass no longer fails).
//
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// The §6 resolver PAIR, fixed by construction: BOTH resolvers are total on exactly the pointers
/// the ONE registration table [`host_name`] knows — the AOT resolver returns the registered NAME,
/// the JIT resolver a slot that is a pure function of that same name — so "AOT 리졸버 도메인 ⊆
/// JIT 리졸버 도메인" (here: equality) is not asserted but CONSTRUCTED (§6: "단일 호스트-등록
/// 테이블에서 … 쌍으로 파생"). The slot values are this harness's own mock-table layout
/// (`differential.rs`/`calls.rs` convention: sparse, non-contiguous, disjoint from the helper
/// slots — so a lowering bug conflating host and helper slots cannot pass by coincidence).
fn jit_resolve_paired(p: usize) -> Option<u32> {
    host_name(p).map(|n| match n {
        "h_add7" => JW_H_ADD7_SLOT,
        "h_mix" => JW_H_MIX_SLOT,
        "hc_ret_ctx" => JW_HC_RET_CTX_SLOT,
        "hc_mix" => JW_HC_MIX_SLOT,
        other => panic!("unregistered host name {other}"),
    })
}

// The JIT-shape mock-table layout (calls.rs/differential.rs convention).
const JW_BINOP_SLOT: u32 = 3;
const JW_CONVERT_SLOT: u32 = 5;
const JW_UNOP_SLOT: u32 = 7;
const JW_DISPATCH_SLOT: u32 = 11;
const JW_TAIL_SLOT: u32 = 12;
const JW_H_ADD7_SLOT: u32 = 13;
const JW_H_MIX_SLOT: u32 = 17;
const JW_HC_RET_CTX_SLOT: u32 = 19;
const JW_HC_MIX_SLOT: u32 = 23;
const JW_TABLE_MIN: u64 = 32;

fn jit_helper_idx() -> HelperIdx {
    HelperIdx {
        dispatch: JW_DISPATCH_SLOT,
        tail: JW_TAIL_SLOT,
        binop: JW_BINOP_SLOT,
        convert: JW_CONVERT_SLOT,
        unop: JW_UNOP_SLOT,
    }
}

/// The `CompiledFn` wasm signature `(ctx, args, argc) -> r`, as a wasmtime typed handle.
type CompiledTf = TypedFunc<(i32, i32, i64), i64>;

/// Store data for the JIT-shape leg — `calls.rs::St`'s shape (the "기존 mock-하네스 경로" §6-(ii)
/// names), minus its `env`/T0 arm: in the §6 differential EVERY function runs as a
/// `compile_dispatch_fn_with` product (that is what "JIT-형" means there; the wasm↔T0 tier-MIX
/// matrix stays `calls.rs`'s own, separate coverage).
struct JitSt {
    nfuncs: usize,
    fill: Value,
    compiled: Vec<Option<CompiledTf>>,
    memory: Option<Memory>,
    // The engine's tail-signal triple (`tail_pending`/`tail_callee`/`tail_args`), mirrored
    // field-for-field like calls.rs.
    tail_pending: Cell<bool>,
    tail_callee: Cell<usize>,
    tail_args: RefCell<Vec<Value>>,
    staging_top: Cell<u32>,
}

/// `calls.rs::read_args`' twin: copy `argc` words at `ptr` out of the mock memory (copy-at-entry;
/// the `argc <= 0` arm mirrors the engine's defensive slice guards).
fn jw_read_args(caller: &mut Caller<'_, JitSt>, ptr: i32, argc: i64) -> Vec<Value> {
    if argc <= 0 {
        return Vec::new();
    }
    let mem = caller.data().memory.expect("memory installed");
    let mut buf = vec![0u8; argc as usize * 8];
    mem.read(&mut *caller, ptr as u32 as usize, &mut buf)
        .expect("args live in the mock memory");
    buf.chunks_exact(8)
        .map(|c| i64::from_le_bytes(c.try_into().expect("chunks_exact(8)")))
        .collect()
}

/// `calls.rs::call_wasm`'s twin: LIFO-stage `args`, re-arm fuel, call one JIT-shape activation.
fn jw_call_wasm(ctx: &mut StoreContextMut<'_, JitSt>, tf: CompiledTf, args: &[Value]) -> Value {
    let (mem, addr) = {
        let st = ctx.data();
        let top = st.staging_top.get();
        st.staging_top.set(top + (args.len() as u32) * 8);
        (st.memory.expect("memory installed"), top)
    };
    assert!(
        addr as usize + args.len() * 8 <= (MEMORY_PAGES as usize) * 65_536,
        "staging overflow — bump MEMORY_PAGES"
    );
    ctx.set_fuel(FUEL).expect("set_fuel");
    let mut bytes = Vec::with_capacity(args.len() * 8);
    for a in args {
        bytes.extend_from_slice(&a.to_le_bytes());
    }
    mem.write(&mut *ctx, addr as usize, &bytes)
        .expect("staging write");
    let r = tf
        .call(&mut *ctx, (CTX_JUNK, addr as i32, args.len() as i64))
        .expect("emitted f must not trap");
    ctx.data().staging_top.set(addr); // LIFO release
    r
}

/// `calls.rs::mini_dispatch`'s twin, all-wasm (see [`JitSt`]'s doc): the engine trampoline loop —
/// loop-head OOB→`fill` fail-closed check, wasm activation, consume-immediately tail signal —
/// with the T0 arm elided (every slot is compiled here by construction, asserted loudly).
fn jw_dispatch(ctx: &mut StoreContextMut<'_, JitSt>, mut idx: usize, args: &[Value]) -> Value {
    let mut owned: Vec<Value> = Vec::new();
    let mut cur: &[Value] = args;
    loop {
        // dispatch: loop-head fail-closed range check (poison half omitted, like calls.rs).
        if idx >= ctx.data().nfuncs {
            return ctx.data().fill;
        }
        let tf = ctx.data().compiled[idx]
            .clone()
            .expect("JitShape is all-wasm: every in-range slot is a compile_dispatch_fn_with product");
        let v = jw_call_wasm(ctx, tf, cur);
        let st = ctx.data();
        if !st.tail_pending.get() {
            return v;
        }
        // Consume the signal IMMEDIATELY (read-and-cleared in one breath — the engine rule).
        st.tail_pending.set(false);
        idx = st.tail_callee.get();
        core::mem::swap(&mut owned, &mut st.tail_args.borrow_mut());
        cur = &owned;
    }
}

/// The §6-(ii) JIT-shape leg: every function of `m` compiled with [`compile_dispatch_fn_with`]
/// (per-function module, `CallMode::Dispatch` — byte-frozen since before A-1) and instantiated
/// against ONE mock memory + table carrying the canonical helpers, the dispatch/tail trampoline
/// stubs, and the [`jit_resolve_paired`] host stubs. Scratch geometry is the AOT manifest's OWN
/// assignment, injected verbatim — instantiating §6's "(리졸버, 스크래치 기하) 동일" premise
/// rather than merely assuming it.
struct JitShape {
    store: Store<JitSt>,
}

impl JitShape {
    fn new(isa: &Isa, m: &IrModule, scratch: &[otter_wasmemit::AotScratchEntry]) -> JitShape {
        assert_eq!(scratch.len(), m.funcs.len());
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).expect("engine with fuel");
        let mut store = Store::new(
            &engine,
            JitSt {
                nfuncs: m.funcs.len(),
                fill: m.fill,
                compiled: vec![None; m.funcs.len()],
                memory: None,
                tail_pending: Cell::new(false),
                tail_callee: Cell::new(0),
                tail_args: RefCell::new(Vec::new()),
                staging_top: Cell::new(STAGING_BASE),
            },
        );
        let memory = Memory::new(&mut store, MemoryType::new(MEMORY_PAGES, None)).expect("memory");
        store.data_mut().memory = Some(memory);
        let table = Table::new(
            &mut store,
            wasmtime::TableType::new(wasmtime::RefType::FUNCREF, JW_TABLE_MIN as u32, None),
            Ref::Func(None),
        )
        .expect("table");

        // Trampoline stubs — calls.rs's `module_dispatch`/`tail_signal` mirrors, `callee_index`
        // (A-C1 full-width conversion) included.
        let dispatch_stub = Func::wrap(
            &mut store,
            |mut caller: Caller<'_, JitSt>, _ctx: i32, idx: i64, args: i32, argc: i64| -> i64 {
                let args = jw_read_args(&mut caller, args, argc);
                let mut ctx = caller.as_context_mut();
                jw_dispatch(&mut ctx, otter::interp::callee_index(idx), &args)
            },
        );
        let tail_stub = Func::wrap(
            &mut store,
            |mut caller: Caller<'_, JitSt>, _ctx: i32, callee: i64, args: i32, argc: i64| -> i64 {
                let copied = jw_read_args(&mut caller, args, argc);
                let st = caller.data();
                let mut buf = st.tail_args.borrow_mut();
                buf.clear();
                buf.extend_from_slice(&copied);
                drop(buf);
                st.tail_callee.set(otter::interp::callee_index(callee));
                st.tail_pending.set(true); // set LAST: the discriminant
                st.fill
            },
        );
        // Canonical helpers + the paired host stubs (the SAME native fns T0/the oracle run).
        let binop = Func::wrap(&mut store, |a: i64, b: i64, ty: u32, op: u32| -> i64 {
            otter::otter_binop(a, b, ty, op)
        });
        let convert = Func::wrap(&mut store, |a: i64, from: u32, to: u32| -> i64 {
            otter::otter_convert(a, from, to)
        });
        let unop = Func::wrap(&mut store, |a: i64, ty: u32, op: u32| -> i64 {
            otter::otter_unop(a, ty, op)
        });
        let h1 = Func::wrap(&mut store, |x: i64| -> i64 { common::h_add7(x) });
        let h2 = Func::wrap(&mut store, |x: i64, y: i64| -> i64 { common::h_mix(x, y) });
        let hc1 = Func::wrap(&mut store, |c: i64, x: i64| -> i64 { common::hc_ret_ctx(c, x) });
        let hc2 = Func::wrap(&mut store, |c: i64, x: i64, y: i64| -> i64 {
            common::hc_mix(c, x, y)
        });
        for (slot, f) in [
            (JW_DISPATCH_SLOT, dispatch_stub),
            (JW_TAIL_SLOT, tail_stub),
            (JW_BINOP_SLOT, binop),
            (JW_CONVERT_SLOT, convert),
            (JW_UNOP_SLOT, unop),
            (JW_H_ADD7_SLOT, h1),
            (JW_H_MIX_SLOT, h2),
            (JW_HC_RET_CTX_SLOT, hc1),
            (JW_HC_MIX_SLOT, hc2),
        ] {
            table
                .set(&mut store, u64::from(slot), Ref::Func(Some(f)))
                .expect("table.set");
        }

        for (j, f) in m.funcs.iter().enumerate() {
            // The ⊂-direction's GREEN half, exercised on every accepted module (§6: "AOT가 수용한
            // 프로그램은 JIT도 수용" — the acceptance predicate is the same code, so a refusal
            // here is a contract violation, not a fixture bug). Honesty note (review A#2): this
            // particular panic has never been observed to fire — mutation ⑦ turns the REFUSAL-pair
            // gate red (the 3-way corpus contains no JIT-refused function, so ⑦ cannot reach this
            // arm); the acceptance-direction claim rests on the shared-predicate construction plus
            // ⑦'s predicate-fork evidence, not on this panic having fired.
            let bytes = compile_dispatch_fn_with(
                isa,
                j,
                f,
                m.fill,
                m.host_ctx,
                jit_helper_idx(),
                &jit_resolve_paired,
                scratch[j].addr,
                EmitOpts::new(),
            )
            .unwrap_or_else(|r| {
                panic!(
                    "§6 ⊂-violation: compile_module accepted func {j} but the JIT surface refused it: {r:?}"
                )
            });
            let module = Module::new(&engine, &bytes).expect("JIT-shape module must validate");
            let instance = Instance::new(&mut store, &module, &[memory.into(), table.into()])
                .expect("instantiate against otter.memory/otter.table");
            let tf = instance
                .get_typed_func::<(i32, i32, i64), i64>(&mut store, "f")
                .expect("export f of the CompiledFn type");
            store.data_mut().compiled[j] = Some(tf);
        }
        JitShape { store }
    }

    fn dispatch(&mut self, idx: usize, args: &[Value]) -> Value {
        let mut ctx = self.store.as_context_mut();
        jw_dispatch(&mut ctx, idx, args)
    }
}

/// §2.5's normative sequential-disjoint assignment, asserted off the manifest: run `j`'s address
/// is EXACTLY the cumulative cursor and `scratch_region` ends exactly at the last run's end. This
/// is mutation ④'s enforcing pin inside the 3-way pipeline (see the ledger's ④ entry for why the
/// EXECUTION differential alone cannot catch an overlap under today's lowering).
fn assert_scratch_runs_disjoint(man: &otter_wasmemit::AotManifest) {
    let (base, end) = man.scratch_region;
    let mut cursor = base;
    for (j, e) in man.scratch.iter().enumerate() {
        assert_eq!(
            e.addr, cursor,
            "func {j}: §2.5 sequential disjoint scratch assignment violated"
        );
        cursor += 8 * e.words as u64;
    }
    assert_eq!(cursor, end, "manifest scratch_region end != last run's end");
}

/// The §6 3-way observational-equivalence gate for ONE module: (i) the T0 oracle
/// ([`run_module`], entry re-pointed per case), (ii) the JIT-shape leg ([`JitShape`]),
/// (iii) the AOT whole-module leg ([`Nin1`]) — every case BITWISE identical across all three.
/// Resolver pair fixed ([`jit_resolve_paired`]/[`compile_with_log`]'s `host_name`-derived
/// resolver), scratch geometry shared (manifest injection), `push_func` out of scope (section
/// doc). Returns the number of cases compared (non-vacuity bookkeeping).
fn check_three_way(isa: &Isa, m: &IrModule, cases: &[(usize, Vec<Vec<Value>>)]) -> usize {
    let (artifact, order) = compile_with_log(isa, m, &AotOpts::new(SCRATCH_BASE));
    assert_scratch_runs_disjoint(&artifact.manifest);
    let mut jit = JitShape::new(isa, m, &artifact.manifest.scratch);
    let mut aot = Nin1::new(&artifact, &order);
    let mut compared = 0usize;
    for (j, argsets) in cases {
        for args in argsets {
            let mut mj = m.clone();
            mj.entry = *j;
            let t0 = run_module(isa, &mj, args);
            let jv = jit.dispatch(*j, args);
            let av = aot.call(*j, args, args.len());
            assert_eq!(
                jv, t0,
                "3-way: JIT-shape ≠ T0 oracle: func {j} args={args:?}\nfunc={:?}",
                m.funcs[*j]
            );
            assert_eq!(
                av, t0,
                "3-way: AOT ≠ T0 oracle: func {j} args={args:?}\nfunc={:?}",
                m.funcs[*j]
            );
            compared += 1;
        }
    }
    compared
}

// --- §6 3-way differential: explicit corpora -----------------------------------------------------

/// 3-way over the call-free explicit corpus (dispatcher loop, host chain — all four host shapes —
/// consts folding, argc/fill probe) — one module, shared (fill, host_ctx), every function
/// compared through all three legs.
#[test]
fn three_way_explicit_call_free_corpus_is_bitwise_identical() {
    let isa = common::isa();
    let loop_sum = func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 1, 0, 0),
            Inst::new(MOV_IMM, 2, 1, 0),
            Inst::new(bin_opcode(3, 0), 1, 1, 0),
            Inst::new(bin_opcode(3, 1), 0, 0, 2),
            Inst::new(JUMP_IF, 0, 2, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    );
    // All FOUR host shapes in one function — Host1/Host2/HostCtx1/HostCtx2 each traverse the
    // paired resolvers once.
    let hosts = func(
        2,
        6,
        vec![
            Inst::new(HOST1, 2, 0, 0),
            Inst::new(HOST2, 3, 2, 1),
            Inst::new(common::HOSTCTX1, 4, 3, 0),
            Inst::new(HOSTCTX2, 5, 4, 1),
            Inst::new(HALT, 5, 0, 0),
        ],
    );
    let consts_fn = FuncDef {
        nparams: 1,
        consts: vec![10, -20, 0x0000_7FFF_FFFF_FFFF],
        nlocals: 6,
        code: vec![
            Inst::new(bin_opcode(3, 0), 4, 1, 2),
            Inst::new(bin_opcode(3, 0), 4, 4, 3),
            Inst::new(bin_opcode(3, 0), 4, 4, 0),
            Inst::new(HALT, 4, 0, 0),
        ],
    };
    let argc_probe = func(3, 3, vec![Inst::new(HALT, 2, 0, 0)]);
    let m = IrModule {
        funcs: vec![loop_sum, hosts, consts_fn, argc_probe],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let n = check_three_way(
        &isa,
        &m,
        &[
            (0, vec![vec![1], vec![7], vec![50]]),
            (1, vec![vec![1, 2], vec![-7, 1 << 40], vec![i64::MIN, i64::MAX]]),
            (2, vec![vec![5], vec![-3]]),
            (3, vec![vec![11, 22, 33]]),
        ],
    );
    assert_eq!(n, 9);
}

/// 3-way over the call-carrying explicit corpus — the gate ⑤ shapes (self-recursive `Call`,
/// self/mutual `TailCall`, wide-argc order-sensitive marshal, `argc=0` dead-args, mixed
/// Call-then-TailCall-tree) now adjudicated by ALL THREE executables, not just T0-vs-AOT.
/// `Halt` stays top-frame-only (§6 carve-out, section doc).
#[test]
fn three_way_explicit_call_carrying_corpus_is_bitwise_identical() {
    let isa = common::isa();
    #[allow(clippy::type_complexity)] // one-off corpus table, same shape as the A-3 corpus above
    let corpus: Vec<(&'static str, Vec<FuncDef>, Vec<(usize, Vec<Vec<Value>>)>)> = vec![
        (
            "fact",
            vec![fact_fn(), probe_fn()],
            vec![(0, vec![vec![0], vec![1], vec![5], vec![10]]), (1, vec![vec![]])],
        ),
        (
            "countdown",
            vec![tail_fn(0, 42), probe_fn()],
            vec![(0, vec![vec![0], vec![5], vec![20]])],
        ),
        (
            "even/odd",
            vec![tail_fn(1, 1), tail_fn(0, 0)],
            vec![
                (0, vec![vec![0], vec![7], vec![20]]),
                (1, vec![vec![0], vec![7], vec![20]]),
            ],
        ),
        (
            "marshal-8",
            marshal_pair(8),
            vec![(0, vec![vec![1, 2, 3, 4, 5, 6, 7, 8]])],
        ),
        (
            "marshal-257",
            marshal_pair(257),
            vec![(0, vec![(0..257).map(|k| k as i64 * 3 - 400).collect()])],
        ),
        ("zero-argc", zero_argc_pair(), vec![(0, vec![vec![]])]),
        (
            "mixed Call+TailCall tree",
            mixed_funcs(),
            vec![(0, vec![vec![0], vec![1], vec![5], vec![300]])],
        ),
    ];
    for (name, funcs, cases) in corpus {
        let m = IrModule {
            funcs,
            fill: FILL,
            host_ctx: HOST_CTX,
            entry: 0,
        };
        let n = check_three_way(&isa, &m, &cases);
        assert!(n > 0, "{name}: vacuous case list");
    }
}

// --- §6 3-way differential: fuzz (same generators, same seeds, same counts as the A-1 corpus) -----

/// The straight-line fuzz stream — generator, seed AND count identical to
/// `differential::fuzz_straightline_matches_oracle` / `nin1_fuzz_straightline_same_corpus_matches_oracle`
/// (corpus-identical, adjudication widened to 3-way).
#[test]
fn three_way_fuzz_straightline_same_corpus() {
    let isa = common::isa();
    let mut next = lcg(0x9E37_79B9_7F4A_7C15);
    for _ in 0..400 {
        let (f, args, fill, host_ctx) = gen_straightline(&mut next);
        let m = IrModule {
            funcs: vec![f, probe_fn()],
            fill,
            host_ctx,
            entry: 0,
        };
        check_three_way(&isa, &m, &[(0, vec![args]), (1, vec![vec![]])]);
    }
}

/// The random-CFG fuzz stream — generator, seed AND count identical to
/// `differential::fuzz_cfg_matches_oracle` / `nin1_fuzz_cfg_same_corpus_matches_oracle`.
#[test]
fn three_way_fuzz_cfg_same_corpus() {
    let isa = common::isa();
    let mut next = lcg(0xA5F1_5EED_0D15_EA5E);
    for _ in 0..300 {
        let (f, args, fill, host_ctx) = gen_cfg(&mut next);
        let m = IrModule {
            funcs: vec![f, probe_fn()],
            fill,
            host_ctx,
            entry: 0,
        };
        check_three_way(&isa, &m, &[(0, vec![args]), (1, vec![vec![]])]);
    }
}

// --- §6 fail-closed shape equivalence: JIT-shape ↔ AOT tier pair ----------------------------------

/// §6's "fail-closed 형상(OOB/음수 콜리 → fill) 동등" across the two COMPILED legs: all four
/// garbage-callee shapes (OOB, plain negative, and A-C1's two full-width truncation-hazard words
/// `2^32+1` / `-2^32`), on both `Call` and `TailCall`, yield `fill` on the JIT-shape leg (engine
/// trampoline loop-head check + `callee_index`) AND on the AOT leg (`i64.ge_u` guard) — equal to
/// each other and to the module's `fill`, never a trap.
///
/// The T0 [`run_module`] oracle is DELIBERATELY absent here, same as `calls.rs`'s OOB test:
/// `run_module` has no OOB-callee guard at all (it panics in `make_frame` — its own documented
/// contract: "Panics on a malformed module"), so it cannot adjudicate this shape; the canonical
/// fail-closed semantics for a dispatched call are the ENGINE's (loop-head `fill` fallback),
/// which the JIT-shape leg mirrors statement-for-statement. This is the §6 contract's own
/// framing: observational equivalence of the COMPILED modes' fail-closed behavior.
#[test]
fn three_way_fail_closed_garbage_callee_jit_and_aot_agree_on_fill() {
    let isa = common::isa();
    for shape in [
        GarbageShape::Oob,
        GarbageShape::Negative,
        GarbageShape::FullWidePos,
        GarbageShape::FullWideNeg,
    ] {
        for (name, f) in [
            ("Call", garbage_call_fn(shape)),
            ("TailCall", garbage_tailcall_fn(shape)),
        ] {
            let m = IrModule {
                funcs: vec![f, probe_fn()],
                fill: FILL,
                host_ctx: HOST_CTX,
                entry: 0,
            };
            let (artifact, order) = compile_with_log(&isa, &m, &AotOpts::new(SCRATCH_BASE));
            let mut jit = JitShape::new(&isa, &m, &artifact.manifest.scratch);
            let mut aot = Nin1::new(&artifact, &order);
            let jv = jit.dispatch(0, &[]);
            let av = aot.call(0, &[], 0);
            assert_eq!(
                jv, FILL,
                "{name} {shape:?}: JIT-shape leg must fail closed to fill"
            );
            assert_eq!(av, FILL, "{name} {shape:?}: AOT leg must fail closed to fill");
            assert_eq!(jv, av, "{name} {shape:?}: the two compiled modes must agree");
        }
    }
}

// --- §6 ⊂-direction: refusal-pair gate (the red side) ---------------------------------------------

/// §6's refusal half: for every FUNCTION-LEVEL refusal shape, `compile_module` fails with
/// `AotError::Func {{ func_idx, refusal }}` AND `compile_dispatch_fn_with` on the SAME function
/// under the SAME (resolver, scratch geometry) parameterization fails with the IDENTICAL
/// `Refusal` VALUE — the §4.2 "동일 코드" claim made executable in the rejecting direction
/// (the accepting direction is [`JitShape::new`]'s ⊂-violation panic, exercised on every 3-way
/// corpus module; mutation ⑦'s ledger entry proves both directions non-vacuous).
///
/// The five AOT-ONLY refusal sources (§6's corrected list) have no JIT twin BY DESIGN and are
/// asserted elsewhere in this file (§8.4 block): module-level `ScratchBaseMisaligned` /
/// `EntryOutOfRange` / `DuplicateHostImportName` (+ the untestable-by-input `TooManyTableSlots`)
/// — a per-function surface has no module to check — and the resolver/scratch PARAMETER
/// differences behind `HostFnUnresolved`/`ScratchOutOfAddressSpace`, which this test neutralizes
/// by construction (paired empty resolvers; the AOT-assigned address re-injected), so the
/// refusal VALUES match exactly even for those two.
#[test]
fn subset_refusal_pairs_aot_and_jit_refuse_identically() {
    let isa = common::isa();
    let jit_none = |_: usize| -> Option<u32> { None };
    let aot_none = |_: usize| -> Option<HostImportName> { None };

    // (bad function, its ISA, the paired JIT resolver, the JIT-side scratch addr = what the AOT
    // single pass assigns func 1 after func 0 (= probe, 0 words): SCRATCH_BASE — except the
    // scratch case, which overrides base to the §2.5 boundary.)
    let host1_fn = func(1, 2, vec![Inst::new(HOST1, 1, 0, 0), Inst::new(HALT, 1, 0, 0)]);
    let over = MAX_WASM_FUNCTION_LOCALS - 3;
    let locals_fn = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: over,
        code: vec![Inst::new(HALT, 0, 0, 0)],
    };
    let scratch_fn = FuncDef {
        nparams: 0,
        consts: vec![],
        nlocals: 5,
        code: vec![Inst::new(CALL, 3, 0, 2), Inst::new(HALT, 3, 0, 0)],
    };
    let mut bad_isa = common::isa();
    let bad_op = bad_isa.ops.len() as u16;
    bad_isa.ops.push(otter::OpDef {
        name: "ret_then_write",
        effects: vec![
            otter::Effect::Return {
                value: otter::Expr::LocalAt(0),
            },
            otter::Effect::SetLocalAt {
                reg: 0,
                value: otter::Expr::Imm(1),
            },
        ],
        hints: otter::OpHints {
            pure: false,
            terminator: true,
        },
    });
    let ctl_fn = func(0, 1, vec![Inst::new(bad_op, 0, 0, 0)]);

    #[allow(clippy::type_complexity)] // one-off fixture table; the tuple IS the documentation
    let fixtures: Vec<(&'static str, &Isa, FuncDef, &dyn Fn(usize) -> Option<u32>, u64)> = vec![
        (
            "ValidationFailed",
            &isa,
            malformed_fn(),
            &jit_resolve_paired,
            SCRATCH_BASE,
        ),
        (
            "IntegerDiv",
            &isa,
            integer_div_fn(),
            &jit_resolve_paired,
            SCRATCH_BASE,
        ),
        (
            "FallsOffEnd",
            &isa,
            func(0, 1, vec![Inst::new(MOV_IMM, 0, 7, 0)]),
            &jit_resolve_paired,
            SCRATCH_BASE,
        ),
        (
            "TrailingJumpIf",
            &isa,
            func(1, 2, vec![Inst::new(JUMP_IF, 0, 0, 0)]),
            &jit_resolve_paired,
            SCRATCH_BASE,
        ),
        (
            "ControlEffectNotLast",
            &bad_isa,
            ctl_fn,
            &jit_resolve_paired,
            SCRATCH_BASE,
        ),
        (
            "WasmLocalsLimitExceeded",
            &isa,
            locals_fn,
            &jit_resolve_paired,
            SCRATCH_BASE,
        ),
        ("HostFnUnresolved", &isa, host1_fn, &jit_none, SCRATCH_BASE),
        (
            "ScratchOutOfAddressSpace",
            &isa,
            scratch_fn,
            &jit_resolve_paired,
            0xFFFF_FFF8,
        ),
    ];
    for (name, fisa, bad, jit_resolve, base) in fixtures {
        let m = IrModule {
            funcs: vec![probe_fn(), bad.clone()],
            fill: FILL,
            host_ctx: HOST_CTX,
            entry: 0,
        };
        // AOT side: whole-module failure, attributed to func 1.
        let aot_resolve: &dyn Fn(usize) -> Option<HostImportName> = if name == "HostFnUnresolved" {
            &aot_none
        } else {
            &resolve_all_named
        };
        let aot_err = compile_module(fisa, &m, aot_resolve, &AotOpts::new(base))
            .expect_err("§6 ⊂-direction: this fixture must be AOT-refused");
        let AotError::Func { func_idx, refusal } = aot_err else {
            panic!("{name}: expected AotError::Func, got {aot_err:?}")
        };
        assert_eq!(func_idx, 1, "{name}: attributed to the bad function");
        // JIT side: the same function under the same (resolver, scratch geometry) must refuse
        // with the IDENTICAL value.
        let jit_err = compile_dispatch_fn_with(
            fisa,
            1,
            &bad,
            m.fill,
            m.host_ctx,
            jit_helper_idx(),
            jit_resolve,
            base, // func 0 (probe) spans 0 words, so func 1's assigned addr == base
            EmitOpts::new(),
        )
        .expect_err("§6 ⊂-direction: the JIT surface must refuse the same shape");
        assert_eq!(
            jit_err, refusal,
            "{name}: AOT refusal and JIT refusal must be the same value (§4.2 same-code claim)"
        );
    }
}

// --- O-19 §2.6: the record==live A64 exemption is LOUD ---------------------------------------------

/// O-19 §2.6 [결정 v2]: A64 corpus is NOT part of the record==live gates — and its NON-inclusion
/// is machine-asserted in the failure direction: feeding an A64 artifact to either R helper is an
/// immediate panic (the guards at their heads), never a silent mis-comparison against the A32
/// live twin. This is the "(i) 포함 시 즉시 적색" half of §2.6; the "(ii) 커버리지 0 출하" half
/// is covered by the modulo-addrtype differential gate (tests/addrtype.rs), which asserts the
/// A64 body IS the same lowering modulo the §2.2-enumerated width points.
#[test]
fn record_equals_live_rejects_a64_corpus_loudly() {
    let isa = common::isa();
    let opts = AotOpts::new(SCRATCH_BASE).with_memory64(true);

    // R1 leg: call-free corpus.
    let m_free = module_of(vec![probe_fn(), probe_fn()]);
    let (a_free, o_free) = compile_with_log(&isa, &m_free, &opts);
    assert!(a_free.manifest.memory64, "manifest must carry the A64 flag");
    let red = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_record_equals_live(&isa, &m_free, &a_free, &o_free)
    }));
    assert!(red.is_err(), "R1 must reject an A64 artifact loudly (O-19 §2.6)");

    // R2 leg: call-carrying corpus.
    let call_fn = func(
        0,
        4,
        vec![
            Inst::new(MOV_IMM, 0, 0, 0),
            Inst::new(MOV_IMM, 1, 5, 0),
            Inst::new(CALL, 2, 0, 1),
            Inst::new(HALT, 2, 0, 0),
        ],
    );
    let m_call = module_of(vec![call_fn, probe_fn()]);
    let (a_call, o_call) = compile_with_log(&isa, &m_call, &opts);
    let red = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_record_equals_live_windowed(&isa, &m_call, &a_call, &o_call)
    }));
    assert!(red.is_err(), "R2 must reject an A64 artifact loudly (O-19 §2.6)");
}

// --- the anti-silent-skip guarantee (A-4 item 4) --------------------------------------------------

/// The ONE structural guarantee that no refactor silently drops the A-4 gate suite: this file's
/// own source (compile-time `include_str!` — travels with the binary, zero external wiring) must
/// still DEFINE every registry-listed gate function, and must contain no ignore-attribute at all
/// (none of these gates may ever be soft-disabled; the needle is built at runtime so this test's
/// own source cannot trip it). What this does and does not catch — recorded honestly: it catches
/// deletion and renaming of any gate (the definition string disappears → red) and
/// ignore-attribute insertion in both its spellings (the plain/`= "reason"` forms via the prefix
/// needle, and the cfg_attr soft-disable form — a conditional attribute whose second argument is
/// the ignore token — via a comma-form needle; the latter hole was found and closed by review B);
/// it does NOT catch a
/// `#[cfg]`-out (the source text survives) or CI dropping `--workspace` (which would drop this
/// meta-test too — that failure mode is not silently-skipping ONE gate but dropping the whole
/// suite, guarded by the registry comments in `justfile`/`ci.yml` naming this file). A
/// `--list`-based count floor in CI was considered and rejected as extra wiring for the same
/// coverage (A-4 judgment: 과잉 배선 금지).
#[test]
fn a4_gate_registry_tests_exist_and_none_are_ignored() {
    let src = include_str!("aot.rs");
    // R1/R2 gate helpers + R3 mutation evidence + the 3-way/⊂ gates + their corpus carriers.
    let registry = [
        // record==live R1 (call-free full-text) + its corpus carriers
        "assert_record_equals_live",
        "nin1_explicit_multifunction_module_matches_oracle",
        "nin1_fuzz_straightline_same_corpus_matches_oracle",
        "nin1_fuzz_cfg_same_corpus_matches_oracle",
        "nin1_fuzz_pairs_share_one_interner_and_t_normalization_is_non_vacuous",
        // record==live R2 (window-localized)
        "assert_record_equals_live_windowed",
        "record_equals_live_r2_windowed_on_call_carrying_modules",
        // record==live R3 (mutation evidence, permanent form)
        "mutation_table_mode_noncall_lowering_turns_r1_and_r2_red",
        // §6 3-way differential + fail-closed + ⊂-direction
        "check_three_way",
        "three_way_explicit_call_free_corpus_is_bitwise_identical",
        "three_way_explicit_call_carrying_corpus_is_bitwise_identical",
        "three_way_fuzz_straightline_same_corpus",
        "three_way_fuzz_cfg_same_corpus",
        "three_way_fail_closed_garbage_callee_jit_and_aot_agree_on_fill",
        "subset_refusal_pairs_aot_and_jit_refuse_identically",
    ];
    for name in registry {
        assert!(
            src.contains(&format!("fn {name}(")),
            "gate registry violation: `fn {name}(` is no longer defined in tests/aot.rs — if this \
             was an intentional rename/move, update the registry (and the justfile/ci.yml gate \
             comments) in the same change"
        );
    }
    // Built at runtime so the needles themselves never appear verbatim in this file's source (the
    // prefix form also catches the `= "reason"` variant of the attribute; the comma form catches
    // the cfg_attr soft-disable spelling, i.e. a conditional attribute closing with the ignore
    // token — review B demonstrated the prefix needle alone lets that variant pass, `1 ignored`).
    let ignore_needle = format!("#[{}", "ignore");
    let cfg_attr_ignore_needle = format!(", {})]", "ignore");
    assert!(
        !src.contains(&ignore_needle) && !src.contains(&cfg_attr_ignore_needle),
        "gate registry violation: an ignore attribute (plain or cfg_attr form) appeared in \
         tests/aot.rs — the A-4 gate suite must never be soft-disabled"
    );
}
