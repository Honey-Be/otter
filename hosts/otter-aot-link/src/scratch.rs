//! `check --scratch` — the §5.2 static scratch checks, all three, fail-closed:
//!
//! - **(i) marker match** (width-branched, §2-1): the memory-owning module (pre-link injected
//!   b′ gate copy / post-link product) must carry EXACTLY the shape's markers at EXACTLY the
//!   mandated offsets with EXACTLY the mandated magics — plus the common sub-check
//!   **non-marker active data ∩ region = ∅** (an overlapping segment is a reservation
//!   violation even when every marker is in place — §5.6-⑩'s target). `W == 0` is the explicit
//!   empty-region branch: no markers, shape logged by the caller (never a silent pass), and
//!   the ∩ sub-check is EXEMPT — an empty `[base, base)` region contains no bytes, so nothing
//!   can intersect it (L-2 hardening: the L-1b review's false-red on segments spanning the
//!   empty point).
//!   PASSIVE-SEGMENT BLIND SPOT (stated per the design): passive segments carry no static
//!   address, so the overlap sub-check cannot see them; their placement is runtime
//!   `memory.init` code. The floor side of that blind spot is absorbed by the
//!   `memory_min×65536` term in the §2-2 heap_floor formula (panel M1).
//! - **(ii) coverage** (§2-3): `memory min pages ≥ ⌈end/65536⌉` (trivially green on W==0).
//! - **(iii) library floor** (§2-2): `region.base ≥ sidecar.heap_floor` — the ONLY static
//!   defense against shadow-stack/scratch overlap (the stack is not a data segment; SPK §5-2).
//!
//! Consumes the library sidecar fail-closed (§4.3): the sidecar must be finalized and its
//! `artifact_sha256` must match the b′ master bytes handed in (`--lib-artifact`) — check
//! re-verifies the pair rather than trusting that inject-marker already did (no bypass).
//! Recovery for a red (iii): RE-EMIT with a new base (compile_module re-call, O-14 §2.5-iv) —
//! §5.2/G-N-4.

use crate::facts::ModuleFacts;
use crate::marker::{RegionShape, region_shape};
use crate::schema::{LibrarySidecar, ManifestSidecar};
use crate::{LinkError, sha256_hex};

/// Green-path summary of a passed `check --scratch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScratchReport {
    /// Which width branch ran (callers MUST log `Empty` explicitly — §2-1).
    pub shape: RegionShape,
    /// Markers found (== the shape's mandated count).
    pub markers: usize,
    /// Artifact memory min (pages).
    pub min_pages: u64,
    /// Pages required by coverage (`⌈end/65536⌉`; 0 reported on the empty branch).
    pub required_pages: u64,
    /// Region base.
    pub base: u32,
    /// Library floor the base was checked against.
    pub heap_floor: u64,
}

/// Run all three §5.2 checks on `artifact` for `manifest.scratch_region`, against the
/// library sidecar (+ the b′ master bytes for the §4.3 pair re-verification).
pub fn check_scratch(
    artifact: &[u8],
    manifest: &ManifestSidecar,
    sidecar: &LibrarySidecar,
    lib_artifact: &[u8],
) -> Result<ScratchReport, LinkError> {
    // §4.3 pair binding, fail-closed before any check result is produced.
    let want = sidecar.required_sha()?;
    let got = sha256_hex(lib_artifact);
    if want != got {
        return Err(LinkError::Pair(format!(
            "--lib-artifact hashes to {got} but the library sidecar binds {want} — wrong \
             b′/sidecar pair (§4.3)"
        )));
    }
    if sidecar.shared != manifest.shared_memory {
        return Err(LinkError::Pair(format!(
            "profile mismatch: library sidecar shared={} but product manifest \
             shared_memory={}",
            sidecar.shared, manifest.shared_memory
        )));
    }

    let facts = ModuleFacts::parse(artifact)?;
    let memory = facts.owned_memory("check --scratch")?;
    let (base, end) = (manifest.scratch_region.base, manifest.scratch_region.end);
    let shape = region_shape(base, end)?;
    let slots = shape.marker_slots(base, end);

    let mut violations: Vec<String> = Vec::new();

    // (i) marker match + non-marker ∩ region = ∅.
    let mut found = vec![false; slots.len()];
    for (i, seg) in facts.data.iter().enumerate() {
        let Some(active) = &seg.active else {
            continue; // passive: the documented blind spot (no static address to audit).
        };
        if active.memory_index != 0 {
            violations.push(format!(
                "(i) active data segment #{i} targets memory {} — single-memory artifacts \
                 only",
                active.memory_index
            ));
            continue;
        }
        let Some(off) = active.static_offset else {
            violations.push(format!(
                "(i) active data segment #{i} has a non-`i32.const` offset expr — placement \
                 not statically verifiable, refusing (fail-closed)"
            ));
            continue;
        };
        let len = seg.bytes.len() as u64;
        if let Some(slot) = slots
            .iter()
            .position(|(slot_off, _)| *slot_off == off && len == 8)
        {
            if seg.bytes != slots[slot].1 {
                violations.push(format!(
                    "(i) marker at 0x{off:x} has wrong magic {:?} (expected {:?})",
                    String::from_utf8_lossy(&seg.bytes),
                    String::from_utf8_lossy(&slots[slot].1)
                ));
            } else if found[slot] {
                violations.push(format!("(i) duplicate marker at 0x{off:x}"));
            } else {
                found[slot] = true;
            }
            continue;
        }
        // Not a marker slot: the common ∩ = ∅ sub-check. W==0 is EXEMPT (L-2 hardening — the
        // L-1b review's false-red: `[base, base)` contains no bytes, so nothing can intersect
        // it; the old `off < end && off+len > base` still fired on a segment SPANNING the
        // empty point, flagging a reservation violation against a region that reserves
        // nothing).
        if shape == RegionShape::Empty {
            continue;
        }
        if off < end && off + len > u64::from(base) {
            violations.push(format!(
                "(i) non-marker active data segment #{i} [0x{off:x}, 0x{:x}) intersects the \
                 scratch region [0x{base:x}, 0x{end:x}) — reservation violated",
                off + len
            ));
        }
    }
    for (slot, ok) in found.iter().enumerate() {
        if !ok {
            let (off, magic) = &slots[slot];
            violations.push(format!(
                "(i) missing marker {:?} at 0x{off:x} (§2-1 {})",
                String::from_utf8_lossy(magic),
                shape.describe()
            ));
        }
    }

    // (ii) coverage (trivially green on the empty branch — §5.2).
    let required_pages = if shape == RegionShape::Empty {
        0
    } else {
        end.div_ceil(65536)
    };
    if memory.min < required_pages {
        violations.push(format!(
            "(ii) memory min {} page(s) < required {} (⌈0x{end:x}/65536⌉) — scratch region \
             not covered",
            memory.min, required_pages
        ));
    }

    // (iii) library floor — the shadow-stack overlap defense (SPK §5-2).
    if u64::from(base) < sidecar.heap_floor {
        violations.push(format!(
            "(iii) scratch base 0x{base:x} < library heap_floor 0x{:x} — shadow-stack/static \
             image overlap; recovery = RE-EMIT with a new base (O-14 §2.5-iv)",
            sidecar.heap_floor
        ));
    }

    if violations.is_empty() {
        Ok(ScratchReport {
            shape,
            markers: slots.len(),
            min_pages: memory.min,
            required_pages,
            base,
            heap_floor: sidecar.heap_floor,
        })
    } else {
        Err(LinkError::Scratch(violations))
    }
}
