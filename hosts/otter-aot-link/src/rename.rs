//! `rename` — the §2-6/§6.1 mapping-table renamer.
//!
//! Implementation rule (§2-6, SPK §4-3): the target section (import in import mode, export in
//! export mode) is the ONLY section that gets re-encoded; every other section — customs
//! included — is copied through as raw bytes, so the §2-6 invariance holds BY CONSTRUCTION.
//! The tool still self-asserts it afterwards ([`crate::invariance::assert_invariance`]) and
//! validates the product with the wasmparser [`Validator`] (well-formedness only; the EH-census
//! and semantic checks belong to `check --linked`, L-4 — design §0-5).
//!
//! The mapping table is GENERAL (§6.1, panel G-SF-2): arbitrary module-string pairs and
//! arbitrary name pairs, no hardcoded `otter_*` strings — whatever module-string convention
//! the L-5 host-lift measurement reveals costs zero rework here. Fail-closed refusals:
//! an empty mapping, a mapping key that matches nothing, kebab outside export mode, and the
//! three §6.1 kebab rejections are all LOUD errors.

use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::reencode::{Reencode, RoundtripReencoder};
use wasm_encoder::{Encode, ExportSection, ImportSection};
use wasmparser::{Parser, Payload, Validator, WasmFeatures};

use crate::section::scan_sections;
use crate::{LinkError, RenameMaps, RenameMode, format_keys, is_valid_wit_ident, kebab};

/// Rename `bytes` per `maps` in `mode`; returns the rewritten module.
///
/// Post-conditions asserted by the function itself (not left to the caller):
/// 1. §2-6 invariance — only the mode's allowed section changed;
/// 2. wasmparser `Validator` accepts the product (all features enabled — this is a
///    well-formedness gate, not the EH-free gate).
pub fn rename(bytes: &[u8], mode: RenameMode, maps: &RenameMaps) -> Result<Vec<u8>, LinkError> {
    validate_maps(mode, maps)?;
    let spans = scan_sections(bytes)?;
    let target_id = mode.allowed_section_id();
    if spans.iter().filter(|s| s.id == target_id).count() > 1 {
        return Err(LinkError::Malformed(format!(
            "more than one section with id {target_id}"
        )));
    }

    // Build the replacement payload for the target section (if the module has one).
    let new_section: Option<Vec<u8>> = match mode {
        RenameMode::Import => rewrite_imports(bytes, maps)?,
        RenameMode::Export => rewrite_exports(bytes, maps)?,
    };

    let mut out = Vec::with_capacity(bytes.len() + 16);
    out.extend_from_slice(&bytes[0..8]);
    for span in &spans {
        if span.id == target_id {
            let sec = new_section
                .as_ref()
                .expect("target section framed by scanner but not rewritten");
            out.extend_from_slice(sec);
        } else {
            out.extend_from_slice(&bytes[span.full.clone()]);
        }
    }

    // Self-assertions (§2-6; fail-closed — never hand back an unchecked product).
    crate::invariance::assert_invariance(bytes, &out, mode)?;
    Validator::new_with_features(WasmFeatures::all())
        .validate_all(&out)
        .map_err(|e| LinkError::Validate(e.to_string()))?;
    Ok(out)
}

fn validate_maps(mode: RenameMode, maps: &RenameMaps) -> Result<(), LinkError> {
    if maps.kebab && mode != RenameMode::Export {
        return Err(LinkError::RenameMapping(
            "--kebab is export-mode only (§6.1: kebab renames are performed in the rename \
             subcommand's export mode)"
                .into(),
        ));
    }
    if !maps.modules.is_empty() && mode == RenameMode::Export {
        return Err(LinkError::RenameMapping(
            "module-string mappings apply to import mode only (export entries have no module \
             string)"
                .into(),
        ));
    }
    if maps.modules.is_empty() && maps.names.is_empty() && !maps.kebab {
        return Err(LinkError::RenameMapping(
            "empty mapping: nothing to rename (a no-op invocation is treated as operator error)".into(),
        ));
    }
    Ok(())
}

/// Import mode: rewrite module strings and/or field names per the tables.
/// Returns `None` when the module has no import section (then any mapping key is unmatched,
/// which the key audit below turns into a loud error).
fn rewrite_imports(bytes: &[u8], maps: &RenameMaps) -> Result<Option<Vec<u8>>, LinkError> {
    let mut used_modules: BTreeSet<&str> = BTreeSet::new();
    let mut used_names: BTreeSet<&str> = BTreeSet::new();
    let mut section: Option<ImportSection> = None;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        if let Payload::ImportSection(reader) = payload {
            let mut enc = ImportSection::new();
            for import in reader.into_imports() {
                let import = import.map_err(|e| LinkError::Malformed(e.to_string()))?;
                let module = match maps.modules.get_key_value(import.module) {
                    Some((k, v)) => {
                        used_modules.insert(k.as_str());
                        v.as_str()
                    }
                    None => import.module,
                };
                let name = match maps.names.get_key_value(import.name) {
                    Some((k, v)) => {
                        used_names.insert(k.as_str());
                        v.as_str()
                    }
                    None => import.name,
                };
                let entity = RoundtripReencoder
                    .entity_type(import.ty)
                    .map_err(|e| LinkError::Malformed(format!("entity type: {e}")))?;
                enc.import(module, name, entity);
            }
            section = Some(enc);
        }
    }

    audit_unmatched(&maps.modules, &used_modules, "module")?;
    audit_unmatched(&maps.names, &used_names, "name")?;

    Ok(section.map(|enc| encode_section(2, &enc)))
}

/// Export mode: rewrite export names per the explicit table and/or the §6.1 kebab conversion.
fn rewrite_exports(bytes: &[u8], maps: &RenameMaps) -> Result<Option<Vec<u8>>, LinkError> {
    let mut used_names: BTreeSet<&str> = BTreeSet::new();
    let mut section: Option<ExportSection> = None;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        if let Payload::ExportSection(reader) = payload {
            let mut finals: Vec<(String, wasm_encoder::ExportKind, u32)> = Vec::new();
            for export in reader {
                let export = export.map_err(|e| LinkError::Malformed(e.to_string()))?;
                let new_name: String = match maps.names.get_key_value(export.name) {
                    Some((k, v)) => {
                        used_names.insert(k.as_str());
                        v.clone()
                    }
                    None if maps.kebab => {
                        // §6.1 rejection ①: sources containing '-' are banned so that the
                        // `_`→`-` substitution stays reverse-unique.
                        if export.name.contains('-') {
                            return Err(LinkError::KebabDashSource(format!("export `{}`", export.name)));
                        }
                        let converted = kebab(export.name);
                        // §6.1 rejection ③: the result must be a valid WIT identifier.
                        if !is_valid_wit_ident(&converted) {
                            return Err(LinkError::KebabInvalidIdent(format!(
                                "export `{}` → `{converted}`",
                                export.name
                            )));
                        }
                        converted
                    }
                    None => export.name.to_string(),
                };
                let kind = RoundtripReencoder
                    .export_kind(export.kind)
                    .map_err(|e| LinkError::Malformed(format!("export kind: {e}")))?;
                finals.push((new_name, kind, export.index));
            }
            // §6.1 rejection ② (applied to EVERY export-mode rename, kebab or explicit —
            // duplicate export names are invalid wasm and must fail here, loudly, not in the
            // downstream validator): post-substitution collision.
            let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
            for (name, _, _) in &finals {
                *seen.entry(name.as_str()).or_insert(0) += 1;
            }
            let dups: Vec<&str> = seen.iter().filter(|&(_, &n)| n > 1).map(|(&k, _)| k).collect();
            if !dups.is_empty() {
                return Err(LinkError::RenameCollision(format!(
                    "export name(s) duplicated after substitution: {}",
                    dups.join(", ")
                )));
            }
            let mut enc = ExportSection::new();
            for (name, kind, index) in &finals {
                enc.export(name, *kind, *index);
            }
            section = Some(enc);
        }
    }

    // L-1a review incorporation (finding 2): --kebab is a conversion DIRECTIVE — a module with
    // no export section gives it nothing to act on, and silently returning the input green
    // would swallow a wrong-input-file mistake (explicit-map keys are already caught by the
    // unmatched-key audit below; kebab has no keys, so it needs its own loud refusal).
    if maps.kebab && section.is_none() {
        return Err(LinkError::RenameMapping(
            "--kebab, but the module has no export section — nothing to convert (wrong input \
             file?)"
                .into(),
        ));
    }

    audit_unmatched(&maps.names, &used_names, "name")?;

    Ok(section.map(|enc| encode_section(7, &enc)))
}

/// Every mapping key must have matched at least one entry — a key that matches nothing is an
/// operator error (fail-closed: the silent-partial class of SPK §4-1, refused at the source).
fn audit_unmatched(
    map: &BTreeMap<String, String>,
    used: &BTreeSet<&str>,
    what: &str,
) -> Result<(), LinkError> {
    let unmatched: BTreeSet<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|k| !used.contains(*k))
        .collect();
    if unmatched.is_empty() {
        Ok(())
    } else {
        Err(LinkError::RenameMapping(format!(
            "{what} mapping key(s) matched no entry: {}",
            format_keys(&unmatched)
        )))
    }
}

/// Encode a full section (id byte + size LEB + payload) from a wasm-encoder section value.
fn encode_section(id: u8, section: &impl Encode) -> Vec<u8> {
    let mut out = vec![id];
    section.encode(&mut out);
    out
}
