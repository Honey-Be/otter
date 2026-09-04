//! `inject-marker` — the §2-1 scratch-reservation marker injector.
//!
//! The marker is the STATIC EVIDENCE the §5.2 checks compare against (SPK §3-3: "메모리-소유
//! 모듈의 active data 세그먼트 = 예약 마커"); the producer is deliberately this deterministic
//! injection step, NOT rustc/wasm-ld link arguments (§2-1: that would be a fragile
//! linker-script craft AND leave the min-page auto-raise an unmeasured assumption — the
//! injector performs the raise CONSTRUCTIVELY instead).
//!
//! Width-branched semantics (§0-2/§2-1 verbatim; `W = end − base`, 8-byte-word granular):
//! - `W ≥ 16`: two markers — `[base, base+8)` = `"OTTRSCR0"`, `[end-8, end)` = `"OTTRSCR1"`
//!   (the tail marker gives §5.2-(ii) an end-of-region byte witness — belt and suspenders);
//! - `W == 8`: ONE merged marker `[base, end)` = `"OTTRSCRM"` (base == end−8: two markers
//!   cannot coexist — the merged magic serves both roles; panel M2's conservative pick over
//!   padding, which would have changed compile_module semantics);
//! - `W == 0`: no markers, min-raise no-op — the explicit empty-region branch; the CALLER must
//!   log the shape (never a silent pass).
//!
//! Fail-closed order (§2-1/§4.3): the input b′ master's SHA-256 is compared against the
//! library sidecar BEFORE anything is injected; the memory min is raised to
//! `max(current, ⌈end/65536⌉)` and a raise past the declared max (shared profiles always
//! declare one) is a LOUD injection failure. Region source is the per-product artifact
//! manifest (§4.2: injection is a PER-PRODUCT step of the link recipe).

use wasm_encoder::reencode::{Reencode, RoundtripReencoder};
use wasm_encoder::{ConstExpr, DataCountSection, DataSection, Encode, MemorySection, MemoryType};
use wasmparser::{BinaryReader, DataSectionReader, Validator, WasmFeatures};

use crate::facts::ModuleFacts;
use crate::schema::{LibrarySidecar, ManifestSidecar};
use crate::section::scan_sections;
use crate::{LinkError, sha256_hex};

/// Base-marker magic (`[base, base+8)`, W ≥ 16).
pub const MARKER_BASE_MAGIC: [u8; 8] = *b"OTTRSCR0";
/// End-marker magic (`[end-8, end)`, W ≥ 16).
pub const MARKER_END_MAGIC: [u8; 8] = *b"OTTRSCR1";
/// Merged single-word magic (`[base, end)`, W == 8).
pub const MARKER_MERGED_MAGIC: [u8; 8] = *b"OTTRSCRM";

/// The §2-1 width branch of a scratch region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionShape {
    /// `W == 0` — call-free program, nothing to reserve (explicit branch, logged by callers).
    Empty,
    /// `W == 8` — merged `OTTRSCRM` marker.
    MergedWord,
    /// `W ≥ 16` — base + end markers.
    TwoMarkers,
}

impl RegionShape {
    /// Human-readable branch label (for the mandated explicit logging).
    pub fn describe(self) -> &'static str {
        match self {
            RegionShape::Empty => "W==0 empty-region branch (markers 0, min-raise no-op)",
            RegionShape::MergedWord => "W==8 merged-marker branch (single OTTRSCRM word)",
            RegionShape::TwoMarkers => "W>=16 two-marker branch (OTTRSCR0 @ base, OTTRSCR1 @ end-8)",
        }
    }

    /// The `(offset, magic)` marker slots this shape mandates.
    pub fn marker_slots(self, base: u32, end: u64) -> Vec<(u64, [u8; 8])> {
        match self {
            RegionShape::Empty => vec![],
            RegionShape::MergedWord => vec![(u64::from(base), MARKER_MERGED_MAGIC)],
            RegionShape::TwoMarkers => {
                vec![(u64::from(base), MARKER_BASE_MAGIC), (end - 8, MARKER_END_MAGIC)]
            }
        }
    }
}

/// Classify `[base, end)` per §2-1. Loud on a malformed region (end < base, or a width that
/// is not 8-byte-word granular — scratch is allocated in 8-byte words, SPK/§2-1).
pub fn region_shape(base: u32, end: u64) -> Result<RegionShape, LinkError> {
    let base = u64::from(base);
    if end < base {
        return Err(LinkError::Manifest(format!(
            "scratch_region end 0x{end:x} < base 0x{base:x}"
        )));
    }
    let w = end - base;
    if !w.is_multiple_of(8) {
        return Err(LinkError::Manifest(format!(
            "scratch_region width {w} is not a multiple of 8 (scratch is 8-byte-word granular)"
        )));
    }
    Ok(match w {
        0 => RegionShape::Empty,
        8 => RegionShape::MergedWord,
        _ => RegionShape::TwoMarkers,
    })
}

/// Green-path summary of an injection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectReport {
    /// Which width branch ran.
    pub shape: RegionShape,
    /// Memory min (pages) before injection.
    pub old_min: u64,
    /// Memory min (pages) after the constructive raise (== old_min when already covering, and
    /// always on the W==0 no-op branch).
    pub new_min: u64,
    /// Number of marker segments injected (0 / 1 / 2).
    pub injected: usize,
}

/// Inject the §2-1 markers for `manifest.scratch_region` into the b′ master `input`.
///
/// Fail-closed sequence: ① sidecar pair check (input SHA-256 == `artifact_sha256`; an
/// unfinalized sidecar refuses), ② profile pairing (`sidecar.shared` ==
/// `manifest.shared_memory`) and sidecar-metadata-vs-artifact cross-check, ③ width branch,
/// ④ constructive min raise (loud when past the declared max), ⑤ byte surgery limited to the
/// memory/data/data-count sections + self-assertion of exactly that + full-feature Validator.
pub fn inject_markers(
    input: &[u8],
    sidecar: &LibrarySidecar,
    manifest: &ManifestSidecar,
) -> Result<(Vec<u8>, InjectReport), LinkError> {
    // ① §4.3 content binding — before anything else, no bypass flag.
    let want = sidecar.required_sha()?;
    let got = sha256_hex(input);
    if want != got {
        return Err(LinkError::Pair(format!(
            "input artifact hashes to {got} but the library sidecar binds {want} — wrong \
             b′/sidecar pair (§4.3)"
        )));
    }

    // ② profile pairing + sidecar metadata honesty.
    if sidecar.shared != manifest.shared_memory {
        return Err(LinkError::Pair(format!(
            "profile mismatch: library sidecar shared={} but product manifest \
             shared_memory={} — wrong b′ profile for this product (O-14 §12.1)",
            sidecar.shared, manifest.shared_memory
        )));
    }
    let facts = ModuleFacts::parse(input)?;
    let memory = facts.owned_memory("inject-marker")?;
    if memory.shared != sidecar.shared || memory.min != sidecar.memory_min || memory.max != sidecar.memory_max
    {
        return Err(LinkError::Pair(format!(
            "artifact memory (shared={}, min={}, max={:?}) contradicts the sidecar metadata \
             (shared={}, min={}, max={:?})",
            memory.shared, memory.min, memory.max, sidecar.shared, sidecar.memory_min, sidecar.memory_max
        )));
    }

    // ③ width branch.
    let (base, end) = (manifest.scratch_region.base, manifest.scratch_region.end);
    let shape = region_shape(base, end)?;
    if shape == RegionShape::Empty {
        // Explicit no-op branch — output == input, caller logs the shape (§2-1: not silent).
        return Ok((
            input.to_vec(),
            InjectReport {
                shape,
                old_min: memory.min,
                new_min: memory.min,
                injected: 0,
            },
        ));
    }

    // ④ constructive min raise.
    let required_pages = end.div_ceil(65536);
    let new_min = memory.min.max(required_pages);
    if let Some(max) = memory.max
        && new_min > max
    {
        return Err(LinkError::Inject(format!(
            "region end 0x{end:x} needs min {new_min} pages but the memory declares \
             max {max} — refusing (§2-1: raising past the declared max is a loud failure)"
        )));
    }

    // ⑤ byte surgery: memory (id 5, only when raising), data (id 11, append or create),
    // data count (id 12, bump only when present).
    let slots = shape.marker_slots(base, end);
    let spans = scan_sections(input)?;
    for id in [5u8, 11, 12] {
        if spans.iter().filter(|s| s.id == id).count() > 1 {
            return Err(LinkError::Malformed(format!(
                "more than one section with id {id}"
            )));
        }
    }

    let new_memory_bytes: Option<Vec<u8>> = if new_min != memory.min {
        let mut enc = MemorySection::new();
        enc.memory(MemoryType {
            minimum: new_min,
            maximum: memory.max,
            // Faithful carry (L-2 hardening: the review flagged a hardcoded `false` as a silent
            // memory64 rewrite class). owned_memory above REFUSES memory64/custom-page-size
            // inputs, so these are provably `false`/`None` here — carried, not assumed.
            memory64: memory.memory64,
            shared: memory.shared,
            page_size_log2: memory.page_size_log2,
        });
        Some(encode_section(5, &enc))
    } else {
        None
    };

    let mut data_enc = DataSection::new();
    if let Some(span) = spans.iter().find(|s| s.id == 11) {
        let reader = DataSectionReader::new(BinaryReader::new(
            &input[span.payload.clone()],
            span.payload.start as u64,
        ))
        .map_err(|e| LinkError::Malformed(e.to_string()))?;
        RoundtripReencoder
            .parse_data_section(&mut data_enc, reader)
            .map_err(|e| LinkError::Inject(format!("re-encoding existing data section: {e}")))?;
    }
    for (offset, magic) in &slots {
        let offset = i32::try_from(*offset)
            .or_else(|_| u32::try_from(*offset).map(|v| v as i32))
            .map_err(|_| LinkError::Inject(format!("marker offset 0x{offset:x} exceeds u32")))?;
        data_enc.active(0, &ConstExpr::i32_const(offset), magic.iter().copied());
    }
    let new_data_bytes = encode_section(11, &data_enc);

    let new_count_bytes: Option<Vec<u8>> = facts.data_count.map(|count| {
        encode_section(
            12,
            &DataCountSection {
                count: count + slots.len() as u32,
            },
        )
    });

    let mut out = Vec::with_capacity(input.len() + 64);
    out.extend_from_slice(&input[0..8]);
    let had_data = spans.iter().any(|s| s.id == 11);
    // Insertion point for a fresh data section: after the LAST non-custom section (data is the
    // last ordered section id; trailing custom sections may legally follow it).
    let insert_after = spans.iter().rposition(|s| s.id != 0);
    for (i, span) in spans.iter().enumerate() {
        match span.id {
            5 => match &new_memory_bytes {
                Some(bytes) => out.extend_from_slice(bytes),
                None => out.extend_from_slice(&input[span.full.clone()]),
            },
            11 => out.extend_from_slice(&new_data_bytes),
            12 => match &new_count_bytes {
                Some(bytes) => out.extend_from_slice(bytes),
                None => out.extend_from_slice(&input[span.full.clone()]),
            },
            _ => out.extend_from_slice(&input[span.full.clone()]),
        }
        if !had_data && insert_after == Some(i) {
            out.extend_from_slice(&new_data_bytes);
        }
    }
    if !had_data && insert_after.is_none() {
        out.extend_from_slice(&new_data_bytes);
    }

    assert_surgery_perimeter(input, &out)?;
    Validator::new_with_features(WasmFeatures::all())
        .validate_all(&out)
        .map_err(|e| LinkError::Validate(e.to_string()))?;

    Ok((
        out,
        InjectReport {
            shape,
            old_min: memory.min,
            new_min,
            injected: slots.len(),
        },
    ))
}

/// Self-assertion: the injection changed NOTHING outside the memory/data/data-count sections
/// (ids 5/11/12) — the same construct-then-verify discipline as rename's invariance gate.
fn assert_surgery_perimeter(input: &[u8], output: &[u8]) -> Result<(), LinkError> {
    let untouched = |bytes: &[u8]| -> Result<Vec<(u8, Vec<u8>)>, LinkError> {
        Ok(scan_sections(bytes)?
            .into_iter()
            .filter(|s| !matches!(s.id, 5 | 11 | 12))
            .map(|s| (s.id, bytes[s.full].to_vec()))
            .collect())
    };
    if untouched(input)? != untouched(output)? {
        return Err(LinkError::Inject(
            "self-assertion failed: a section outside memory/data/data-count changed".into(),
        ));
    }
    Ok(())
}

/// Encode a full section (id byte + size LEB + payload) from a wasm-encoder section value.
fn encode_section(id: u8, section: &impl Encode) -> Vec<u8> {
    let mut out = vec![id];
    section.encode(&mut out);
    out
}
