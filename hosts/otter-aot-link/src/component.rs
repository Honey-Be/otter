//! `check --component` — the §6.2 NATIVE component checks (L-5 increment).
//!
//! Implementation rule (§6.2, panel T-S4): wasmparser `component-model` NATIVE parsing, never
//! a `wasm-tools component wit` text-dump comparison — CLI output is human-facing and a pin
//! bump could silently vacuate a grep-style gate (the exact class the repo's mutations block).
//!
//! Checks, item-for-item:
//! - **native validation, EH-free features**: the wasmparser `Validator` runs with
//!   `WasmFeatures::all()` minus `EXCEPTIONS`/`LEGACY_EXCEPTIONS` (the recipe's
//!   `--features=all,-exceptions,-legacy-exceptions` line, native form) — component-model
//!   stays enabled, so this validates the whole component INCLUDING nested core modules with
//!   EH structurally rejected;
//! - **empty-world detection** (SPK L5/§4-9, E10 trap): a component whose export set is empty
//!   means `component new` ran without `embed --world` — core exports are NOT auto-promoted;
//!   loud red, never a silent pass;
//! - **world-shape match**: outer-component exports must be EXACTLY `otter-entry` (func), and
//!   outer-component imports must EQUAL the kebab conversion of the manifest's `otter_host`
//!   set (names, both directions, all funcs) — the §5.1 exact-match discipline (`⊆` rejected,
//!   panel G-SF-3) carried to the component boundary. A host-free product naturally expects
//!   the empty import set;
//! - **EH census over nested core modules** (belt-and-suspenders parity with `check
//!   --linked`): every depth-0 `ModuleSection`'s code is scanned with the same operator set
//!   ([`crate::linked::is_eh_op`]) plus the tag-section census.
//!
//! Depth discipline: `Parser::parse_all` descends into nested modules/components; only
//! depth-0 (outer component) import/export sections define the world surface. Nested
//! sections are structural payload (the embedded core module), not surface.

use wasmparser::{Encoding, Parser, Payload, Validator, WasmFeatures};

use crate::linked::is_eh_op;
use crate::schema::{ImportKind, ManifestSidecar};
use crate::{LinkError, is_valid_wit_ident, kebab};

/// Green-path summary of a passed `check --component`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentReport {
    /// Outer-component export names (world surface), section order.
    pub exports: Vec<String>,
    /// Outer-component import names (lifted host set), section order.
    pub imports: Vec<String>,
    /// Number of nested core modules EH-censused.
    pub nested_modules: usize,
}

/// The expected world export surface (§6.1 canonical world — functions only, entry alone).
const WORLD_EXPORT: &str = "otter-entry";

/// Run the §6.2 checks on `bytes` against `manifest`.
pub fn check_component(bytes: &[u8], manifest: &ManifestSidecar) -> Result<ComponentReport, LinkError> {
    // Native validation first (EH-free feature set; component-model enabled). A validation
    // failure is terminal — structure below would be untrustworthy.
    let features = WasmFeatures::all()
        .difference(WasmFeatures::EXCEPTIONS)
        .difference(WasmFeatures::LEGACY_EXCEPTIONS);
    Validator::new_with_features(features)
        .validate_all(bytes)
        .map_err(|e| LinkError::Component(vec![format!("native validation (EH-free features): {e}")]))?;

    // Structural walk: depth-0 surface + nested core module ranges.
    let mut exports: Vec<(String, &'static str)> = Vec::new();
    let mut imports: Vec<(String, &'static str)> = Vec::new();
    let mut nested: Vec<std::ops::Range<usize>> = Vec::new();
    let mut depth: usize = 0;
    let mut seen_component_header = false;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        match payload {
            Payload::Version { encoding, .. } => {
                // Only the OUTER header decides the input kind (nested modules re-emit one).
                if !seen_component_header {
                    if encoding != Encoding::Component {
                        return Err(LinkError::Component(vec![
                            "input is a CORE MODULE, not a component — the §6.2 chain order \
                             (embed --world → new) was not run"
                                .to_string(),
                        ]));
                    }
                    seen_component_header = true;
                }
            }
            Payload::ModuleSection { unchecked_range, .. } => {
                if depth == 0 {
                    let start = usize::try_from(unchecked_range.start)
                        .map_err(|_| LinkError::Malformed("module range exceeds usize".into()))?;
                    let end = usize::try_from(unchecked_range.end)
                        .map_err(|_| LinkError::Malformed("module range exceeds usize".into()))?;
                    nested.push(start..end);
                }
                depth += 1;
            }
            Payload::ComponentSection { .. } => depth += 1,
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::ComponentExportSection(reader) if depth == 0 => {
                for export in reader {
                    let export = export.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    exports.push((export.name.name.to_string(), export.kind.desc()));
                }
            }
            Payload::ComponentImportSection(reader) if depth == 0 => {
                for import in reader {
                    let import = import.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    let kind = match import.ty {
                        wasmparser::ComponentTypeRef::Func(_) => "func",
                        wasmparser::ComponentTypeRef::Module(_) => "module",
                        wasmparser::ComponentTypeRef::Value(_) => "value",
                        wasmparser::ComponentTypeRef::Type(_) => "type",
                        wasmparser::ComponentTypeRef::Instance(_) => "instance",
                        wasmparser::ComponentTypeRef::Component(_) => "component",
                    };
                    imports.push((import.name.name.to_string(), kind));
                }
            }
            _ => {}
        }
    }

    let mut violations: Vec<String> = Vec::new();

    // Empty-world detection (the E10 trap, machine form). Checked BEFORE the shape match so
    // the failure names the actual pitfall, not a generic mismatch.
    if exports.is_empty() {
        violations.push(
            "EMPTY WORLD: component exports nothing — core exports are NOT auto-promoted \
             (SPK L5/§4-9); run `component embed --world` before `component new` (§6.2)"
                .to_string(),
        );
    } else {
        // World-shape match, export side: exactly the canonical entry, a func.
        match exports.as_slice() {
            [(name, kind)] if name == WORLD_EXPORT && *kind == "func" => {}
            _ => violations.push(format!(
                "export surface {:?} != the canonical world surface [(\"{WORLD_EXPORT}\", func)] \
                 (§6.1)",
                exports
            )),
        }
    }

    // World-shape match, import side: EXACTLY the kebab image of the manifest's otter_host
    // set (both directions — a lost lift and an excess import are both red, §5.1 discipline).
    let mut expected: Vec<String> = Vec::new();
    for imp in &manifest.imports {
        if matches!(imp.kind, ImportKind::Host { .. }) {
            if imp.name.contains('-') {
                violations.push(format!(
                    "manifest host import `{}` contains '-' — kebab reverse-uniqueness lost \
                     (§6.1 rejection ①)",
                    imp.name
                ));
                continue;
            }
            let converted = kebab(&imp.name);
            if !is_valid_wit_ident(&converted) {
                violations.push(format!(
                    "manifest host import `{}` → `{converted}` is not a valid WIT identifier \
                     (§6.1 rejection ③)",
                    imp.name
                ));
                continue;
            }
            expected.push(converted);
        }
    }
    let actual: Vec<&str> = imports.iter().map(|(n, _)| n.as_str()).collect();
    for want in &expected {
        if !actual.contains(&want.as_str()) {
            violations.push(format!(
                "host import `{want}` LOST: declared in the manifest's otter_host set but not \
                 lifted to the component boundary"
            ));
        }
    }
    for (name, kind) in &imports {
        if !expected.iter().any(|w| w == name) {
            violations.push(format!(
                "unexpected component import `{name}` ({kind}) — not in the manifest's \
                 otter_host set"
            ));
        } else if *kind != "func" {
            violations.push(format!(
                "component import `{name}` has kind {kind}, expected func (Host1/HostCtx1 \
                 scalar shapes)"
            ));
        }
    }

    // EH census over the embedded core module(s) — operator + tag parity with check --linked.
    if nested.is_empty() {
        violations.push(
            "component embeds no core module — not a `component new` product of the link chain".to_string(),
        );
    }
    for (i, range) in nested.iter().enumerate() {
        match eh_census_core(&bytes[range.clone()]) {
            Ok(lines) => {
                for line in lines {
                    violations.push(format!("nested core module {i}: {line}"));
                }
            }
            Err(e) => violations.push(format!("nested core module {i}: census failed: {e}")),
        }
    }

    if violations.is_empty() {
        Ok(ComponentReport {
            exports: exports.into_iter().map(|(n, _)| n).collect(),
            imports: imports.into_iter().map(|(n, _)| n).collect(),
            nested_modules: nested.len(),
        })
    } else {
        Err(LinkError::Component(violations))
    }
}

/// EH census over a CORE module's bytes: every EH operator occurrence + the tag census
/// (defined and imported), as violation lines (empty = census 0).
fn eh_census_core(bytes: &[u8]) -> Result<Vec<String>, LinkError> {
    let mut lines: Vec<String> = Vec::new();
    let mut tags = 0usize;
    let mut imported_tags = 0usize;
    let mut body_index = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        match payload {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    if matches!(import.ty, wasmparser::TypeRef::Tag(_)) {
                        imported_tags += 1;
                    }
                }
            }
            Payload::TagSection(reader) => tags += reader.count() as usize,
            Payload::CodeSectionEntry(body) => {
                let idx = body_index;
                body_index += 1;
                let reader = body
                    .get_operators_reader()
                    .map_err(|e| LinkError::Malformed(e.to_string()))?;
                for op in reader {
                    let op = op.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    if is_eh_op(&op) {
                        lines.push(format!("EH operator present (body {idx}): {op:?}"));
                    }
                }
            }
            _ => {}
        }
    }
    if tags != 0 || imported_tags != 0 {
        lines.push(format!(
            "tag section present ({tags} defined, {imported_tags} imported) — EH \
             infrastructure must be 0"
        ));
    }
    Ok(lines)
}
