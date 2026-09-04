//! `shrink-exports` — the §5.4 export-surface reduction (L-4 increment; moved here from L-1
//! by panel G-SF-5).
//!
//! The linked artifact's export set is the UNION of the b′ surface and the product surface
//! (SPK L3/§4-8 — the "8종" figure is the stand-in-host shape's; the real chain's union is
//! per-product and recorded by the L-4 recipe). This subcommand reduces it to a KEEP LIST the
//! caller owns (§5.4: the recipe passes the O-14 §2.4 surface `otter_entry`/`otter_table`
//! plus `memory`, the latter kept for the §12.4 args-block marshaling duty; keeping the list
//! a CALLER input rather than a hardcoded constant is the same generality rule as rename's
//! mapping table, §6.1/G-SF-2).
//!
//! Fail-closed: an empty keep list is refused (operator error), and every keep name must
//! actually exist in the export section — a keep name that matches nothing means the wrong
//! artifact (or a renamed surface) and silently "keeping" it would ship a surface hole.
//! The rewrite touches ONLY the export section and self-asserts exactly that
//! ([`crate::invariance::assert_invariance`], export mode) + wasmparser [`Validator`]
//! well-formedness — same construct-then-verify discipline as `rename`. The execution
//! differential AFTER the shrink (§5.4: "축소 후 실행-차등 재실행") is the recipe's, not
//! this function's.

use std::collections::BTreeSet;

use wasm_encoder::reencode::{Reencode, RoundtripReencoder};
use wasm_encoder::{Encode, ExportSection};
use wasmparser::{Parser, Payload, Validator, WasmFeatures};

use crate::section::scan_sections;
use crate::{LinkError, RenameMode, format_keys};

/// Green-path summary of a shrink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShrinkReport {
    /// Export names kept (export-section order).
    pub kept: Vec<String>,
    /// Export names dropped (export-section order).
    pub dropped: Vec<String>,
}

/// Shrink `bytes`' export section down to `keep`; returns the rewritten module + report.
pub fn shrink_exports(bytes: &[u8], keep: &BTreeSet<String>) -> Result<(Vec<u8>, ShrinkReport), LinkError> {
    if keep.is_empty() {
        return Err(LinkError::Shrink(
            "empty keep list — a shrink to zero exports is treated as operator error (the \
             §5.4 gate surface is at least otter_entry + otter_table)"
                .into(),
        ));
    }
    let spans = scan_sections(bytes)?;
    if spans.iter().filter(|s| s.id == 7).count() > 1 {
        return Err(LinkError::Malformed("more than one export section".into()));
    }

    let mut kept: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut new_section: Option<Vec<u8>> = None;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        if let Payload::ExportSection(reader) = payload {
            let mut enc = ExportSection::new();
            for export in reader {
                let export = export.map_err(|e| LinkError::Malformed(e.to_string()))?;
                if keep.contains(export.name) {
                    let kind = RoundtripReencoder
                        .export_kind(export.kind)
                        .map_err(|e| LinkError::Malformed(format!("export kind: {e}")))?;
                    enc.export(export.name, kind, export.index);
                    kept.push(export.name.to_string());
                } else {
                    dropped.push(export.name.to_string());
                }
            }
            new_section = Some(encode_section(7, &enc));
        }
    }
    let Some(new_section) = new_section else {
        return Err(LinkError::Shrink(
            "module has no export section — nothing to shrink (wrong input file?)".into(),
        ));
    };

    // Every keep name must have matched (fail-closed — a no-match keep is a surface hole).
    let matched: BTreeSet<&str> = kept.iter().map(String::as_str).collect();
    let unmatched: BTreeSet<&str> = keep
        .iter()
        .map(String::as_str)
        .filter(|k| !matched.contains(*k))
        .collect();
    if !unmatched.is_empty() {
        return Err(LinkError::Shrink(format!(
            "keep name(s) matched no export: {} (exports present: {})",
            format_keys(&unmatched),
            kept.iter()
                .chain(dropped.iter())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[0..8]);
    for span in &spans {
        if span.id == 7 {
            out.extend_from_slice(&new_section);
        } else {
            out.extend_from_slice(&bytes[span.full.clone()]);
        }
    }

    // Self-assertions (construct-then-verify — never hand back an unchecked product).
    crate::invariance::assert_invariance(bytes, &out, RenameMode::Export)?;
    Validator::new_with_features(WasmFeatures::all())
        .validate_all(&out)
        .map_err(|e| LinkError::Validate(e.to_string()))?;

    Ok((out, ShrinkReport { kept, dropped }))
}

/// Encode a full section (id byte + size LEB + payload) from a wasm-encoder section value.
fn encode_section(id: u8, section: &impl Encode) -> Vec<u8> {
    let mut out = vec![id];
    section.encode(&mut out);
    out
}
