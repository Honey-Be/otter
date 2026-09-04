//! `lib-manifest` — the b′ library sidecar producer (design §2-2, two stages; §4.3):
//!
//! - [`extract`] (stage ①, PRE-strip): computes the sidecar's floor components and exports.
//!   The stack pointer is identified BY NAME (`__stack_pointer`, name section) — that is why
//!   §2-2 pins extraction to the pre-strip artifact. When name-based identification is
//!   impossible, the "largest-initial-value mutable i32 global" heuristic is deliberately a
//!   FAILURE, not a silent fallback (§8-6: "휴리스틱 폴백 시 경고-아닌-실패").
//! - [`finalize`] (stage ③, POST-strip): binds `artifact_sha256` over the strip-final master
//!   bytes — the very bytes the link chain consumes (§2-2 generation order, §4.3 content-hash
//!   truth) — after cross-checking that the stripped artifact still matches the extracted
//!   facts (strip removes custom sections ONLY; a memory/export drift means a wrong pair).
//!
//! heap_floor formula (§2-2, panel M1): `max(__stack_pointer initial, max active-data end,
//! memory_min × 65536)`. The `memory_min` term absorbs the passive-segment blind spot
//! (+atomics: `.rodata` is passive — no static address in the declaration; wasm-ld still
//! reflects the placed image in the memory min), and partially defends the floor even if
//! stack-pointer identification were wrong (§8-6).

use crate::facts::ModuleFacts;
use crate::schema::LibrarySidecar;
use crate::{LinkError, sha256_hex};

/// The name-section global name that identifies the shadow-stack pointer.
pub const STACK_POINTER_NAME: &str = "__stack_pointer";

/// Stage ① — extract the sidecar from the PRE-strip b′ artifact. `otter_version` and
/// `rustc_version` are record-only pass-throughs (§4.3): the recipe supplies them, this tool
/// never fabricates them. `artifact_sha256` is left `null` (stage ③ binds it).
pub fn extract(
    unstripped: &[u8],
    otter_version: &str,
    rustc_version: &str,
) -> Result<LibrarySidecar, LinkError> {
    let facts = ModuleFacts::parse(unstripped)?;
    // memory64 + custom-page-size refusals live in owned_memory since the L-2 hardening (the
    // common seam for ALL consumer paths — extract's own memory64 check moved there).
    let memory = facts.owned_memory("lib-manifest --extract")?;

    // Stack pointer: name-section identification ONLY (§8-6 — no silent heuristic).
    let sp_indices: Vec<u32> = facts
        .global_names
        .iter()
        .filter(|(_, name)| name == STACK_POINTER_NAME)
        .map(|(idx, _)| *idx)
        .collect();
    let sp_index = match sp_indices.as_slice() {
        [one] => *one,
        [] => {
            return Err(LinkError::Extract(format!(
                "no global named `{STACK_POINTER_NAME}` in the name section — extraction MUST \
                 run on the PRE-strip artifact (§2-2); the largest-initial-value-mutable-i32 \
                 heuristic is deliberately a failure, not a silent fallback (§8-6)"
            )));
        }
        many => {
            return Err(LinkError::Extract(format!(
                "{} globals named `{STACK_POINTER_NAME}` — ambiguous, refusing",
                many.len()
            )));
        }
    };
    let defined_index = (sp_index as usize)
        .checked_sub(facts.imported_globals)
        .ok_or_else(|| {
            LinkError::Extract(format!(
                "`{STACK_POINTER_NAME}` (global #{sp_index}) is an IMPORTED global — its \
                 initial value is not statically knowable"
            ))
        })?;
    let sp = facts.globals.get(defined_index).ok_or_else(|| {
        LinkError::Extract(format!(
            "name section names global #{sp_index} but the module defines only {} globals \
             after {} imports",
            facts.globals.len(),
            facts.imported_globals
        ))
    })?;
    if sp.ty != wasmparser::ValType::I32 || !sp.mutable {
        return Err(LinkError::Extract(format!(
            "`{STACK_POINTER_NAME}` is {}{:?}, expected a mutable i32 (does not look like a \
             shadow-stack pointer)",
            if sp.mutable { "mutable " } else { "immutable " },
            sp.ty
        )));
    }
    let sp_init = sp.i32_init.ok_or_else(|| {
        LinkError::Extract(format!(
            "`{STACK_POINTER_NAME}` init expr is not a plain `i32.const` — not statically \
             evaluable, refusing"
        ))
    })?;

    // Max active-data end. Non-static active offsets are a loud failure — the floor must
    // never be computed from a guess. Passive segments are skipped: that is the §2-2 blind
    // spot the memory_min term absorbs.
    let mut max_active_end: u64 = 0;
    for (i, seg) in facts.data.iter().enumerate() {
        if let Some(active) = &seg.active {
            let offset = active.static_offset.ok_or_else(|| {
                LinkError::Extract(format!(
                    "active data segment #{i} has a non-`i32.const` offset expr — not \
                     statically evaluable, refusing"
                ))
            })?;
            max_active_end = max_active_end.max(offset + seg.bytes.len() as u64);
        }
    }

    let heap_floor = sp_init.max(max_active_end).max(memory.min * 65536);
    Ok(LibrarySidecar {
        heap_floor,
        shared: memory.shared,
        memory_min: memory.min,
        memory_max: memory.max,
        exports: facts.exports,
        otter_version: otter_version.to_string(),
        rustc_version: rustc_version.to_string(),
        artifact_sha256: None,
    })
}

/// Stage ③ — bind the content hash over the strip-final master bytes. Cross-checks the
/// stripped artifact against the extracted facts first (memory shape + exports survive a
/// strip untouched — a mismatch means the sidecar and this artifact are NOT a pair).
/// Idempotent: re-finalizing with the same bytes is a no-op; a DIFFERENT hash on an already
/// finalized sidecar is a loud pair violation.
pub fn finalize(sidecar: &LibrarySidecar, stripped: &[u8]) -> Result<LibrarySidecar, LinkError> {
    let facts = ModuleFacts::parse(stripped)?;
    let memory = facts.owned_memory("lib-manifest --finalize")?;
    if memory.shared != sidecar.shared || memory.min != sidecar.memory_min || memory.max != sidecar.memory_max
    {
        return Err(LinkError::Pair(format!(
            "stripped artifact memory (shared={}, min={}, max={:?}) contradicts the sidecar \
             (shared={}, min={}, max={:?}) — wrong sidecar/artifact pair",
            memory.shared, memory.min, memory.max, sidecar.shared, sidecar.memory_min, sidecar.memory_max
        )));
    }
    if facts.exports != sidecar.exports {
        return Err(LinkError::Pair(format!(
            "stripped artifact exports {:?} contradict the sidecar exports {:?} — wrong \
             sidecar/artifact pair (strip removes custom sections only)",
            facts.exports, sidecar.exports
        )));
    }
    let sha = sha256_hex(stripped);
    if let Some(existing) = &sidecar.artifact_sha256
        && *existing != sha
    {
        return Err(LinkError::Pair(format!(
            "sidecar is already finalized to {existing} but these bytes hash to {sha} — \
             refusing to rebind"
        )));
    }
    let mut out = sidecar.clone();
    out.artifact_sha256 = Some(sha);
    Ok(out)
}
