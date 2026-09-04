//! §2-6 post-rename invariance gate, per-mode:
//! - (a) import mode: the ONLY section whose bytes may differ is the import section (id 2) —
//!   code/type/function/table/element/export and every other section byte-identical;
//! - (b) export mode: the ONLY section whose bytes may differ is the export section (id 7) —
//!   code/type/function/table/element/import/memory/global/data and every other section
//!   byte-identical.
//!
//! The comparison is on RAW full-section bytes (id + size LEB + payload), so even a size-LEB
//! renormalization of an untouched section is refused — this is the other half of the §2-6
//! implementation contract ("import (or export) section bytes rewritten — every other
//! section's bytes identical").

use crate::section::scan_sections;
use crate::{LinkError, RenameMode};

/// Assert the §2-6 invariance between `original` and `renamed` for `mode`.
///
/// Rules (all violations are loud errors):
/// - headers byte-identical;
/// - same section count, same section-id sequence (a rename never adds, drops or reorders
///   sections — not even the allowed one);
/// - every section whose id is NOT the mode's allowed id: full bytes identical;
/// - the allowed section: free to differ (it may also be byte-identical, e.g. an explicit
///   mapping that only touches entries absent from this particular module is already refused
///   earlier as an unmatched key).
pub fn assert_invariance(original: &[u8], renamed: &[u8], mode: RenameMode) -> Result<(), LinkError> {
    if original.len() < 8 || renamed.len() < 8 || original[0..8] != renamed[0..8] {
        return Err(LinkError::Invariance("header bytes differ".into()));
    }
    let orig_spans = scan_sections(original)?;
    let new_spans = scan_sections(renamed)?;
    if orig_spans.len() != new_spans.len() {
        return Err(LinkError::Invariance(format!(
            "section count changed: {} → {}",
            orig_spans.len(),
            new_spans.len()
        )));
    }
    let allowed = mode.allowed_section_id();
    for (i, (o, n)) in orig_spans.iter().zip(new_spans.iter()).enumerate() {
        if o.id != n.id {
            return Err(LinkError::Invariance(format!(
                "section #{i} id changed: {} → {}",
                o.id, n.id
            )));
        }
        if o.id == allowed {
            continue; // the one section this mode may rewrite
        }
        if original[o.full.clone()] != renamed[n.full.clone()] {
            return Err(LinkError::Invariance(format!(
                "section #{i} (id {}) bytes changed outside the allowed section (id {allowed})",
                o.id
            )));
        }
    }
    Ok(())
}
