//! The JSON sidecar schema layer — SOLE owner of the sidecar type definitions, serialization
//! and deserialization (design §3.1/§0-7): producers (compile_module callers such as
//! otter-aot-e2e) path-dep THIS crate and serialize the very same types, so the schema never
//! forks into a second hand-rolled truth (the record==live double-source class, blocked at the
//! schema layer).
//!
//! Owned sidecars: the ARTIFACT-manifest sidecar ([`ManifestSidecar`] — the machine-readable
//! half of an `AotArtifact`, mirroring `otter_wasmemit::AotManifest`; consumed fields per SPK
//! §3-2 = imports / scratch_region / shared_memory / entry_slot / table_size, all fields
//! carried; L-1a), and the b′ LIBRARY sidecar ([`LibrarySidecar`] —
//! `otter_helpers.manifest.json`: heap_floor / shared / memory_min·max / exports / versions /
//! artifact_sha256, §2-2/§4.3; L-1b — produced by `lib-manifest --extract`/`--finalize`).
//!
//! Fail-closed posture: `deny_unknown_fields` everywhere (an unknown field means schema drift
//! between producer and consumer generations — refuse, don't guess), and the conversion from
//! the live `AotManifest` is a `TryFrom` that REFUSES unknown (`#[non_exhaustive]`-future)
//! import kinds instead of mislabeling them.

use serde::{Deserialize, Serialize};

use otter_wasmemit::{AotImportKind, AotManifest, HelperKind};

use crate::LinkError;

/// JSON mirror of `otter_wasmemit::AotManifest` (the artifact-manifest sidecar placed next to
/// the emitted `.wasm` by the compile_module caller — design §3.1 "매니페스트 입력 = JSON 사이드카").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSidecar {
    /// `AotManifest::entry_slot`.
    pub entry_slot: u32,
    /// `AotManifest::nfuncs`.
    pub nfuncs: u32,
    /// `AotManifest::helper_slots`.
    pub helper_slots: [u32; 3],
    /// `AotManifest::host_slots`.
    pub host_slots: Vec<u32>,
    /// `AotManifest::imports`, import-section order preserved.
    pub imports: Vec<ImportEntry>,
    /// `AotManifest::scratch` (`words` widened to u64 for a stable JSON shape).
    pub scratch: Vec<ScratchEntry>,
    /// `AotManifest::scratch_region` (`[base, end)`; `end` is u64 — `== 2^32` is legal).
    pub scratch_region: ScratchRegion,
    /// `AotManifest::table_size`.
    pub table_size: u32,
    /// `AotManifest::shared_memory`.
    pub shared_memory: bool,
    /// `AotManifest::memory64` (O-19 C5, design §2.5 [v2 — tech-S3/scope-N10]). `#[serde(default)]`
    /// (= `false`) because the field postdates shipped v1 sidecars: an OLD sidecar (field absent)
    /// parses as `false`, and default=false stays FAIL-CLOSED because the consumer-side
    /// record==live cross-check (`check --linked`'s memory64 predicate, linked.rs) refuses
    /// memory64 BYTES against a false/old manifest — the artifact bytes are the ground truth,
    /// never the manifest alone. The producer bridge below still REFUSES to serialize an A64
    /// manifest at all (the link/sidecar chain is wasm32-only until its external blockers
    /// clear): this field exists so CONSUMERS can cross-check, not so producers can smuggle
    /// A64 into the chain.
    #[serde(default)]
    pub memory64: bool,
    /// `AotManifest::emitter_version` — recorded verbatim. NOTE (§4.3, honest by design):
    /// while the whole repo is version `0.0.0` this comparison has ZERO discriminating power;
    /// the fail-closed story is carried by the b′ sidecar's content hash (L-1b).
    pub emitter_version: String,
}

/// One import entry (`AotImport` mirror).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportEntry {
    /// Import module string (e.g. `otter`, `otter_helpers`, `otter_host`).
    pub module: String,
    /// Import field name.
    pub name: String,
    /// What the import is (`AotImportKind` mirror).
    pub kind: ImportKind,
}

/// `AotImportKind` mirror. The wasm-level TYPE is deliberately not duplicated (O-14 §4.1: the
/// canonical type source is the artifact's type section; `Host{arity,ctx}` maps TOTALLY onto
/// the canonical HOST1/HOST2/HOSTCTX2 types — see [`ImportKind::host_param_count`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportKind {
    /// `otter.memory`.
    Memory,
    /// `otter_helpers.otter_{binop,convert,unop}`.
    Helper {
        /// Which helper.
        helper: HelperName,
    },
    /// `otter_host.<name>`.
    Host {
        /// i64 value arguments (1 or 2), NOT counting the prepended `host_ctx` word.
        arity: u8,
        /// Whether a `host_ctx` word is prepended.
        ctx: bool,
    },
}

/// `HelperKind` mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperName {
    /// `otter_binop` — `(i64, i64, i32, i32) -> i64`.
    Binop,
    /// `otter_convert` — `(i64, i32, i32) -> i64`.
    Convert,
    /// `otter_unop` — `(i64, i32, i32) -> i64`.
    Unop,
}

/// `AotScratchEntry` mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchEntry {
    /// First byte of the function's scratch run.
    pub addr: u32,
    /// The function's scratch word count (span = 8 × words bytes at addr).
    pub words: u64,
}

/// `AotManifest::scratch_region` mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchRegion {
    /// Region base (inclusive).
    pub base: u32,
    /// Region end (exclusive; u64 — `2^32` is representable and legal).
    pub end: u64,
}

impl ImportKind {
    /// The i64 parameter count of a `Host` import per the canonical O-14 §2.3 mapping:
    /// `(1,false)→1` (HOST1_TY), `(2,false)/(1,true)→2` (HOST2_TY), `(2,true)→3` (HOSTCTX2_TY).
    /// Errors on any other shape — a manifest error is never downgraded to a gate pass.
    pub fn host_param_count(&self) -> Result<usize, LinkError> {
        match self {
            ImportKind::Host {
                arity: arity @ (1 | 2),
                ctx,
            } => Ok(usize::from(*arity) + usize::from(*ctx)),
            ImportKind::Host { arity, ctx } => Err(LinkError::Manifest(format!(
                "host import with arity {arity} (ctx={ctx}) is outside the canonical \
                 HOST1/HOST2/HOSTCTX2 space (O-14 §2.3)"
            ))),
            _ => Err(LinkError::Manifest(
                "host_param_count called on a non-host import kind".into(),
            )),
        }
    }
}

impl TryFrom<&AotManifest> for ManifestSidecar {
    type Error = LinkError;

    /// Producer-side bridge (§3.1): serialize the LIVE `AotManifest` through this crate's
    /// types. Refuses unknown future variants of the `#[non_exhaustive]` kind enums instead of
    /// guessing a label.
    fn try_from(m: &AotManifest) -> Result<Self, Self::Error> {
        // O-19 §2.5 [결정]: the link/sidecar chain is wasm32-only until its external blockers
        // clear (wasi-libc wasm64 absence — the same root as the L2 block), so an A64 manifest
        // is REFUSED here outright: producing a v1 sidecar for it would silently DROP the
        // memory64 flag (the schema has no field for it yet — C5 adds the field + the
        // record==live cross-check; this explicit refusal is the fail-closed minimum until
        // then, and it IS the "emit-aot A64 sidecar → explicit error" seam, since emit-aot
        // serializes through this very bridge).
        if m.memory64 {
            return Err(LinkError::Manifest(
                "memory64 (A64) artifact manifests cannot become v1 sidecars — the link/sidecar \
                 chain is wasm32-only (O-19 §2.5)"
                    .into(),
            ));
        }
        // Same posture for the address fields: the v1 schema keeps its wasm32 (`u32`) widths,
        // so an address that does not fit u32 (possible since the emitter's O-19 C1 u64
        // widening) is refused instead of silently truncated. Schema widening is the day the
        // link chain opens to memory64, not today.
        let narrow = |what: &str, v: u64| -> Result<u32, LinkError> {
            u32::try_from(v).map_err(|_| {
                LinkError::Manifest(format!(
                    "{what} {v:#x} does not fit the v1 (wasm32) sidecar schema — the \
                     link/sidecar chain is wasm32-only (O-19 §2.5)"
                ))
            })
        };
        let mut imports = Vec::with_capacity(m.imports.len());
        for imp in &m.imports {
            let kind = match imp.kind {
                AotImportKind::Memory => ImportKind::Memory,
                // HelperKind is exhaustive (unlike AotImportKind) — a new helper variant is a
                // compile error here, which IS the lockstep signal.
                AotImportKind::Helper(h) => ImportKind::Helper {
                    helper: match h {
                        HelperKind::Binop => HelperName::Binop,
                        HelperKind::Convert => HelperName::Convert,
                        HelperKind::Unop => HelperName::Unop,
                    },
                },
                AotImportKind::Host { arity, ctx } => ImportKind::Host { arity, ctx },
                ref other => {
                    return Err(LinkError::Manifest(format!(
                        "unknown AotImportKind variant {other:?} — schema module must be \
                         extended in lockstep"
                    )));
                }
            };
            imports.push(ImportEntry {
                module: imp.module.clone(),
                name: imp.name.clone(),
                kind,
            });
        }
        Ok(ManifestSidecar {
            entry_slot: m.entry_slot,
            nfuncs: m.nfuncs,
            helper_slots: m.helper_slots,
            host_slots: m.host_slots.clone(),
            imports,
            scratch: m
                .scratch
                .iter()
                .map(|s| {
                    Ok(ScratchEntry {
                        addr: narrow("scratch addr", s.addr)?,
                        words: s.words as u64,
                    })
                })
                .collect::<Result<Vec<_>, LinkError>>()?,
            scratch_region: ScratchRegion {
                base: narrow("scratch_region base", m.scratch_region.0)?,
                end: m.scratch_region.1,
            },
            table_size: m.table_size,
            shared_memory: m.shared_memory,
            // Always false HERE (the refusal above fired on any A64 manifest) — carried as a
            // field write anyway so the record==live mirror is total, never inferred.
            memory64: m.memory64,
            emitter_version: m.emitter_version.to_string(),
        })
    }
}

/// Deserialize a manifest sidecar from JSON bytes (fail-closed: unknown fields refuse).
pub fn manifest_from_json(bytes: &[u8]) -> Result<ManifestSidecar, LinkError> {
    serde_json::from_slice(bytes).map_err(|e| LinkError::Manifest(e.to_string()))
}

/// Serialize a manifest sidecar to pretty JSON (deterministic field order = struct order).
pub fn manifest_to_json(m: &ManifestSidecar) -> Result<Vec<u8>, LinkError> {
    let mut v = serde_json::to_vec_pretty(m).map_err(|e| LinkError::Manifest(e.to_string()))?;
    v.push(b'\n');
    Ok(v)
}

/// The b′ LIBRARY sidecar (`otter_helpers.manifest.json` — design §2-2, field list verbatim).
///
/// Produced in two stages (§2-2 generation order, panel C-S2): ① `lib-manifest --extract` on
/// the PRE-strip artifact (heap_floor components + exports need the name section — §8-6) with
/// `artifact_sha256` still `null`; ② after `wasm-tools strip`, `lib-manifest --finalize`
/// computes the SHA-256 of the FINAL master bytes (the very bytes the link chain consumes) and
/// binds it. Consumers (`inject-marker`, `check --scratch`) REFUSE an unfinalized sidecar —
/// `artifact_sha256` is the §4.3 content-hash truth; the version fields are record-only
/// (§4.3: at repo version `0.0.0` version comparison has zero discriminating power).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibrarySidecar {
    /// §2-2 formula (panel M1): `max(__stack_pointer initial, max active-data end,
    /// memory_min × 65536)` — the `memory_min` term absorbs the passive-segment blind spot
    /// (passive segments carry no static address; wasm-ld still reflects the placed image in
    /// the memory min). Injected markers excluded (extract runs pre-injection by §2-2 order).
    pub heap_floor: u64,
    /// Whether the b′ memory is shared (+atomics profile).
    pub shared: bool,
    /// Declared memory min, in 64KiB pages.
    pub memory_min: u64,
    /// Declared memory max, in 64KiB pages (`None` = no max; shared always has one).
    pub memory_max: Option<u64>,
    /// Export names, export-section order.
    pub exports: Vec<String>,
    /// Record-only (§4.3 — zero discriminating power at repo version 0.0.0).
    pub otter_version: String,
    /// Record-only (§4.3 — no artifact-side counterpart field, comparison predicate undefined).
    pub rustc_version: String,
    /// SHA-256 (lowercase hex) of the strip-final master bytes — the §4.3 content-binding
    /// truth. `null` until `--finalize`; every consumer refuses `null` (fail-closed).
    pub artifact_sha256: Option<String>,
}

/// Deserialize a library sidecar from JSON bytes (fail-closed: unknown fields refuse).
pub fn library_from_json(bytes: &[u8]) -> Result<LibrarySidecar, LinkError> {
    serde_json::from_slice(bytes).map_err(|e| LinkError::Manifest(e.to_string()))
}

/// Serialize a library sidecar to pretty JSON (deterministic field order = struct order).
pub fn library_to_json(m: &LibrarySidecar) -> Result<Vec<u8>, LinkError> {
    let mut v = serde_json::to_vec_pretty(m).map_err(|e| LinkError::Manifest(e.to_string()))?;
    v.push(b'\n');
    Ok(v)
}

impl LibrarySidecar {
    /// The finalized content hash, or a loud [`LinkError::Pair`] if the sidecar was never
    /// `--finalize`d (consuming an unfinalized sidecar would mean trusting an unbound floor).
    pub fn required_sha(&self) -> Result<&str, LinkError> {
        self.artifact_sha256.as_deref().ok_or_else(|| {
            LinkError::Pair(
                "library sidecar is UNFINALIZED (artifact_sha256 is null) — run \
                 `lib-manifest --finalize` on the strip-final artifact first (§2-2 order)"
                    .into(),
            )
        })
    }
}
