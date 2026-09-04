//! L-5 gate tests (design §7 L-5 row: `check --component` — unit level; the real components,
//! the embed/new chain, the host-lift EXECUTION and the E10/E11 negative probes are
//! `just wasm-aot-component`'s):
//! - green: a hand-built (wasm-encoder `ComponentBuilder`) component with the canonical
//!   `otter-entry` lift — world-shape match, nested-module census, native EH-free validation;
//! - empty-world detection (the E10 trap, machine form): a component with no exports is red
//!   with the pitfall named;
//! - non-component input red (a core module — the §6.2 chain order was not run);
//! - world-shape negatives: wrong export name / lost host import / excess host import
//!   (§5.1 exact-match discipline at the component boundary, both directions);
//! - EH-in-nested-module red (native validation with exceptions disabled rejects it);
//! - L-6 (the L-5 review's carried-over branch): import kind ≠ func under a matching host
//!   name (an instance import named `h-add7`) is red — the name match alone is not enough.

use otter_aot_link::LinkError;
use otter_aot_link::component::check_component;
use otter_aot_link::schema::{ImportEntry, ImportKind, ManifestSidecar, ScratchRegion};

use wasm_encoder::{
    CodeSection, ComponentBuilder, ComponentExportKind, ComponentTypeRef, ComponentValType, ExportKind,
    ExportSection, Function, FunctionSection, InstanceType, Module, PrimitiveValType, TypeSection, ValType,
};

// ---------------------------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------------------------

/// A minimal LINKED-shaped core module: one func of the canonical entry core type
/// `(i32,i32,i64) -> i64`, exported under `export_name` (already kebab in the green shape).
fn core_module(export_name: &str, eh_body: bool) -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I64], [ValType::I64]);
    module.section(&types);
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    module.section(&funcs);
    let mut exports = ExportSection::new();
    exports.export(export_name, ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut f = Function::new([]);
    if eh_body {
        // EH material: `throw_ref` (parse-level; native validation with exceptions OFF
        // rejects it — that rejection IS the machine EH-free gate).
        f.instructions().throw_ref().end();
    } else {
        f.instructions().i64_const(7).end();
    }
    code.function(&f);
    module.section(&code);
    module.finish()
}

/// Build a component embedding `core`: lift its `lift_name` export as the world export
/// `otter-entry`, optionally declaring (unused) component-level func imports `imports`
/// (the lifted-host surface shape — Host1 scalar type `func(x: s64) -> s64`).
fn component(core: &[u8], lift_name: &str, imports: &[&str]) -> Vec<u8> {
    let mut cb = ComponentBuilder::default();
    for name in imports {
        let (host_ty, mut enc) = cb.type_function(None);
        enc.params([("x", ComponentValType::Primitive(PrimitiveValType::S64))])
            .result(Some(ComponentValType::Primitive(PrimitiveValType::S64)));
        cb.import(*name, ComponentTypeRef::Func(host_ty));
    }
    let module_idx = cb.core_module_raw(None, core);
    let instance_idx = cb.core_instantiate(None, module_idx, []);
    let core_func = cb.core_alias_export(None, instance_idx, lift_name, ExportKind::Func);
    let (entry_ty, mut enc) = cb.type_function(None);
    enc.params([
        ("ctx", ComponentValType::Primitive(PrimitiveValType::U32)),
        ("args", ComponentValType::Primitive(PrimitiveValType::U32)),
        ("argc", ComponentValType::Primitive(PrimitiveValType::U64)),
    ])
    .result(Some(ComponentValType::Primitive(PrimitiveValType::S64)));
    let func_idx = cb.lift_func(None, core_func, entry_ty, []);
    cb.export("otter-entry", ComponentExportKind::Func, func_idx, None);
    cb.finish()
}

/// Manifest fixture — `check --component` consults the `imports` host set only.
fn manifest(hosts: &[&str]) -> ManifestSidecar {
    ManifestSidecar {
        entry_slot: 0,
        nfuncs: 1,
        helper_slots: [1, 2, 3],
        host_slots: (0..hosts.len() as u32).map(|i| 4 + i).collect(),
        imports: hosts
            .iter()
            .map(|name| ImportEntry {
                module: "otter_host".to_string(),
                name: (*name).to_string(),
                kind: ImportKind::Host { arity: 1, ctx: false },
            })
            .collect(),
        scratch: vec![],
        scratch_region: ScratchRegion {
            base: 0x11_0000,
            end: 0x11_0008,
        },
        table_size: 4,
        shared_memory: false,
        memory64: false,
        emitter_version: "0.0.0".to_string(),
    }
}

fn violations(bytes: &[u8], man: &ManifestSidecar) -> Vec<String> {
    match check_component(bytes, man) {
        Err(LinkError::Component(v)) => v,
        other => panic!("expected Component violations, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// positives
// ---------------------------------------------------------------------------------------------

#[test]
fn component_green_self_contained() {
    let comp = component(&core_module("otter-entry", false), "otter-entry", &[]);
    let report = check_component(&comp, &manifest(&[])).expect("green");
    assert_eq!(report.exports, ["otter-entry"]);
    assert!(report.imports.is_empty());
    assert_eq!(report.nested_modules, 1);
}

#[test]
fn component_green_host_lifted() {
    // The calc shape: the manifest's otter_host set {h_add7, hc_xor} must appear as the
    // kebab component imports {h-add7, hc-xor} — exact, both directions.
    let comp = component(
        &core_module("otter-entry", false),
        "otter-entry",
        &["h-add7", "hc-xor"],
    );
    let report = check_component(&comp, &manifest(&["h_add7", "hc_xor"])).expect("green");
    assert_eq!(report.exports, ["otter-entry"]);
    assert_eq!(report.imports, ["h-add7", "hc-xor"]);
}

// ---------------------------------------------------------------------------------------------
// negatives
// ---------------------------------------------------------------------------------------------

#[test]
fn component_red_empty_world() {
    // `component new` without `embed --world` (E10): exports nothing. Machine form: a bare
    // component carrying only the embedded core module, no lift, no export.
    let mut cb = ComponentBuilder::default();
    cb.core_module_raw(None, &core_module("otter_entry", false));
    let comp = cb.finish();
    let v = violations(&comp, &manifest(&[]));
    assert!(
        v.iter().any(|m| m.contains("EMPTY WORLD")),
        "empty-world detection must name the E10 pitfall: {v:?}"
    );
}

#[test]
fn component_red_core_module_input() {
    let core = core_module("otter-entry", false);
    let v = violations(&core, &manifest(&[]));
    assert!(
        v.iter().any(|m| m.contains("CORE MODULE")),
        "a core module input must name the chain-order violation: {v:?}"
    );
}

#[test]
fn component_red_wrong_export_name() {
    // Lift under a wrong world export name (the helper always exports `otter-entry`, so
    // build this one by hand): the world-shape match must go red.
    let mut cb = ComponentBuilder::default();
    let module_idx = cb.core_module_raw(None, &core_module("wrong-name", false));
    let instance_idx = cb.core_instantiate(None, module_idx, []);
    let core_func = cb.core_alias_export(None, instance_idx, "wrong-name", ExportKind::Func);
    let (entry_ty, mut enc) = cb.type_function(None);
    enc.params([
        ("ctx", ComponentValType::Primitive(PrimitiveValType::U32)),
        ("args", ComponentValType::Primitive(PrimitiveValType::U32)),
        ("argc", ComponentValType::Primitive(PrimitiveValType::U64)),
    ])
    .result(Some(ComponentValType::Primitive(PrimitiveValType::S64)));
    let func_idx = cb.lift_func(None, core_func, entry_ty, []);
    cb.export("wrong-name", ComponentExportKind::Func, func_idx, None);
    let v = violations(&cb.finish(), &manifest(&[]));
    assert!(
        v.iter().any(|m| m.contains("canonical world surface")),
        "wrong export name must fail the world-shape match: {v:?}"
    );
}

#[test]
fn component_red_lost_host_import() {
    // Manifest declares a host fn, the component lifted nothing — a LOST lift (⊆ rejected).
    let comp = component(&core_module("otter-entry", false), "otter-entry", &[]);
    let v = violations(&comp, &manifest(&["h_add7"]));
    assert!(
        v.iter().any(|m| m.contains("`h-add7` LOST")),
        "a lost host lift must be red: {v:?}"
    );
}

#[test]
fn component_red_excess_host_import() {
    // The component imports a func the manifest never declared — excess is red too.
    let comp = component(&core_module("otter-entry", false), "otter-entry", &["h-add7"]);
    let v = violations(&comp, &manifest(&[]));
    assert!(
        v.iter()
            .any(|m| m.contains("unexpected component import `h-add7`")),
        "an excess component import must be red: {v:?}"
    );
}

#[test]
fn component_red_import_kind_not_func() {
    // L-6 (the L-5 review's carried-over branch): an import whose NAME matches the manifest's
    // otter_host kebab image but whose KIND is not func — here an (empty) instance import
    // named `h-add7`. The name-level match must not be enough; Host1/HostCtx1 shapes are
    // funcs, any other kind is red.
    let mut cb = ComponentBuilder::default();
    let inst_ty = cb.type_instance(None, &InstanceType::new());
    cb.import("h-add7", ComponentTypeRef::Instance(inst_ty));
    let module_idx = cb.core_module_raw(None, &core_module("otter-entry", false));
    let instance_idx = cb.core_instantiate(None, module_idx, []);
    let core_func = cb.core_alias_export(None, instance_idx, "otter-entry", ExportKind::Func);
    let (entry_ty, mut enc) = cb.type_function(None);
    enc.params([
        ("ctx", ComponentValType::Primitive(PrimitiveValType::U32)),
        ("args", ComponentValType::Primitive(PrimitiveValType::U32)),
        ("argc", ComponentValType::Primitive(PrimitiveValType::U64)),
    ])
    .result(Some(ComponentValType::Primitive(PrimitiveValType::S64)));
    let func_idx = cb.lift_func(None, core_func, entry_ty, []);
    cb.export("otter-entry", ComponentExportKind::Func, func_idx, None);
    let v = violations(&cb.finish(), &manifest(&["h_add7"]));
    assert!(
        v.iter().any(|m| m.contains("has kind instance, expected func")),
        "a non-func import kind under a matching host name must be red: {v:?}"
    );
}

#[test]
fn component_red_eh_in_nested_module() {
    // throw_ref in the embedded core module: the native validator (exceptions disabled)
    // rejects the whole component — the machine EH-free gate's rejection direction at the
    // unit level (the recipe-level mutation is §5.6-⑧, L-6).
    let comp = component(&core_module("otter-entry", true), "otter-entry", &[]);
    let v = violations(&comp, &manifest(&[]));
    assert!(
        v.iter().any(|m| m.contains("native validation")),
        "EH in the nested module must fail EH-free validation: {v:?}"
    );
}
