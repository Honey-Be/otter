//! O-19 C2 — the A64 (memory64) emission gates (design `otter-o19-wasm64-design.md` §2/§2.4/
//! §2.6/§6-2..§6-5):
//!
//! * **modulo-addrtype differential** (§2.6 [결정 v2 — 옵션 (b)]): the SAME IR emitted A32 and
//!   A64 must be the SAME lowering everywhere except the §2.2-enumerated width points — the
//!   compiled type's pointer params, the baked scratch-address constants, the memory import's
//!   memory64 flag — machine-asserted op-by-op and section-by-section below. This is the A64
//!   structural gate that REPLACES record==live for A64 (which is explicitly exempt: no live
//!   twin source exists while `EmitOpts` stays un-extended — the exemption's loud-failure half
//!   lives in tests/aot.rs `record_equals_live_rejects_a64_corpus_loudly`).
//! * **mixed shape** (§2.1 [결정]): memory64 memory import + 32-bit declared table — the
//!   table/element/`call_indirect` machinery must be BYTE-identical across addrtypes.
//! * **feature-keyed validation**: the A64 product validates under wasmparser's defaults
//!   (memory64 default-ON in 0.258) and is REJECTED by a validator with the memory64 feature
//!   subtracted — the machine proof the product genuinely uses the proposal.
//! * **§2.4 bound**: the A64 scratch span cap is `2^63` (exclusive-end-representable in u64),
//!   `checked_add`-total, with the end-exactly-at-cap edge legal.
//! * **fail-closed negatives**: shared × memory64 refuses (`SharedMemory64Unsupported`), and
//!   the input-check order keeps alignment first.

mod common;

use common::{
    CALL, COPY, DEC_JNZ, HALT, HOST1, HOST2, HOSTCTX1, HOSTCTX2, MOV_IMM, RET, TAILCALL, bin_opcode, func,
};
use otter::{FuncDef, Inst, Module as IrModule};
use otter_wasmemit::{
    AotArtifact, AotError, AotOpts, HostImportName, Refusal, compile_module, validate_aot_artifact,
};
use wasmparser::{Operator, Parser, Payload, TypeRef, ValType, Validator, WasmFeatures};

const FILL: otter::Value = 0x5EED_F00D_5EED_F00D_u64 as otter::Value;
const HOST_CTX: otter::Value = 0x7E57_C0DE_7E57_C0DE_u64 as otter::Value;
const SCRATCH_BASE: u64 = 0x1_0000;

fn resolve_aot(p: usize) -> Option<HostImportName> {
    let name = if p == common::h_add7 as otter::HostFn1 as usize {
        "h_add7"
    } else if p == common::h_mix as otter::HostFn2 as usize {
        "h_mix"
    } else if p == common::hc_ret_ctx as otter::HostFnCtx1 as usize {
        "hc_ret_ctx"
    } else if p == common::hc_mix as otter::HostFnCtx2 as usize {
        "hc_mix"
    } else {
        return None;
    };
    Some(HostImportName(name.to_string()))
}

/// The differential corpus: `Call` marshal (f0), `TailCall` (f1), a dispatcher loop with params
/// and a `HostCtx2` site (f2 — the site whose A64 wasm type STRUCTURALLY COLLIDES with the A64
/// compiled type, exercising the interner-dedup cascade on purpose), and the remaining host
/// shapes (f3) — every §2.2 "바뀌는 곳" and "안 바뀌는 곳" class is represented.
fn mixed_module() -> IrModule {
    let f0 = FuncDef {
        nparams: 0,
        consts: vec![0x7777_0000_0000_0001],
        nlocals: 6,
        code: vec![
            Inst::new(MOV_IMM, 1, 1, 0),
            Inst::new(MOV_IMM, 2, 9, 0),
            Inst::new(CALL, 3, 1, 1),
            Inst::new(bin_opcode(3, 0), 4, 3, 0), // i64 add
            Inst::new(HOST1, 5, 4, 0),
            Inst::new(HALT, 5, 0, 0),
        ],
    };
    let f1 = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(MOV_IMM, 1, 2, 0),
            Inst::new(COPY, 2, 0, 0),
            Inst::new(TAILCALL, 1, 1, 0),
        ],
    };
    let f2 = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 3,
        code: vec![
            Inst::new(HOSTCTX2, 1, 0, 0),
            Inst::new(DEC_JNZ, 0, 0, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    };
    let f3 = FuncDef {
        nparams: 2,
        consts: vec![],
        nlocals: 4,
        code: vec![
            Inst::new(HOST2, 2, 0, 1),
            Inst::new(HOSTCTX1, 3, 2, 0),
            Inst::new(RET, 3, 0, 0),
        ],
    };
    IrModule {
        funcs: vec![f0, f1, f2, f3],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    }
}

fn compile_pair() -> (AotArtifact, AotArtifact) {
    let isa = common::isa();
    let m = mixed_module();
    let a32 = compile_module(&isa, &m, &resolve_aot, &AotOpts::new(SCRATCH_BASE)).expect("A32 compiles");
    let a64 = compile_module(
        &isa,
        &m,
        &resolve_aot,
        &AotOpts::new(SCRATCH_BASE).with_memory64(true),
    )
    .expect("A64 compiles");
    (a32, a64)
}

// --- shared parsing helpers ------------------------------------------------------------------------

/// The module's function types, structurally, in type-section order (aot.rs `func_types` twin).
fn func_types(bytes: &[u8]) -> Vec<(Vec<ValType>, Vec<ValType>)> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::TypeSection(r) = payload.expect("parse") {
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

fn ty_str(t: &(Vec<ValType>, Vec<ValType>)) -> String {
    format!("{t:?}")
}

/// T-NORMALIZED operator strings of every function body (aot.rs `decode_norm_ops` twin, all
/// bodies at once): `call_indirect`/`return_call_indirect` render their target type
/// STRUCTURALLY (through the module's OWN type section) instead of by raw index — required here
/// because the A64 interner legally DEDUPS `HostCtx2`'s `(i64,i64,i64)->i64` onto the A64
/// compiled type, shifting every later raw type index by one relative to A32 (a width-cascade,
/// not a second lowering — this normalization is exactly what proves that).
fn norm_bodies(bytes: &[u8]) -> Vec<Vec<String>> {
    let types = func_types(bytes);
    let mut bodies = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("parse") {
            let mut ops = Vec::new();
            // Review F1 (O-19 C0-C2 review lens A): the locals DECLARATIONS are part of the
            // lockstep too — without this entry, an addrtype-conditional extra local (a second
            // emission path smuggled through declarations rather than operators) passes every
            // layer, because A64 is record==live-exempt and the operator walk starts past the
            // locals vector. Locals are addrtype-invariant today, so any A32/A64 difference here
            // is a divergence OUTSIDE the §2.2-enumerated width points and must go red.
            let locals: Vec<(u32, ValType)> = body
                .get_locals_reader()
                .expect("locals")
                .into_iter()
                .map(|e| e.expect("locals entry"))
                .collect();
            ops.push(format!("Locals{locals:?}"));
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                let op = reader.read().expect("operator");
                let s = match op {
                    Operator::CallIndirect {
                        type_index,
                        table_index,
                    } => format!(
                        "CallIndirectNorm{{ty={}, table={table_index}}}",
                        ty_str(&types[type_index as usize])
                    ),
                    Operator::ReturnCallIndirect {
                        type_index,
                        table_index,
                    } => format!(
                        "ReturnCallIndirectNorm{{ty={}, table={table_index}}}",
                        ty_str(&types[type_index as usize])
                    ),
                    other => format!("{other:?}"),
                };
                ops.push(s);
            }
            bodies.push(ops);
        }
    }
    bodies
}

/// Raw content bytes of section `wanted` (by section name tag used below).
fn section_bytes(bytes: &[u8], wanted: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        let range = match (payload.expect("parse"), wanted) {
            (Payload::FunctionSection(r), "function") => r.range(),
            (Payload::TableSection(r), "table") => r.range(),
            (Payload::ExportSection(r), "export") => r.range(),
            (Payload::ElementSection(r), "element") => r.range(),
            _ => continue,
        };
        out = bytes[range.start as usize..range.end as usize].to_vec();
    }
    out
}

const COMPILED32: (&[ValType], &[ValType]) = (&[ValType::I32, ValType::I32, ValType::I64], &[ValType::I64]);
const COMPILED64: (&[ValType], &[ValType]) = (&[ValType::I64, ValType::I64, ValType::I64], &[ValType::I64]);

fn ty_pair(t: (&[ValType], &[ValType])) -> (Vec<ValType>, Vec<ValType>) {
    (t.0.to_vec(), t.1.to_vec())
}

// --- gates ------------------------------------------------------------------------------------------

/// Feature-keyed validation (§6-2): the A64 product validates under wasmparser 0.258 defaults
/// (memory64 is default-ON there — the C6 documented 실측) AND is rejected once the MEMORY64
/// feature is subtracted, proving the bytes genuinely use the proposal; the A32 product
/// validates under BOTH validators. `Err` asserted, no message-string pin (§4 귀속 정정 norm).
#[test]
fn a64_validates_by_default_and_requires_the_memory64_feature() {
    let (a32, a64) = compile_pair();
    Validator::new()
        .validate_all(&a64.bytes)
        .expect("A64 must validate under default features (memory64 default-ON, 0.258)");
    Validator::new()
        .validate_all(&a32.bytes)
        .expect("A32 must validate under default features");

    let without = WasmFeatures::default() & !WasmFeatures::MEMORY64;
    assert!(
        Validator::new_with_features(without)
            .validate_all(&a64.bytes)
            .is_err(),
        "A64 must be REJECTED without the memory64 feature — the product really uses it"
    );
    Validator::new_with_features(without)
        .validate_all(&a32.bytes)
        .expect("A32 must not depend on the memory64 feature");

    // §2.5 documented status, pinned as-is: the pre-link whitelist validator is section-keyed
    // only, so an A64 pre-link artifact passes it TODAY — the manifest×bytes memory64
    // cross-check is C5's record==live extension, not this validator's job.
    validate_aot_artifact(&a64.bytes).expect("whitelist validator is addrtype-blind (§2.5)");
}

/// The §2.1 mixed shape, asserted on the parsed A64 bytes: 64-bit non-shared memory import,
/// 32-bit declared table (min=max=T), unchanged section set/order, all-compiled function
/// section, and the manifest flag mirroring the opts.
#[test]
fn a64_structure_is_the_mixed_shape() {
    let (a32, a64) = compile_pair();
    assert!(
        a64.manifest.memory64 && !a32.manifest.memory64,
        "manifest mirrors opts"
    );

    let mut sections = Vec::new();
    for payload in Parser::new(0).parse_all(&a64.bytes) {
        match payload.expect("parse") {
            Payload::Version { .. } | Payload::End(_) => {}
            Payload::TypeSection(_) => sections.push("type"),
            Payload::ImportSection(r) => {
                sections.push("import");
                let imports: Vec<_> = r.into_imports().map(|i| i.expect("import")).collect();
                assert_eq!((imports[0].module, imports[0].name), ("otter", "memory"));
                match imports[0].ty {
                    TypeRef::Memory(mem) => {
                        assert!(mem.memory64, "O-19: the 64-bit memory import IS the change");
                        assert!(!mem.shared, "shared×A64 is fail-closed refused, never emitted");
                        assert_eq!(mem.initial, 0, "min 0 links against any current size");
                        assert_eq!(mem.maximum, None, "no max — non-shared shape keeps none");
                    }
                    ref other => panic!("otter.memory must be a memory import, got {other:?}"),
                }
                // No table IMPORT in any AOT shape since A-2 (the table is module-declared).
                assert!(
                    !imports.iter().any(|i| matches!(i.ty, TypeRef::Table(_))),
                    "AOT artifacts declare their table, never import one"
                );
            }
            Payload::FunctionSection(r) => {
                sections.push("function");
                for e in r {
                    assert_eq!(
                        e.expect("func"),
                        0,
                        "every function is the compiled type (index 0)"
                    );
                }
            }
            Payload::TableSection(r) => {
                sections.push("table");
                for t in r {
                    let t = t.expect("table");
                    assert!(
                        !t.ty.table64,
                        "MIXED shape (§2.1): the table stays 32-bit under A64"
                    );
                    assert!(t.ty.element_type.is_func_ref());
                    assert_eq!(t.ty.initial, u64::from(a64.manifest.table_size));
                    assert_eq!(t.ty.maximum, Some(u64::from(a64.manifest.table_size)));
                }
            }
            Payload::ExportSection(_) => sections.push("export"),
            Payload::ElementSection(r) => {
                sections.push("element");
                for e in r {
                    let e = e.expect("element");
                    match e.kind {
                        wasmparser::ElementKind::Active { table_index, .. } => {
                            assert_eq!(table_index.unwrap_or(0), 0, "targets the declared table 0");
                        }
                        _ => panic!("expected an active element segment"),
                    }
                }
            }
            Payload::CodeSectionStart { count, .. } => {
                sections.push("code");
                assert_eq!(count, 4, "one body per corpus function");
            }
            Payload::CodeSectionEntry(_) => {}
            Payload::StartSection { .. } => panic!("emitted module must NEVER have a start section"),
            other => panic!("unexpected section in the A64 artifact: {other:?}"),
        }
    }
    assert_eq!(
        sections,
        ["type", "import", "function", "table", "export", "element", "code"],
        "§2.1 A-3 section set/order, unchanged by the addrtype"
    );
    // The compiled type itself is the A64 twin (ctx/args pointers widened — §2.2 #1).
    assert_eq!(func_types(&a64.bytes)[0], ty_pair(COMPILED64));
    assert_eq!(func_types(&a32.bytes)[0], ty_pair(COMPILED32));
}

/// **The §2.6 modulo-addrtype differential gate.** Same IR, A32 vs A64 artifacts — every
/// difference between the two products must sit at a §2.2-enumerated width point and NOWHERE
/// else:
///
/// 1. code: op-for-op lockstep (equal lengths — every divergent site is 1 op ↔ 1 op), the only
///    divergences being the scratch-address constant (`i32.const a` ↔ `i64.const a`, SAME
///    address value, at that function's manifest scratch addr) and the compiled type at
///    `Call`/`TailCall` sites (structurally rendered A32↔A64 compiled type, same table);
/// 2. type section: exactly the A32 table with the compiled entry width-swapped, then
///    structurally re-deduped (the `HostCtx2` collision cascade — asserted exercised);
/// 3. imports: identical module/name/kind/structural-type lists except the memory entry's
///    memory64 flag;
/// 4. function/table/export/element sections: byte-identical (혼합형 — the table machinery is
///    untouched, S2-M1-3);
/// 5. manifest: identical except the `memory64` mirror.
///
/// Non-vacuity is asserted for BOTH divergence classes, and (structurally) for the dedup
/// cascade. A64 corpus is NOT fed to record==live — its exemption is enforced loudly there
/// (tests/aot.rs) and this gate is the replacement structural witness (§2.6).
#[test]
fn modulo_addrtype_diff_is_confined_to_enumerated_points() {
    let (a32, a64) = compile_pair();
    let man32 = &a32.manifest;
    let man64 = &a64.manifest;

    // 5. manifest: memory64 is the ONLY field allowed to differ.
    let mut m64 = man64.clone();
    m64.memory64 = false;
    assert_eq!(
        &m64, man32,
        "manifests must agree on everything but the memory64 mirror"
    );

    // 4. untouched sections, byte-for-byte.
    for sec in ["function", "table", "export", "element"] {
        assert_eq!(
            section_bytes(&a32.bytes, sec),
            section_bytes(&a64.bytes, sec),
            "{sec} section must be byte-identical across addrtypes (혼합형)"
        );
    }

    // 3. imports: structural comparison (raw type-index immediates may shift — the dedup
    // cascade), memory64 flag the only allowed difference.
    let parse_imports = |bytes: &[u8]| -> Vec<(String, String, String)> {
        let types = func_types(bytes);
        let mut out = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            if let Payload::ImportSection(r) = payload.expect("parse") {
                for imp in r.into_imports() {
                    let imp = imp.expect("import");
                    let ty = match imp.ty {
                        TypeRef::Func(i) => format!("func {}", ty_str(&types[i as usize])),
                        TypeRef::Memory(mem) => {
                            // The memory64 flag is the allowed divergence — mask it here and
                            // assert it separately below.
                            format!(
                                "memory shared={} min={} max={:?}",
                                mem.shared, mem.initial, mem.maximum
                            )
                        }
                        ref other => format!("{other:?}"),
                    };
                    out.push((imp.module.to_string(), imp.name.to_string(), ty));
                }
            }
        }
        out
    };
    assert_eq!(
        parse_imports(&a32.bytes),
        parse_imports(&a64.bytes),
        "imports must agree on module/name/shape modulo the memory64 flag"
    );

    // 2. type section: the A32 table, compiled entry width-swapped, structurally re-deduped.
    let t32 = func_types(&a32.bytes);
    let t64 = func_types(&a64.bytes);
    let mut expected64: Vec<(Vec<ValType>, Vec<ValType>)> = Vec::new();
    for t in &t32 {
        let mapped = if *t == ty_pair(COMPILED32) {
            ty_pair(COMPILED64)
        } else {
            t.clone()
        };
        if !expected64.contains(&mapped) {
            expected64.push(mapped);
        }
    }
    assert_eq!(
        t64, expected64,
        "type table must be exactly the width-swap of the A32 table (modulo structural dedup)"
    );
    assert!(
        t64.len() < t32.len(),
        "corpus must exercise the HostCtx2↔compiled-A64 dedup cascade (non-vacuity)"
    );

    // 1. code: T-normalized op lockstep with exactly the two enumerated divergence classes.
    let b32 = norm_bodies(&a32.bytes);
    let b64 = norm_bodies(&a64.bytes);
    assert_eq!(b32.len(), b64.len(), "one body per function on both sides");
    let comp32_call = format!("ty={}", ty_str(&ty_pair(COMPILED32)));
    let comp64_call = format!("ty={}", ty_str(&ty_pair(COMPILED64)));
    let mut scratch_divergences = 0usize;
    let mut compiled_ty_divergences = 0usize;
    for (j, (ops32, ops64)) in b32.iter().zip(&b64).enumerate() {
        assert_eq!(
            ops32.len(),
            ops64.len(),
            "func {j}: modulo-addrtype ops must be 1:1 (every width point is one op on each side)"
        );
        let addr = man64.scratch[j].addr;
        assert_eq!(addr, man32.scratch[j].addr, "func {j}: same scratch geometry");
        let scratch32 = format!("I32Const {{ value: {} }}", addr as u32 as i32);
        let scratch64 = format!("I64Const {{ value: {} }}", addr as i64);
        for (i, (o32, o64)) in ops32.iter().zip(ops64).enumerate() {
            if o32 == o64 {
                continue;
            }
            if o32 == &scratch32 && o64 == &scratch64 {
                scratch_divergences += 1; // §2.2 #2/#3 — the address-constant width point
                continue;
            }
            let is_compiled_pair = o32.contains(&comp32_call)
                && o64.contains(&comp64_call)
                && o32.replace(&comp32_call, "") == o64.replace(&comp64_call, "");
            if is_compiled_pair {
                compiled_ty_divergences += 1; // §2.2 #1 — the compiled-type width point
                continue;
            }
            panic!(
                "func {j} op {i}: divergence OUTSIDE the §2.2-enumerated width points:\n  A32: {o32}\n  A64: {o64}"
            );
        }
    }
    assert!(
        scratch_divergences > 0,
        "gate vacuous: no scratch-address width point exercised"
    );
    assert!(
        compiled_ty_divergences > 0,
        "gate vacuous: no compiled-type width point exercised"
    );
}

/// §2.4: the A64 scratch bound is a `2^63` CAP (not `2^64` — the exclusive u64 end must stay
/// representable), `>`-exclusive exactly like the A32 `2^32` bound: a span ending exactly AT
/// the cap is legal (and recorded exactly in the manifest), one byte past refuses with the
/// exact base as the diagnostic.
#[test]
fn a64_scratch_bound_is_capped_at_2_63() {
    let isa = common::isa();
    let probe = func(0, 1, vec![Inst::new(HALT, 0, 0, 0)]);
    let call2 = func(0, 5, vec![Inst::new(CALL, 3, 0, 2), Inst::new(HALT, 3, 0, 0)]);
    let module_of = |funcs: Vec<FuncDef>| IrModule {
        funcs,
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };

    // span 16 ending exactly at 2^63 — legal, manifest says so exactly.
    let base = (1u64 << 63) - 16;
    let m = module_of(vec![probe.clone(), call2.clone()]);
    let a = compile_module(&isa, &m, &resolve_aot, &AotOpts::new(base).with_memory64(true))
        .expect("A64 span ending exactly at the 2^63 cap is legal");
    assert_eq!(a.manifest.scratch[1].addr, base);
    assert_eq!(a.manifest.scratch_region, (base, 1u64 << 63));

    // span 24 from the same base crosses the cap — refused at the violating function.
    let call3 = func(0, 6, vec![Inst::new(CALL, 4, 0, 3), Inst::new(HALT, 4, 0, 0)]);
    let m = module_of(vec![probe.clone(), call3]);
    assert_eq!(
        compile_module(&isa, &m, &resolve_aot, &AotOpts::new(base).with_memory64(true))
            .expect_err("past-cap span must refuse"),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::ScratchOutOfAddressSpace {
                scratch_addr: base,
                words: 3
            }
        }
    );

    // The same geometry is FINE for A64 but refuses on A32 (the bound really is addrtype-keyed).
    let m = module_of(vec![probe, call2]);
    assert!(matches!(
        compile_module(&isa, &m, &resolve_aot, &AotOpts::new(base))
            .expect_err("a 2^63-adjacent base is far past the A32 bound"),
        AotError::Func {
            func_idx: 1,
            refusal: Refusal::ScratchOutOfAddressSpace { .. }
        }
    ));
}

/// §6-5: a scratch base with a NON-ZERO upper 32 bits is baked as a full-width, exact
/// `i64.const` (the A64 address constant is not a widened truncation).
#[test]
fn a64_scratch_addr_bakes_the_full_width_i64_const() {
    let isa = common::isa();
    let base: u64 = 0x1_2345_6780; // > 2^32, 8-aligned, upper-32 nonzero
    let m = IrModule {
        funcs: vec![func(
            0,
            5,
            vec![Inst::new(CALL, 3, 0, 2), Inst::new(HALT, 3, 0, 0)],
        )],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let a64 = compile_module(&isa, &m, &resolve_aot, &AotOpts::new(base).with_memory64(true))
        .expect("A64 compiles above 2^32");
    let needle = format!("I64Const {{ value: {} }}", base as i64);
    assert!(
        norm_bodies(&a64.bytes)[0].iter().any(|op| op == &needle),
        "the baked A64 scratch constant must be the exact full-width address"
    );
    // The identical inputs on A32 refuse (the base is past the wasm32 address space).
    assert!(matches!(
        compile_module(&isa, &m, &resolve_aot, &AotOpts::new(base)).expect_err("A32 must refuse"),
        AotError::Func {
            func_idx: 0,
            refusal: Refusal::ScratchOutOfAddressSpace { .. }
        }
    ));
}

/// §2.1 [결정]: shared × memory64 is fail-closed refused in v1 (`SHARED_MEMORY_IMPORT_MAX_PAGES`
/// is a wasm32-only figure — S2-M2 #11), at the input stage, alignment first (order pinned).
#[test]
fn shared_and_memory64_is_refused_fail_closed() {
    let isa = common::isa();
    let m = IrModule {
        funcs: vec![func(0, 1, vec![Inst::new(HALT, 0, 0, 0)])],
        fill: FILL,
        host_ctx: HOST_CTX,
        entry: 0,
    };
    let opts = AotOpts::new(SCRATCH_BASE)
        .with_shared_memory(true)
        .with_memory64(true);
    assert_eq!(
        compile_module(&isa, &m, &resolve_aot, &opts).expect_err("must refuse"),
        AotError::SharedMemory64Unsupported
    );
    // Input-check order: alignment stays first (`ScratchBaseMisaligned`'s "before anything
    // else" contract is untouched by the new check).
    let misaligned = AotOpts::new(4).with_shared_memory(true).with_memory64(true);
    assert_eq!(
        compile_module(&isa, &m, &resolve_aot, &misaligned).expect_err("must refuse"),
        AotError::ScratchBaseMisaligned { base: 4 }
    );
}
