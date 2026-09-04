//! Refusal-path tests (the complete §4.6 precheck — since W-2 there is no lowering boundary left:
//! every `Effect`/`Expr` variant compiles) and structural assertions on the emitted bytes (design
//! §2.2/§5.2 norms): section set, import shapes, export type, and the **absence of a start
//! section** — checked by actually parsing the bytes with `wasmparser`, plus an independent full
//! validation pass (no wasmtime involved).

mod common;

use common::{
    CALL, DEC_JNZ, HALT, HOST1, HOSTCTX2, JUMP, JUMP_IF, MOV_IMM, TAILCALL, bin_opcode, conv_opcode, func,
    neg_opcode,
};
use otter::validate::ValidationError;
use otter::{ArithOp, BinOp, Effect, Expr, Inst, Isa, NumTy, OpDef, OpHints};
use otter_wasmemit::{HelperIdx, Refusal, compile_dispatch_fn};
use wasmparser::{Parser, Payload, TypeRef, ValType, Validator};

/// Arbitrary (but valid) helper slots — refusal tests never reach instantiation.
fn helpers() -> HelperIdx {
    HelperIdx {
        dispatch: 1,
        tail: 2,
        binop: 3,
        convert: 4,
        unop: 5,
    }
}

/// A resolver that maps every host fn to slot 9 (tests that need `None` pass their own).
fn resolve_all(_p: usize) -> Option<u32> {
    Some(9)
}

/// The `scratch_addr` these tests pass — an arbitrary reserved word: refusal tests never emit,
/// and the compiled-path tests here only validate bytes (never instantiate/run), so the address
/// is never dereferenced. Call-free functions never even bake it (`compile_dispatch_fn`'s doc).
const SCRATCH: u32 = 0x1000;

fn compile(isa: &Isa, f: &otter::FuncDef) -> Result<Vec<u8>, Refusal> {
    compile_dispatch_fn(isa, 0, f, 0, 0, helpers(), &resolve_all, SCRATCH)
}

// --- Refusal cases -------------------------------------------------------------------------------

/// §4.6-3: an integer `Div` at any integer NumTy is refused BEFORE lowering (the helper would
/// panic → nounwind abort → uncatchable wasm trap). Float `Div` is fine.
#[test]
fn integer_div_is_refused() {
    let isa = common::isa();
    let div_idx = 3; // ALL_ARITH[3] == Div
    assert_eq!(common::ALL_ARITH[div_idx], ArithOp::Div);
    for (ti, ty) in common::ALL_TYS.iter().enumerate() {
        let f = func(
            2,
            3,
            vec![
                Inst::new(bin_opcode(ti, div_idx), 2, 0, 1),
                Inst::new(HALT, 2, 0, 0),
            ],
        );
        let r = compile(&isa, &f);
        if ty.is_float() {
            assert!(r.is_ok(), "float Div must lower ({ty:?})");
        } else {
            assert_eq!(
                r,
                Err(Refusal::IntegerDiv { pc: 0 }),
                "integer Div must refuse ({ty:?})"
            );
        }
    }
}

/// The §4.6-3 walk is DEEP: an integer Div nested inside another expression is still found.
#[test]
fn nested_integer_div_is_refused() {
    let mut isa = common::isa();
    let nested = OpDef {
        name: "nested_div",
        effects: vec![Effect::SetLocalAt {
            reg: 0,
            value: Expr::Bin(
                BinOp::new(NumTy::F64, ArithOp::Add),
                Box::new(Expr::LocalAt(1)),
                Box::new(Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Div),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::LocalAt(2)),
                )),
            ),
        }],
        hints: OpHints {
            pure: true,
            terminator: false,
        },
    };
    let opcode = isa.ops.len() as u16;
    isa.ops.push(nested);
    let f = func(2, 3, vec![Inst::new(opcode, 2, 0, 1), Inst::new(HALT, 2, 0, 0)]);
    assert_eq!(compile(&isa, &f), Err(Refusal::IntegerDiv { pc: 0 }));
}

/// The W-2 boundary flip (mirroring how W-1b flipped the fall-through tests): `Call`/`TailCall`
/// now COMPILE — `UnsupportedEffect` is no longer returned for any variant, and the emitted bytes
/// validate. Behavior (against the run_module oracle, through the engine-mirror trampoline) is
/// `tests/calls.rs`'s job; this pins the boundary itself. Also pins §2.3 #7-#8: dispatch and
/// tail_signal share ONE structural type `(i32,i64,i32,i64)->i64`, interned exactly once even
/// when a function contains both.
#[test]
fn call_and_tailcall_now_compile_and_validate() {
    let isa = common::isa();
    // r0 = callee 0, r1 = arg; Call dest=r2 base=r0(argc=1), then TailCall base=r0 — both in one
    // function, straight-line (no jumps ⇒ the W-1a path lowers them too, not just the dispatcher).
    let f = func(
        0,
        3,
        vec![
            Inst::new(MOV_IMM, 0, 0, 0),
            Inst::new(MOV_IMM, 1, 5, 0),
            Inst::new(CALL, 2, 0, 1),
            Inst::new(TAILCALL, 0, 1, 0),
        ],
    );
    let bytes = compile(&isa, &f).expect("Call/TailCall must compile since W-2");
    Validator::new().validate_all(&bytes).expect("validate");
    let mut dispatch_ty_count = 0;
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::TypeSection(r) = payload.expect("parse") {
            for group in r {
                for ty in group.expect("type").into_types() {
                    let fty = ty.composite_type.unwrap_func();
                    if fty.params() == [ValType::I32, ValType::I64, ValType::I32, ValType::I64]
                        && fty.results() == [ValType::I64]
                    {
                        dispatch_ty_count += 1;
                    }
                }
            }
        }
    }
    assert_eq!(
        dispatch_ty_count, 1,
        "dispatch + tail_signal share one structural type (§2.3 #7-#8), interned once"
    );

    // The dispatcher path lowers them too: the same call pair under a real loop.
    let looping = func(
        1,
        3,
        vec![
            Inst::new(MOV_IMM, 0, 0, 0),
            Inst::new(MOV_IMM, 1, 5, 0),
            Inst::new(CALL, 2, 0, 1),
            Inst::new(DEC_JNZ, 2, 2, 0),
            Inst::new(TAILCALL, 0, 1, 0),
        ],
    );
    let bytes = compile(&isa, &looping).expect("dispatcher-path Call/TailCall must compile");
    Validator::new().validate_all(&bytes).expect("validate");
}

/// A host fn the resolver does not know has no table index to bake: refused, not guessed.
#[test]
fn unresolved_host_fn_is_refused() {
    let isa = common::isa();
    let f = func(1, 2, vec![Inst::new(HOST1, 1, 0, 0), Inst::new(HALT, 1, 0, 0)]);
    let none = |_: usize| -> Option<u32> { None };
    assert_eq!(
        compile_dispatch_fn(&isa, 0, &f, 0, 0, helpers(), &none, SCRATCH),
        Err(Refusal::HostFnUnresolved { pc: 0 })
    );
    // ... and a resolver that knows the fn compiles the same function fine.
    assert!(compile(&isa, &f).is_ok());
}

/// `otter::validate::check_func` runs FIRST (§8d): invalid bytecode is `ValidationFailed`, even
/// when it also contains shapes the precheck would refuse.
#[test]
fn validation_failure_is_wrapped_and_takes_precedence() {
    let isa = common::isa();

    let empty = func(0, 1, vec![]);
    assert_eq!(
        compile(&isa, &empty),
        Err(Refusal::ValidationFailed(ValidationError::EmptyCode))
    );

    let oob = func(0, 1, vec![Inst::new(MOV_IMM, 99, 1, 0), Inst::new(HALT, 0, 0, 0)]);
    assert!(matches!(
        compile(&isa, &oob),
        Err(Refusal::ValidationFailed(ValidationError::LocalOutOfRange {
            pc: 0,
            local: 99,
            ..
        }))
    ));

    // out-of-range local AND a Call: validation wins (it runs before the precheck).
    let both = func(0, 1, vec![Inst::new(MOV_IMM, 99, 1, 0), Inst::new(CALL, 0, 0, 0)]);
    assert!(matches!(compile(&isa, &both), Err(Refusal::ValidationFailed(_))));
}

// --- structural assertions on the emitted bytes --------------------------------------------------

/// A representative function whose lowering exercises every W-1a call_indirect type (binop,
/// convert, unop, host1, hostctx2) so the type section is maximally populated.
fn representative() -> (Isa, otter::FuncDef) {
    let isa = common::isa();
    let f = func(
        2,
        4,
        vec![
            Inst::new(bin_opcode(8, 0), 2, 0, 1),   // f16 add -> binop helper
            Inst::new(neg_opcode(3), 2, 2, 0),      // i64 neg -> unop helper
            Inst::new(conv_opcode(3, 10), 3, 2, 0), // i64->f64 -> convert helper
            Inst::new(HOST1, 3, 3, 0),
            Inst::new(HOSTCTX2, 3, 3, 2),
            Inst::new(HALT, 3, 0, 0),
        ],
    );
    (isa, f)
}

/// §2.2/§5.2 norms, asserted on the parsed bytes: sections are EXACTLY type/import/function/
/// export/code (in that order) — **no start section**, no custom/memory/table/global/element/data
/// sections; imports are exactly otter.memory (min 0, no max, 32-bit, unshared) + otter.table
/// (funcref, min 0, no max); the sole export is "f", a func of type (i32,i32,i64)->i64.
#[test]
fn emitted_module_structure_matches_the_contract() {
    let (isa, f) = representative();
    let bytes = compile(&isa, &f).expect("representative function must compile");

    let mut sections: Vec<&'static str> = Vec::new();
    let mut func_type_indices: Vec<u32> = Vec::new();
    let mut types = Vec::new();
    for payload in Parser::new(0).parse_all(&bytes) {
        match payload.expect("emitted bytes must parse") {
            Payload::Version { .. } | Payload::End(_) => {}
            Payload::TypeSection(r) => {
                sections.push("type");
                for group in r {
                    for ty in group.expect("type").into_types() {
                        types.push(ty);
                    }
                }
            }
            Payload::ImportSection(r) => {
                sections.push("import");
                let imports: Vec<_> = r.into_imports().map(|i| i.expect("import")).collect();
                assert_eq!(imports.len(), 2, "exactly two imports (§5.2)");
                assert_eq!((imports[0].module, imports[0].name), ("otter", "memory"));
                match imports[0].ty {
                    TypeRef::Memory(m) => {
                        assert_eq!(m.initial, 0, "memory min 0 links with any current size");
                        assert_eq!(m.maximum, None);
                        // O-19 §6-3: these `!memory64`/`!table64` pins are the A32 ARM of the
                        // addrtype-conditional contract — this JIT surface is A32-only by
                        // design (§2.1), so its arm is unconditional here; the A64 (mixed
                        // memory64∧!table64) arm is pinned on the AOT surface in
                        // tests/addrtype.rs (`a64_structure_is_the_mixed_shape`).
                        assert!(!m.memory64 && !m.shared);
                    }
                    ref other => panic!("otter.memory must be a memory import, got {other:?}"),
                }
                assert_eq!((imports[1].module, imports[1].name), ("otter", "table"));
                match imports[1].ty {
                    TypeRef::Table(t) => {
                        assert!(t.element_type.is_func_ref(), "funcref table");
                        assert_eq!(t.initial, 0);
                        assert_eq!(t.maximum, None, "no max: links growable AND fixed tables");
                        assert!(!t.table64, "32-bit table (§13-3: wasm32 only)");
                        assert!(!t.shared, "unshared (§5.2 threading norm)");
                    }
                    ref other => panic!("otter.table must be a table import, got {other:?}"),
                }
            }
            Payload::FunctionSection(r) => {
                sections.push("function");
                func_type_indices = r.into_iter().map(|i| i.expect("func")).collect();
            }
            Payload::ExportSection(r) => {
                sections.push("export");
                let exports: Vec<_> = r.into_iter().map(|e| e.expect("export")).collect();
                assert_eq!(exports.len(), 1, "exactly one export");
                assert_eq!(exports[0].name, "f");
                assert_eq!(exports[0].kind, wasmparser::ExternalKind::Func);
                assert_eq!(exports[0].index, 0, "no fn imports ⇒ our function is index 0");
            }
            Payload::CodeSectionStart { count, .. } => {
                sections.push("code");
                assert_eq!(count, 1, "exactly one function body");
            }
            Payload::CodeSectionEntry(_) => {}
            // The norm this arm enforces is load-bearing: a start section would run guest code at
            // instantiation time (§5.2 — the backend must make this structurally impossible).
            Payload::StartSection { .. } => panic!("emitted module must NEVER have a start section"),
            other => panic!("unexpected section in emitted module: {other:?}"),
        }
    }
    assert_eq!(
        sections,
        ["type", "import", "function", "export", "code"],
        "§2.2 section set, in order"
    );

    // Export "f" has the CompiledFn wasm type (i32, i32, i64) -> i64 (§2.2).
    assert_eq!(func_type_indices.len(), 1);
    let fty = types[func_type_indices[0] as usize].composite_type.unwrap_func();
    assert_eq!(fty.params(), &[ValType::I32, ValType::I32, ValType::I64]);
    assert_eq!(fty.results(), &[ValType::I64]);

    // Independent full validation (wasmparser's validator — no wasmtime in this test binary).
    Validator::new()
        .validate_all(&bytes)
        .expect("emitted module must validate");
}

/// O-17 (§5.2 v1.1): with `EmitOpts::shared_memory`, the ONLY change to the module is the memory
/// import's declared type — `(memory 0 65536 shared)` (min 0, max = the full wasm32 address
/// space, shared flag set: the universal supertype of every wasm32 shared memory, spike M5) —
/// while the table import, section set, export and code stay exactly the §2.2 shape, and the
/// bytes still fully validate (wasmparser's default features include `threads`, 0.258 measured).
/// The wasmtime differential harness keeps running the NON-shared shape only (its mock memory is
/// non-shared); this is the emission-shape unit gate.
#[test]
fn shared_memory_opt_changes_only_the_memory_import_shape() {
    use otter_wasmemit::{EmitOpts, SHARED_MEMORY_IMPORT_MAX_PAGES, compile_dispatch_fn_with};

    let (isa, f) = representative();
    let shared = compile_dispatch_fn_with(
        &isa,
        0,
        &f,
        0,
        0,
        helpers(),
        &resolve_all,
        u64::from(SCRATCH),
        EmitOpts::new().with_shared_memory(true),
    )
    .expect("shared-shape emission must compile");

    let mut sections: Vec<&'static str> = Vec::new();
    for payload in Parser::new(0).parse_all(&shared) {
        match payload.expect("emitted bytes must parse") {
            Payload::Version { .. } | Payload::End(_) => {}
            Payload::TypeSection(_) => sections.push("type"),
            Payload::ImportSection(r) => {
                sections.push("import");
                let imports: Vec<_> = r.into_imports().map(|i| i.expect("import")).collect();
                assert_eq!(imports.len(), 2, "still exactly two imports (§5.2)");
                assert_eq!((imports[0].module, imports[0].name), ("otter", "memory"));
                match imports[0].ty {
                    TypeRef::Memory(m) => {
                        assert!(m.shared, "O-17: the shared flag IS the change");
                        assert_eq!(m.initial, 0, "min 0 ⇒ links against any current size");
                        assert_eq!(
                            m.maximum,
                            Some(SHARED_MEMORY_IMPORT_MAX_PAGES),
                            "shared memories must declare a max; 65536 ≥ every wasm32 max (M5)"
                        );
                        // O-19 §6-3: the A32 arm of the addrtype-conditional pin (see the
                        // non-shared structure test's note; A64 lives in tests/addrtype.rs).
                        assert!(!m.memory64);
                    }
                    ref other => panic!("otter.memory must be a memory import, got {other:?}"),
                }
                // The table import is NOT shared-parameterized — unchanged §5.2 shape.
                assert_eq!((imports[1].module, imports[1].name), ("otter", "table"));
                match imports[1].ty {
                    TypeRef::Table(t) => {
                        assert!(t.element_type.is_func_ref());
                        assert_eq!((t.initial, t.maximum), (0, None));
                        assert!(!t.shared, "table sharing is NOT part of O-17 L0");
                    }
                    ref other => panic!("otter.table must be a table import, got {other:?}"),
                }
            }
            Payload::FunctionSection(_) => sections.push("function"),
            Payload::ExportSection(_) => sections.push("export"),
            Payload::CodeSectionStart { .. } => sections.push("code"),
            Payload::CodeSectionEntry(_) => {}
            Payload::StartSection { .. } => panic!("emitted module must NEVER have a start section"),
            other => panic!("unexpected section in emitted module: {other:?}"),
        }
    }
    assert_eq!(
        sections,
        ["type", "import", "function", "export", "code"],
        "§2.2 section set unchanged by the shared shape"
    );
    Validator::new()
        .validate_all(&shared)
        .expect("shared-shape module must validate (wasmparser default features include threads)");

    // Everything OUTSIDE the import section is bit-identical to the non-shared emission: the
    // sharedness switch changes the memory import's declared type and nothing else.
    let unshared = compile(&isa, &f).expect("non-shared emission");
    assert_ne!(
        shared, unshared,
        "the two shapes must differ (in the import section)"
    );
    let strip_imports = |bytes: &[u8]| -> Vec<u8> {
        // Rebuild the byte stream minus section id 2 (import) — LEB-walk the section framing.
        let mut out = Vec::new();
        let mut p = 8usize; // magic + version
        out.extend_from_slice(&bytes[..8]);
        while p < bytes.len() {
            let start = p;
            let id = bytes[p];
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
            if id != 2 {
                out.extend_from_slice(&bytes[start..p]);
            }
        }
        out
    };
    assert_eq!(
        strip_imports(&shared),
        strip_imports(&unshared),
        "only the import section may differ between the two shapes"
    );
}

/// O-17 no-regression pin: `compile_dispatch_fn` and `compile_dispatch_fn_with(EmitOpts::new())`
/// (= `EmitOpts::default()`, non-shared) are byte-for-byte identical — the legacy entry point and
/// the default of the new one can never drift apart.
#[test]
fn default_opts_are_byte_identical_to_the_legacy_entry_point() {
    use otter_wasmemit::{EmitOpts, compile_dispatch_fn_with};

    let (isa, f) = representative();
    let legacy = compile(&isa, &f).expect("legacy compile");
    for opts in [EmitOpts::new(), EmitOpts::default()] {
        let via_opts = compile_dispatch_fn_with(
            &isa,
            0,
            &f,
            0,
            0,
            helpers(),
            &resolve_all,
            u64::from(SCRATCH),
            opts,
        )
        .expect("opts compile");
        assert_eq!(legacy, via_opts, "non-shared default must not drift ({opts:?})");
    }
}

/// The dispatcher-loop path (a function WITH jumps — design §3) emits the same §2.2/§5.2 module
/// shape as the straight-line path: identical section set in order, no start section, and the
/// bytes pass full validation — including the degenerate single-leader self-loop, where the
/// `br_table` has exactly one target.
#[test]
fn dispatcher_module_structure_matches_the_contract_too() {
    let isa = common::isa();
    // A real loop: dec_jnz self-loop then halt (two leaders + backward edge)…
    let looping = func(
        1,
        2,
        vec![
            Inst::new(MOV_IMM, 1, 9, 0),
            Inst::new(DEC_JNZ, 0, 1, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    );
    // …and the single-leader shape: `JumpTo 0` as the only instruction (pc 0 is the only leader).
    let self_loop = func(0, 1, vec![Inst::new(JUMP, 0, 0, 0)]);
    for f in [&looping, &self_loop] {
        let bytes = compile(&isa, f).expect("jumpy function must compile since W-1b");
        let mut saw_start = false;
        let mut names = Vec::new();
        for payload in Parser::new(0).parse_all(&bytes) {
            match payload.expect("parse") {
                Payload::StartSection { .. } => saw_start = true,
                Payload::TypeSection(_) => names.push("type"),
                Payload::ImportSection(_) => names.push("import"),
                Payload::FunctionSection(_) => names.push("function"),
                Payload::ExportSection(_) => names.push("export"),
                Payload::CodeSectionStart { .. } => names.push("code"),
                _ => {}
            }
        }
        assert!(!saw_start, "no start section, ever (§5.2)");
        assert_eq!(names, ["type", "import", "function", "export", "code"]);
        Validator::new()
            .validate_all(&bytes)
            .expect("dispatcher module must validate");
    }
}

/// Structure holds for degenerate shapes too: no params, no consts, single-return body.
#[test]
fn minimal_function_emits_the_same_shape_and_validates() {
    let isa = common::isa();
    let f = func(0, 1, vec![Inst::new(HALT, 0, 0, 0)]);
    let bytes = compile(&isa, &f).expect("compile");
    let mut saw_start = false;
    let mut names = Vec::new();
    for payload in Parser::new(0).parse_all(&bytes) {
        match payload.expect("parse") {
            Payload::StartSection { .. } => saw_start = true,
            Payload::TypeSection(_) => names.push("type"),
            Payload::ImportSection(_) => names.push("import"),
            Payload::FunctionSection(_) => names.push("function"),
            Payload::ExportSection(_) => names.push("export"),
            Payload::CodeSectionStart { .. } => names.push("code"),
            _ => {}
        }
    }
    assert!(!saw_start, "no start section, ever");
    assert_eq!(names, ["type", "import", "function", "export", "code"]);
    Validator::new().validate_all(&bytes).expect("validate");
}

/// §4.6-1, FLIPPED as the W-1a version of this test predicted: an end-fall-through function is
/// now REFUSED outright — including the `nlocals == 0` shape whose W-1a safety net used to
/// mis-emit `local.get 3` (panel #2a, lenses A#1/B#1). The net itself stays in the code,
/// defensively, but is no longer reachable.
#[test]
fn fall_through_with_zero_locals_is_refused() {
    // A locals-free, terminator-free function: one nop opcode (no effects). Passes `check_func`
    // (nothing to range-check); §4.6-1 now refuses it before lowering.
    let nop_isa = Isa {
        ops: vec![OpDef {
            name: "nop",
            effects: vec![],
            hints: OpHints {
                pure: true,
                terminator: false,
            },
        }],
    };
    let f = func(0, 0, vec![Inst::new(0, 0, 0, 0)]);
    assert_eq!(compile(&nop_isa, &f), Err(Refusal::FallsOffEnd { last_pc: 0 }));
}

/// The `nlocals >= 1` end-fall-through shape (W-1a: the `return local[0]` net) — also flipped to
/// a §4.6-1 refusal, and the reported pc is the LAST instruction's.
#[test]
fn fall_through_with_locals_is_refused() {
    let isa = common::isa();
    // Sets local 0 but never terminates — used to fall off the end into the safety net.
    let f = func(0, 3, vec![Inst::new(MOV_IMM, 0, 42, 0)]);
    assert_eq!(compile(&isa, &f), Err(Refusal::FallsOffEnd { last_pc: 0 }));
    let f2 = func(
        0,
        3,
        vec![Inst::new(MOV_IMM, 0, 42, 0), Inst::new(MOV_IMM, 1, 7, 0)],
    );
    assert_eq!(compile(&isa, &f2), Err(Refusal::FallsOffEnd { last_pc: 1 }));
}

/// §4.6-2: a `JumpIf` whose not-taken edge is `pc + 1 == code.len()` falls off the end — refused.
/// The same `JumpIf` with one more instruction after it compiles (and validates).
#[test]
fn trailing_jump_if_is_refused() {
    let isa = common::isa();
    let f = func(
        1,
        2,
        vec![Inst::new(MOV_IMM, 1, 3, 0), Inst::new(JUMP_IF, 0, 0, 0)],
    );
    assert_eq!(compile(&isa, &f), Err(Refusal::TrailingJumpIf { pc: 1 }));

    let ok = func(
        1,
        2,
        vec![
            Inst::new(MOV_IMM, 1, 3, 0),
            Inst::new(JUMP_IF, 0, 0, 0),
            Inst::new(HALT, 1, 0, 0),
        ],
    );
    let bytes = compile(&isa, &ok).expect("non-trailing JumpIf must compile");
    Validator::new().validate_all(&bytes).expect("validate");
}

/// §4.6-4: a control effect anywhere but LAST in its `OpDef` is refused, for all five control
/// variants (`TailCall`'s position rule fires before its W-2 `UnsupportedEffect`, so W-2 only has
/// to delete the unsupported arm). The legal multi-effect shape — non-control first, control last
/// (`dec_jnz`) — compiles.
#[test]
fn control_effect_not_last_is_refused() {
    let controls: [(&str, Effect); 5] = [
        ("JumpTo", Effect::JumpTo { target: 1 }),
        (
            "JumpIf",
            Effect::JumpIf {
                cond: Expr::LocalAt(0),
                target: 1,
            },
        ),
        (
            "Halt",
            Effect::Halt {
                value: Expr::LocalAt(0),
            },
        ),
        (
            "Return",
            Effect::Return {
                value: Expr::LocalAt(0),
            },
        ),
        ("TailCall", Effect::TailCall { base: 0, argc: 2 }),
    ];
    for (name, control) in controls {
        let mut isa = common::isa();
        let opcode = isa.ops.len() as u16;
        isa.ops.push(OpDef {
            name: "control_then_write",
            effects: vec![
                control,
                Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::Imm(1),
                },
            ],
            hints: OpHints {
                pure: false,
                terminator: true,
            },
        });
        let f = func(1, 3, vec![Inst::new(opcode, 0, 1, 0), Inst::new(HALT, 0, 0, 0)]);
        assert_eq!(
            compile(&isa, &f),
            Err(Refusal::ControlEffectNotLast { pc: 0, effect: name }),
            "{name}"
        );
    }

    // dec_jnz — [SetLocalAt, JumpIf] with the control effect LAST — is the legal shape.
    let isa = common::isa();
    let ok = func(1, 2, vec![Inst::new(DEC_JNZ, 0, 0, 0), Inst::new(HALT, 0, 0, 0)]);
    let bytes = compile(&isa, &ok).expect("control-last multi-effect op must compile");
    Validator::new().validate_all(&bytes).expect("validate");
}

/// §4.6-5 (the W-1a "known implementation-limit gap", closed): 3 params + `$state` + `nlocals`
/// must fit the engines' 50,000-locals cap. The bound is uniform (+4 for every function), so it
/// is EXACT on the dispatcher path and one slot conservative on the straight path (panel #2b
/// lens C#2) — both at-cap twins compile AND validate ("Ok ⇒ validating module" holds), and the
/// dispatcher twin really declares exactly 50,000 locals (the panel-probed shape).
#[test]
fn wasm_locals_limit_is_refused_at_the_boundary() {
    let isa = common::isa();
    let cap = otter_wasmemit::MAX_WASM_FUNCTION_LOCALS;
    let max_ok_nlocals = cap - 4; // 3 CompiledFn params + the dispatcher's $state

    let over = func(0, max_ok_nlocals + 1, vec![Inst::new(HALT, 0, 0, 0)]);
    assert_eq!(
        compile(&isa, &over),
        Err(Refusal::WasmLocalsLimitExceeded {
            wasm_locals: cap + 1,
            limit: cap,
        })
    );

    // Straight-line at-cap twin: no $state is emitted, so this actually declares cap - 1 locals
    // (the uniform accounting refuses straight-line functions one slot early — conservative,
    // never the reverse; see `Refusal::WasmLocalsLimitExceeded`'s doc).
    let at = func(0, max_ok_nlocals, vec![Inst::new(HALT, 0, 0, 0)]);
    let bytes = compile(&isa, &at).expect("at-cap function must compile");
    Validator::new()
        .validate_all(&bytes)
        .expect("at-cap module must validate (this is exactly the gap W-1a documented)");

    // Dispatcher at-cap twin (panel #2b lens C#2): same nlocals plus a jump ⇒ the $state i32 IS
    // emitted and the function declares exactly `cap` = 50,000 locals, params included — the
    // uniform bound is exact here, and the bytes still compile and validate.
    let at_disp = func(
        0,
        max_ok_nlocals,
        vec![
            Inst::new(MOV_IMM, 0, 1, 0),
            Inst::new(DEC_JNZ, 0, 1, 0), // jump ⇒ dispatcher path ⇒ $state declared
            Inst::new(HALT, 0, 0, 0),
        ],
    );
    let bytes = compile(&isa, &at_disp).expect("dispatcher at-cap function must compile");
    let mut declared = 0usize;
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("parse") {
            for group in body.get_locals_reader().expect("locals") {
                let (count, _ty) = group.expect("local group");
                declared += count as usize;
            }
        }
    }
    assert_eq!(
        declared + 3, // + the 3 CompiledFn params (validators count them toward the cap)
        cap,
        "dispatcher at-cap twin must declare exactly the engine cap"
    );
    Validator::new()
        .validate_all(&bytes)
        .expect("exactly-at-cap dispatcher module must validate");
}

/// The precheck order is deterministic: `check_func` first (see
/// `validation_failure_is_wrapped_and_takes_precedence`), then the function-shaped §4.6 checks
/// (locals cap, then end fall-through), then the per-pc walk (integer Div etc.).
#[test]
fn precheck_shape_checks_precede_the_walk() {
    let isa = common::isa();
    // Integer Div (per-pc walk) AND falls off the end (shape check) → FallsOffEnd wins.
    let div_idx = 3;
    let f = func(2, 3, vec![Inst::new(bin_opcode(3, div_idx), 2, 0, 1)]);
    assert_eq!(compile(&isa, &f), Err(Refusal::FallsOffEnd { last_pc: 0 }));
    // Over the locals cap AND falls off the end → the locals cap wins.
    let f2 = func(
        0,
        otter_wasmemit::MAX_WASM_FUNCTION_LOCALS,
        vec![Inst::new(MOV_IMM, 0, 1, 0)],
    );
    assert!(matches!(
        compile(&isa, &f2),
        Err(Refusal::WasmLocalsLimitExceeded { .. })
    ));
}

/// The `Un(Neg)` lowering really goes through `otter_unop` and `Un(Convert)` through
/// `otter_convert` — visible as the shared (i64,i32,i32)->i64 helper type being interned exactly
/// once (structural type dedup; the §2.3 table's type column is 1:1 in the output).
#[test]
fn helper_types_are_deduplicated() {
    let (isa, f) = representative();
    let bytes = compile(&isa, &f).expect("compile");
    let mut count_unary_helper_ty = 0;
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::TypeSection(r) = payload.expect("parse") {
            for group in r {
                for ty in group.expect("type").into_types() {
                    let fty = ty.composite_type.unwrap_func();
                    if fty.params() == [ValType::I64, ValType::I32, ValType::I32]
                        && fty.results() == [ValType::I64]
                    {
                        count_unary_helper_ty += 1;
                    }
                }
            }
        }
    }
    assert_eq!(
        count_unary_helper_ty, 1,
        "convert + unop share one structural type, interned once"
    );
}

/// Panel #3 lens A finding P2: a scratch span that would extend past the wasm32 address space is
/// a statically detectable always-trap shape — refused, not emitted. The span ending exactly AT
/// `2^32` is legal (`>` boundary) and must compile AND validate.
#[test]
fn scratch_past_address_space_is_refused_at_the_exact_boundary() {
    let isa = common::isa();
    // main calls with argc = 3: marshaling span = 24 bytes.
    let f = func(
        1,
        6,
        vec![
            Inst::new(CALL, 5, 1, 3), // dest=5 base=1 argc=3 (block 1..=4 within nlocals=6)
            Inst::new(HALT, 5, 0, 0),
        ],
    );
    let over = u32::MAX - 22; // span end = 2^32 + 1 > 2^32 → refuse
    assert_eq!(
        compile_dispatch_fn(&isa, 0, &f, 0, 0, helpers(), &resolve_all, over),
        Err(Refusal::ScratchOutOfAddressSpace {
            scratch_addr: u64::from(over),
            words: 3,
        })
    );
    let at = u32::MAX - 23; // span end = exactly 2^32 → legal
    let bytes = compile_dispatch_fn(&isa, 0, &f, 0, 0, helpers(), &resolve_all, at)
        .expect("span ending exactly at 2^32 is legal");
    Validator::new().validate_all(&bytes).expect("validates");
}

/// O-19 C1 (u64 widening regression): the opts-route surface now accepts a full-u64
/// `scratch_addr`, so the §4.6 span check must be `checked_add`-total — an address whose span
/// would WRAP u64 is the same statically-detectable always-trap shape and must refuse (before
/// C1 the u32 parameter made this shape unrepresentable; after it, an unchecked `+` would wrap
/// and silently ACCEPT). The surface stays A32 (§2.1), so the bound itself is still `2^32`.
#[test]
fn u64_scratch_addr_overflow_refuses_instead_of_wrapping() {
    use otter_wasmemit::{EmitOpts, compile_dispatch_fn_with};

    let isa = common::isa();
    let f = func(1, 6, vec![Inst::new(CALL, 5, 1, 3), Inst::new(HALT, 5, 0, 0)]);
    for addr in [u64::MAX, u64::MAX - 23, 1u64 << 63] {
        let r = compile_dispatch_fn_with(&isa, 0, &f, 0, 0, helpers(), &resolve_all, addr, EmitOpts::new());
        assert_eq!(
            r,
            Err(Refusal::ScratchOutOfAddressSpace {
                scratch_addr: addr,
                words: 3
            }),
            "a u64 scratch_addr past (or wrapping) the A32 bound must refuse ({addr:#x})"
        );
    }
}

/// Panel #3 lens A finding P1: `scratch_words` mirrors the §4.6-5 locals cap, so a function
/// `compile_dispatch_fn` is certain to refuse can never first report a huge scratch size to an
/// allocate-then-compile caller.
#[test]
fn scratch_words_mirrors_the_locals_cap() {
    let isa = common::isa();
    let f = func(0, 60_000, vec![Inst::new(HALT, 0, 0, 0)]);
    assert!(matches!(
        otter_wasmemit::scratch_words(&isa, &f),
        Err(Refusal::WasmLocalsLimitExceeded { .. })
    ));
}
