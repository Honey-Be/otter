//! `check --imports` — the §5.1/§2-4 residual-import gate.
//!
//! Gate wording (§2-4, verbatim contract): the combined artifact's residual core import set
//! must EXACTLY EQUAL the `otter_host` entry set of `manifest.imports` — names AND types, in
//! BOTH directions; everything else (`otter.memory`, `otter_helpers.*`, any other module) is 0.
//! `⊆` is rejected (panel G-SF-3): wasm-merge has no registered `otter_host` provider, so a
//! host import can never be resolved — a residual PROPER SUBSET means an import was LOST (an
//! anomaly), and letting it pass silently would violate fail-closed. One residual entry not in
//! the manifest is red; one manifest entry missing from the residual set is red; a name match
//! with a type mismatch is red. Host-free artifacts pass via empty-set equality;
//! `--require-self-contained` is the emphasis mode that ADDITIONALLY refuses a manifest that
//! declares host imports at all.
//!
//! Type comparison: the expected wasm type of each `otter_host` import derives from the
//! manifest kind via the canonical O-14 §2.3 mapping (HOST1/HOST2/HOSTCTX2 — total for
//! `arity ∈ {1,2} × ctx`); the artifact side is read from its own type section. This gate runs
//! on the LINKED artifact, whose bytes wasm-merge re-encoded — the original artifact's type
//! section (the O-14 §4.1 type authority) already agreed with these canonical types at
//! `compile_module`/`validate_aot_artifact` time.

use std::collections::BTreeMap;

use wasmparser::{CompositeInnerType, FuncType, Parser, Payload, TypeRef, ValType};

use crate::LinkError;
use crate::schema::{ImportKind, ManifestSidecar};

/// The module string of embedder-provided host imports (O-14 §2.2 naming contract; the rename
/// step leaves this namespace untouched — §2-6: only `otter`/`otter_helpers` are renamed).
pub const HOST_MODULE: &str = "otter_host";

/// Green-path summary of a passed gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportsReport {
    /// Number of residual `otter_host` imports matched (== manifest otter_host entry count).
    pub matched_hosts: usize,
    /// Whether the artifact ended up fully self-contained (zero imports).
    pub self_contained: bool,
}

/// Run the residual-import gate on `artifact` against `manifest`.
pub fn check_imports(
    artifact: &[u8],
    manifest: &ManifestSidecar,
    require_self_contained: bool,
) -> Result<ImportsReport, LinkError> {
    // Expected set: the manifest's otter_host entries, keyed by name → i64 param count.
    // Duplicates or non-Host kinds under otter_host are manifest errors (never a pass).
    let mut expected: BTreeMap<&str, (usize, usize)> = BTreeMap::new(); // name → (params, budget)
    for entry in manifest.imports.iter().filter(|e| e.module == HOST_MODULE) {
        if !matches!(entry.kind, ImportKind::Host { .. }) {
            return Err(LinkError::Manifest(format!(
                "manifest import `{HOST_MODULE}.{}` has non-host kind {:?}",
                entry.name, entry.kind
            )));
        }
        let params = entry.kind.host_param_count()?;
        if expected.insert(entry.name.as_str(), (params, 1)).is_some() {
            return Err(LinkError::Manifest(format!(
                "manifest declares `{HOST_MODULE}.{}` more than once (host names are distinct \
                 by first-resolution order — O-14 §2.3)",
                entry.name
            )));
        }
    }

    let mut violations: Vec<String> = Vec::new();
    if require_self_contained && !expected.is_empty() {
        violations.push(format!(
            "--require-self-contained, but the manifest declares {} otter_host import(s): {}",
            expected.len(),
            expected.keys().copied().collect::<Vec<_>>().join(", ")
        ));
    }

    // Artifact side: type section + import section.
    let mut func_types: Vec<Option<FuncType>> = Vec::new();
    let mut residuals: Vec<(String, String, TypeRef)> = Vec::new();
    for payload in Parser::new(0).parse_all(artifact) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        match payload {
            Payload::TypeSection(reader) => {
                for group in reader {
                    let group = group.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    for ty in group.into_types() {
                        func_types.push(match &ty.composite_type.inner {
                            CompositeInnerType::Func(f) => Some(f.clone()),
                            _ => None,
                        });
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    residuals.push((import.module.into(), import.name.into(), import.ty));
                }
            }
            _ => {}
        }
    }

    // Direction 1: every residual import must be an expected otter_host function import.
    for (module, name, ty) in &residuals {
        if module != HOST_MODULE {
            violations.push(format!(
                "residual non-host import `{module}.{name}` (must be 0 — §5.1: only the \
                 manifest's otter_host set may remain)"
            ));
            continue;
        }
        let Some((params, budget)) = expected.get_mut(name.as_str()) else {
            violations.push(format!(
                "residual import `{HOST_MODULE}.{name}` is not in the manifest's otter_host \
                 set (excess)"
            ));
            continue;
        };
        if *budget == 0 {
            violations.push(format!(
                "residual import `{HOST_MODULE}.{name}` appears more than once (excess)"
            ));
            continue;
        }
        let type_ok = match ty {
            TypeRef::Func(idx) | TypeRef::FuncExact(idx) => {
                match func_types.get(*idx as usize).and_then(Option::as_ref) {
                    Some(f) => sig_matches(f, *params),
                    None => false,
                }
            }
            _ => false,
        };
        if !type_ok {
            violations.push(format!(
                "residual import `{HOST_MODULE}.{name}` type mismatch: expected \
                 ({}) -> i64 per the manifest kind (O-14 §2.3 canonical), artifact declares {}",
                vec!["i64"; *params].join(", "),
                describe_type(ty, &func_types)
            ));
            continue;
        }
        *budget = 0; // matched exactly once
    }

    // Direction 2: every expected entry must have been matched (a lost import is an anomaly —
    // wasm-merge cannot have resolved it; ⊆ would let this pass silently, hence `=`).
    for (name, (params, budget)) in &expected {
        if *budget != 0 {
            violations.push(format!(
                "manifest otter_host import `{HOST_MODULE}.{name}` (({}) -> i64) is MISSING \
                 from the residual set (lost import)",
                vec!["i64"; *params].join(", ")
            ));
        }
    }

    if violations.is_empty() {
        Ok(ImportsReport {
            matched_hosts: expected.len(),
            self_contained: residuals.is_empty(),
        })
    } else {
        Err(LinkError::Imports(violations))
    }
}

/// `(i64 × params) -> i64` — the canonical host signature shape (O-14 §2.3).
fn sig_matches(f: &FuncType, params: usize) -> bool {
    f.params().len() == params
        && f.params().iter().all(|p| *p == ValType::I64)
        && f.results() == [ValType::I64]
}

/// Human-readable description of an artifact-side import type (for violation messages).
fn describe_type(ty: &TypeRef, func_types: &[Option<FuncType>]) -> String {
    match ty {
        TypeRef::Func(idx) | TypeRef::FuncExact(idx) => {
            match func_types.get(*idx as usize).and_then(Option::as_ref) {
                Some(f) => format!(
                    "({}) -> ({})",
                    f.params()
                        .iter()
                        .map(|p| format!("{p:?}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    f.results()
                        .iter()
                        .map(|r| format!("{r:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => format!("func type index {idx} (unresolvable or non-func)"),
            }
        }
        other => format!("non-function import ({other:?})"),
    }
}
