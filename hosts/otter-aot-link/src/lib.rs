//! otter-aot-link — the b″ link driver's library half (O-16c′ design §3.1; L-1a increment).
//!
//! L-1a surface (design §7 L-1a row):
//! - [`rename`]: general mapping-table rename over import module strings / import field names /
//!   export names (import + export modes — §2-6/§6.1), including the kebab-case conversion with
//!   its three fail-closed rejections (§6.1), plus the tool's own post-rename invariance
//!   assertion and a wasmparser well-formedness validation of the product.
//! - [`invariance::assert_invariance`]: the §2-6 per-mode invariance gate — import mode may
//!   change ONLY the import section's bytes, export mode ONLY the export section's bytes; every
//!   other section (customs included) must be byte-identical.
//! - [`imports::check_imports`]: the §5.1 residual-import gate — the linked artifact's core
//!   import set must EQUAL (names AND types, both directions) the manifest's `otter_host` entry
//!   set; anything else (`otter.memory`, `otter_helpers.*`, …) is a violation. `⊆` was rejected
//!   (panel G-SF-3): a proper subset means a LOST import — silent-pass would violate
//!   fail-closed. `--require-self-contained` is the empty-set emphasis mode (§2-4).
//! - [`schema`]: the JSON sidecar types this crate SOLELY owns (§0-7/§3.1) — both the
//!   artifact-manifest sidecar (`check --imports`) and, since L-1b, the b′ library sidecar
//!   (`otter_helpers.manifest.json` — heap_floor/artifact_sha256/…, §2-2).
//!
//! L-1b surface (design §7 L-1b row):
//! - [`lib_manifest`]: `lib-manifest --extract` (pre-strip: heap_floor components + exports —
//!   name section required, §8-6 loud-failure fallback) and `--finalize` (post-strip: binds
//!   `artifact_sha256` over the FINAL master bytes — §2-2 generation order, §4.3 content-hash
//!   truth).
//! - [`marker`]: `inject-marker` — the §2-1 width-branched scratch markers (W≥16 two magics /
//!   W==8 merged `OTTRSCRM` / W==0 empty-region no-op branch), the CONSTRUCTIVE memory-min
//!   raise, and the fail-closed input-SHA pair comparison.
//! - [`scratch`]: `check --scratch` — the §5.2 static checks (i) marker match + non-marker
//!   `∩ region = ∅`, (ii) coverage `min ≥ ⌈end/65536⌉`, (iii) library floor
//!   `base ≥ sidecar.heap_floor` (the §2-2 formula includes the `memory_min×65536` term —
//!   the passive-segment blind-spot absorber, panel M1).
//!
//! L-2 hardening (the L-1b review's four carried-over prescriptions, folded in L-2 — each
//! with a test in `tests/l1b_gates.rs`): (a) the W==0 ∩-exemption in [`scratch`] (empty-region
//! false-red), (b)+(c) memory64 + custom-page-size refusal at the COMMON
//! [`facts::ModuleFacts::owned_memory`] seam (covers extract/finalize/inject/check in one
//! wiring point; inject-marker's memory re-encode now carries `memory64` faithfully instead of
//! hardcoding `false`), (d) the finalize re-bind refusal branch is test-covered.
//!
//! L-4 surface (design §7 L-4 row — `shrink-exports`/`check --linked` moved here from L-1,
//! panel G-SF-5: the consumer, a real linked artifact, first exists in L-4):
//! - [`linked`]: `check --linked` — the §5.3 semantic-level post-link checks (single memory
//!   definition, `otter_entry`/`otter_table` exports, declared-table + element slot shape,
//!   machine EH census, and the §6.3 profile-split start check with the init-only whitelist).
//! - [`shrink`]: `shrink-exports` — the §5.4 keep-list export reduction with export-mode
//!   invariance self-assertion.
//!
//! L-5 surface (design §7 L-5 row):
//! - [`component`]: `check --component` — the §6.2 NATIVE component checks (wasmparser
//!   `component-model` feature, explicit in Cargo.toml): EH-free-feature validation of the
//!   whole component, empty-world detection (the E10 trap — core exports are never
//!   auto-promoted), world-shape exact match (export = `otter-entry` func; imports = the
//!   kebab image of the manifest's `otter_host` set, both directions), and the nested-core-
//!   module EH census (operator-set shared with [`linked`]).
//!
//! Still deferred by design: the wasmtime instantiation assertions live in otter-aot-e2e's
//! L-4 modes (§0-5: this crate stays wasmtime-free — wasmparser Validator only), and the
//! component EXECUTION differential (host wiring included) is otter-aot-e2e's
//! `run-component` mode (L-5).
//!
//! Everything here is deterministic and network-free (§3.1).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub mod component;
pub mod facts;
pub mod imports;
pub mod invariance;
pub mod lib_manifest;
pub mod linked;
pub mod marker;
pub mod rename;
pub mod schema;
pub mod scratch;
pub mod section;
pub mod shrink;

/// Unified error type for every L-1a gate/operation. Every variant is a LOUD, attributed
/// failure — no warnings, no fallback continuation (the fail-closed discipline of SPK §4-1:
/// the chain's external tool, wasm-merge, silently part-links, so every check this crate owns
/// must refuse instead of degrade).
#[derive(Debug)]
pub enum LinkError {
    /// Input is not a core wasm module / has malformed section framing.
    Malformed(String),
    /// `rename` mapping-table problems: empty mapping, key matching nothing, mode misuse.
    RenameMapping(String),
    /// Kebab fail-closed rejection ① (§6.1): a source name subject to kebab conversion
    /// contains `-` (reverse-uniqueness of `_`→`-` would be lost).
    KebabDashSource(String),
    /// Kebab/rename fail-closed rejection ② (§6.1): two names collide after substitution
    /// (duplicate export names are also invalid wasm — refuse loudly, not via the validator).
    RenameCollision(String),
    /// Kebab fail-closed rejection ③ (§6.1): the substitution result is not a valid WIT
    /// identifier (uppercase, non-ASCII, leading digit, empty word, …).
    KebabInvalidIdent(String),
    /// §2-6 invariance gate violation: a section outside the mode's allowed section changed.
    Invariance(String),
    /// §5.1 residual-import gate violation(s) — all violations, deterministically ordered.
    Imports(Vec<String>),
    /// The manifest sidecar itself is unusable (unknown kind, arity out of range, duplicate
    /// host names) — a manifest error is NEVER downgraded to a gate pass.
    Manifest(String),
    /// wasmparser validation of a rename product failed (well-formedness only — the EH-census
    /// and semantic checks are `check --linked`'s, L-4).
    Validate(String),
    /// §5.2 scratch gate violation(s) — all violations, deterministically ordered.
    Scratch(Vec<String>),
    /// `inject-marker` failure (§2-1): required min exceeds the declared max, section surgery
    /// impossible, self-assertion failed, ….
    Inject(String),
    /// `lib-manifest --extract` failure (§2-2/§8-6): stack-pointer identification via the name
    /// section failed — the largest-mutable-i32 heuristic is deliberately a FAILURE, never a
    /// silent fallback (extract runs on the PRE-strip artifact by contract).
    Extract(String),
    /// Sidecar↔artifact pair violation (§4.3 content-hash truth): SHA-256 mismatch, an
    /// unfinalized sidecar (`artifact_sha256` still null), or sidecar metadata that contradicts
    /// the artifact it claims to describe. Fail-closed, NO bypass flag.
    Pair(String),
    /// §5.3 `check --linked` semantic violation(s) — all violations, deterministically ordered.
    Linked(Vec<String>),
    /// `shrink-exports` failure (§5.4): empty keep list, keep name matching no export, no
    /// export section.
    Shrink(String),
    /// §6.2 `check --component` violation(s) — all violations, deterministically ordered
    /// (empty world, world-shape mismatch, EH census, non-component input).
    Component(Vec<String>),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Malformed(m) => write!(f, "malformed wasm: {m}"),
            LinkError::RenameMapping(m) => write!(f, "rename mapping error: {m}"),
            LinkError::KebabDashSource(m) => {
                write!(f, "kebab rejection (source contains '-'): {m}")
            }
            LinkError::RenameCollision(m) => {
                write!(f, "rename rejection (post-substitution collision): {m}")
            }
            LinkError::KebabInvalidIdent(m) => {
                write!(f, "kebab rejection (not a valid WIT identifier): {m}")
            }
            LinkError::Invariance(m) => write!(f, "post-rename invariance violation: {m}"),
            LinkError::Imports(v) => {
                write!(f, "residual-import gate violations ({}):", v.len())?;
                for line in v {
                    write!(f, "\n  - {line}")?;
                }
                Ok(())
            }
            LinkError::Manifest(m) => write!(f, "manifest sidecar error: {m}"),
            LinkError::Validate(m) => write!(f, "validator rejected rename product: {m}"),
            LinkError::Scratch(v) => {
                write!(f, "scratch gate violations ({}):", v.len())?;
                for line in v {
                    write!(f, "\n  - {line}")?;
                }
                Ok(())
            }
            LinkError::Inject(m) => write!(f, "inject-marker failed: {m}"),
            LinkError::Extract(m) => write!(f, "lib-manifest extract failed: {m}"),
            LinkError::Pair(m) => write!(f, "sidecar/artifact pair violation: {m}"),
            LinkError::Linked(v) => {
                write!(f, "linked-artifact gate violations ({}):", v.len())?;
                for line in v {
                    write!(f, "\n  - {line}")?;
                }
                Ok(())
            }
            LinkError::Shrink(m) => write!(f, "shrink-exports failed: {m}"),
            LinkError::Component(v) => {
                write!(f, "component gate violations ({}):", v.len())?;
                for line in v {
                    write!(f, "\n  - {line}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LinkError {}

/// Which section a `rename` invocation is allowed to rewrite (§2-6 mode split).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameMode {
    /// Import mode (§2-6-(a)): only the import section (id 2) may change.
    Import,
    /// Export mode (§2-6-(b), §6.1 kebab): only the export section (id 7) may change.
    Export,
}

impl RenameMode {
    /// The single section id this mode is allowed to rewrite.
    pub fn allowed_section_id(self) -> u8 {
        match self {
            RenameMode::Import => 2,
            RenameMode::Export => 7,
        }
    }

    /// Parse the CLI spelling.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "import" => Some(RenameMode::Import),
            "export" => Some(RenameMode::Export),
            _ => None,
        }
    }
}

/// The general mapping table (§6.1, panel G-SF-2): NO hardcoded module strings — arbitrary
/// old→new pairs, so whatever core module-string convention L-5's host-lift measurement turns
/// out to require costs zero rework here.
#[derive(Clone, Debug, Default)]
pub struct RenameMaps {
    /// Import-module-string mapping (import mode only — export entries have no module string).
    pub modules: BTreeMap<String, String>,
    /// Name mapping: import FIELD names (import mode) or export names (export mode).
    pub names: BTreeMap<String, String>,
    /// §6.1 kebab conversion (`_`→`-`) applied to every export name not explicitly mapped —
    /// export mode only, with the three fail-closed rejections.
    pub kebab: bool,
}

/// `_` → `-` (the ONLY substitution §6.1 allows; injective because sources containing `-`
/// are rejected — that rejection is exactly what keeps the reverse map unique).
pub fn kebab(name: &str) -> String {
    name.replace('_', "-")
}

/// §6.1 machine rule: a valid WIT identifier is `word("-"word)*` with
/// `word = [a-z][a-z0-9]*` — uppercase, non-ASCII, leading digits and empty words are all
/// rejected (the design deliberately narrows WIT's grammar: no uppercase acronym words).
pub fn is_valid_wit_ident(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.split('-').all(|word| {
        let mut chars = word.chars();
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() => chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            _ => false,
        }
    })
}

/// Deterministically format a set of leftover (unmatched) mapping keys.
pub(crate) fn format_keys(keys: &BTreeSet<&str>) -> String {
    keys.iter().copied().collect::<Vec<_>>().join(", ")
}

/// SHA-256 of `bytes` as lowercase hex — the §4.3 content-hash truth format (the same spelling
/// `sha256sum` prints, so recipe-side and crate-side hashes are directly comparable).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
