//! `check --linked` — the §5.3 semantic-level post-link verification (L-4 increment; moved
//! here from L-1 by panel G-SF-5 — the consumer, a real linked artifact, first exists in L-4).
//!
//! Byte gates are IMPOSSIBLE post-link (wasm-merge is a binaryen IR round-trip re-encode —
//! SPK §4-4), so this check is deliberately SEMANTIC, item-for-item the §5.3 list:
//! - single memory DEFINITION (library-owned shape: 0 imported memories, exactly 1 defined —
//!   the L3 "링크 후 메모리 1개, 라이브러리 소유" result as a machine predicate);
//! - **memory sharedness == manifest.shared_memory** (L-6, the L-4 review's carried-over
//!   prescription — defect 2): without this cross-predicate a forged or mispaired manifest
//!   picks its own start-check profile (§6.3 split) — `shared_memory: true` against a
//!   non-shared artifact would tolerate an arbitrary init-shaped start, and `false` against a
//!   shared artifact misreports the profile. The artifact's actual memory declaration is the
//!   ground truth the manifest must match (fail-open closure; O-14 §12.1 sharedness row);
//! - **memory64 == manifest.memory64** (O-19 C5, design §2.5 [v2 — scope-6], isomorphic to the
//!   sharedness predicate above): `defined_memories` used to capture sharedness ONLY, so a
//!   memory64 LINKED artifact's rejection rested on a single layer (`check --scratch`'s
//!   `owned_memory` seam). The linked-side predicate completes the layering: the artifact's
//!   own memory64 bit is the ground truth, and a memory64 artifact × `memory64: false`/OLD
//!   manifest (the `#[serde(default)]` back-compat parse) is refused here — the record==live
//!   cross-check's consumer half (S2-M3 권고). Real products cannot legally reach the true×true
//!   green today (the A64×b′ static link is externally blocked, §2.5) — the predicate exists
//!   for forged/mispaired inputs, fail-closed;
//! - `otter_entry` (func) and `otter_table` (table) exports exist;
//! - exactly one table, `min == max == manifest.table_size` (the declared-table shape of
//!   O-14 §2.5-iii, preserved through the merge);
//! - exactly one ACTIVE element segment on table 0 at offset `i32.const 0` with item count
//!   `== manifest.table_size` (slot identity as far as it is statically checkable — function
//!   indices are renumbered by the merge, values live in the re-numbered space; the execution
//!   differential is the layer that proves the mapping, §5.3);
//! - EH census == 0: no exception-handling OPERATOR anywhere in the code section (both the
//!   current `try_table`/`throw`/`throw_ref` set and the legacy `try`/`catch`/`rethrow`/
//!   `delegate`/`catch_all` set) and no tag section / tag imports — the machine counterpart of
//!   the recipe-level WAT grep;
//! - **start check** (§5.3, panel M3/MF-1): start must be ABSENT on a non-shared linked
//!   artifact (§6.3 profile split), and on a shared one may only be the wasm-ld
//!   `__wasm_init_memory` init shape — every body operator inside the whitelist below, zero
//!   call-class operators. The whitelist keeps "not guest logic, not a code-execution path"
//!   machine-enforced rather than document-trusted.
//!
//! RECORDED DESIGN DEVIATIONS (start whitelist, measured against the real +atomics b′ start
//! body — the design §5.3 list was written from the panel's summary dump, the L-4 full dump
//! shows two more operators, both in the already-whitelisted classes):
//! - `br_table`: the measured `__wasm_init_memory` guard dispatches the cmpxchg result with
//!   `br_table 0 1 2` — pure branch control, same class as the whitelisted `br`/`br_if`
//!   (the load-bearing exclusion — call/call_indirect/return_call — is untouched);
//! - `i64.const`: the `memory.atomic.wait32` timeout operand is `i64.const -1` — a constant,
//!   same class as the whitelisted `i32.const`.
//!
//! `atomic.fence` is NOT whitelisted (the design lists rmw/cmpxchg/store/load/wait/notify
//! only and the measured body has no fence — fail-closed until evidence).

use wasmparser::{ElementItems, ElementKind, Operator, Parser, Payload, TypeRef};

use crate::LinkError;
use crate::schema::ManifestSidecar;

/// What the start section turned out to be (green dispositions only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartDisposition {
    /// No start section (the only green shape on a non-shared profile).
    Absent,
    /// Shared profile: an init-only start passed the whitelist (operator count carried for
    /// the mandated explicit logging).
    InitOnly {
        /// Number of body operators audited (all whitelisted).
        ops: usize,
    },
}

impl StartDisposition {
    /// Human-readable disposition (for the recipe log — the start check's first live run).
    pub fn describe(self) -> String {
        match self {
            StartDisposition::Absent => "start absent".to_string(),
            StartDisposition::InitOnly { ops } => {
                format!("init-only start ({ops} whitelisted op(s), 0 call-class)")
            }
        }
    }
}

/// Green-path summary of a passed `check --linked`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedReport {
    /// The single table's size (== min == max == manifest.table_size).
    pub table_size: u32,
    /// Item count of the single active element segment (== table_size).
    pub element_items: u32,
    /// Export count (informational — shrink-exports shrinks this surface).
    pub exports: usize,
    /// Start disposition.
    pub start: StartDisposition,
    /// The single defined memory's sharedness (== manifest.shared_memory — the L-6
    /// cross-predicate; logged explicitly, never a silent pass).
    pub shared_memory: bool,
    /// The single defined memory's memory64 bit (== manifest.memory64 — the O-19 C5
    /// cross-predicate, same explicit-logging rule).
    pub memory64: bool,
}

/// Everything this check needs from the module, one parse.
#[derive(Default)]
struct LinkedFacts {
    imported_funcs: u32,
    imported_memories: usize,
    imported_tables: usize,
    imported_tags: usize,
    /// Defined memories: `(shared, memory64)` per declaration (the count check uses `len()`;
    /// the single green memory's flags feed the two manifest cross-predicates).
    defined_memories: Vec<(bool, bool)>,
    /// Defined tables: `(min, max)`.
    tables: Vec<(u64, Option<u64>)>,
    /// Element segments: `(is_active_on_table0, static_offset_if_i32const, item_count)`.
    elements: Vec<(bool, Option<u64>, u32)>,
    /// `(name, is_func, is_table)` triples, export-section order.
    exports: Vec<(String, bool, bool)>,
    start: Option<u32>,
    tags: usize,
}

/// Run the §5.3 semantic checks on `artifact` against `manifest`.
pub fn check_linked(artifact: &[u8], manifest: &ManifestSidecar) -> Result<LinkedReport, LinkError> {
    let mut facts = LinkedFacts::default();
    let mut eh_ops: Vec<String> = Vec::new();
    let mut start_body_ops: Option<Vec<Operator<'_>>> = None;

    // Pass 1: sections. Function bodies are audited in place (census for all; the start body
    // is captured for the whitelist audit once its index is known — start section precedes
    // code in a well-formed module, so `facts.start` is already set when bodies arrive).
    let mut body_index: u32 = 0;
    for payload in Parser::new(0).parse_all(artifact) {
        let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
        match payload {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    match import.ty {
                        TypeRef::Func(_) | TypeRef::FuncExact(_) => facts.imported_funcs += 1,
                        TypeRef::Memory(_) => facts.imported_memories += 1,
                        TypeRef::Table(_) => facts.imported_tables += 1,
                        TypeRef::Tag(_) => facts.imported_tags += 1,
                        _ => {}
                    }
                }
            }
            Payload::MemorySection(reader) => {
                for memory in reader {
                    let memory = memory.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    facts.defined_memories.push((memory.shared, memory.memory64));
                }
            }
            Payload::TableSection(reader) => {
                for table in reader {
                    let table = table.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    facts.tables.push((table.ty.initial, table.ty.maximum));
                }
            }
            Payload::TagSection(reader) => facts.tags += reader.count() as usize,
            Payload::ElementSection(reader) => {
                for element in reader {
                    let element = element.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    let (active_t0, offset) = match &element.kind {
                        ElementKind::Active {
                            table_index,
                            offset_expr,
                        } => (
                            matches!(table_index, None | Some(0)),
                            eval_i32_const(offset_expr)?,
                        ),
                        _ => (false, None),
                    };
                    let count = match &element.items {
                        ElementItems::Functions(r) => r.count(),
                        ElementItems::Expressions(_, r) => r.count(),
                    };
                    facts.elements.push((active_t0, offset, count));
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    facts.exports.push((
                        export.name.to_string(),
                        export.kind == wasmparser::ExternalKind::Func,
                        export.kind == wasmparser::ExternalKind::Table,
                    ));
                }
            }
            Payload::StartSection { func, .. } => facts.start = Some(func),
            Payload::CodeSectionEntry(body) => {
                let func_index = facts.imported_funcs + body_index;
                body_index += 1;
                let is_start = facts.start == Some(func_index);
                let mut ops: Vec<Operator<'_>> = Vec::new();
                let reader = body
                    .get_operators_reader()
                    .map_err(|e| LinkError::Malformed(e.to_string()))?;
                for op in reader {
                    let op = op.map_err(|e| LinkError::Malformed(e.to_string()))?;
                    if is_eh_op(&op) {
                        eh_ops.push(format!("func {func_index}: {op:?}"));
                    }
                    if is_start {
                        ops.push(op);
                    }
                }
                if is_start {
                    start_body_ops = Some(ops);
                }
            }
            _ => {}
        }
    }

    let mut violations: Vec<String> = Vec::new();

    // Single memory DEFINITION (library-owned shape).
    if facts.imported_memories != 0 {
        violations.push(format!(
            "linked artifact IMPORTS {} memor{} — the merge must have bound the product's \
             memory import to the b′ definition (library-owned shape, SPK L3)",
            facts.imported_memories,
            if facts.imported_memories == 1 { "y" } else { "ies" }
        ));
    }
    if facts.defined_memories.len() != 1 {
        violations.push(format!(
            "linked artifact defines {} memories — exactly 1 (the b′-owned memory) expected",
            facts.defined_memories.len()
        ));
    }

    // Sharedness cross-predicate (L-6 — the L-4 review's carried-over prescription): the
    // artifact's ACTUAL memory declaration must equal manifest.shared_memory. Without this,
    // the manifest alone selects the §6.3 start-check profile — a forged/mispaired manifest
    // claiming `shared_memory: true` against a non-shared artifact would tolerate an
    // init-shaped start (fail-open), and the reverse misreports the profile (O-14 §12.1:
    // sharedness must match exactly).
    if let [(artifact_shared, artifact_memory64)] = facts.defined_memories.as_slice() {
        if *artifact_shared != manifest.shared_memory {
            violations.push(format!(
                "memory sharedness mismatch: artifact defines a {} memory but manifest.\
                 shared_memory is {} — forged or mispaired manifest (the manifest must not pick \
                 the §6.3 start-check profile on its own; O-14 §12.1 sharedness row)",
                if *artifact_shared { "SHARED" } else { "non-shared" },
                manifest.shared_memory
            ));
        }
        // memory64 cross-predicate (O-19 C5, design §2.5 — the sharedness predicate's addrtype
        // twin): the bytes' own memory64 bit is the ground truth; a memory64 artifact against a
        // `memory64: false` manifest — which is also what an OLD sidecar parses to via
        // `#[serde(default)]` — is a forged/mispaired pair, refused (record==live, S2-M3 권고).
        // The link chain itself is wasm32-only (§2.5), so the only green today is false==false.
        if *artifact_memory64 != manifest.memory64 {
            violations.push(format!(
                "memory64 mismatch: artifact defines a {} memory but manifest.memory64 is {} — \
                 forged or mispaired manifest (the bytes' memory64 bit is the ground truth; \
                 O-19 §2.5 record==live cross-check)",
                if *artifact_memory64 {
                    "memory64 (i64)"
                } else {
                    "wasm32 (i32)"
                },
                manifest.memory64
            ));
        }
    }

    // otter_entry / otter_table exports.
    if !facts.exports.iter().any(|(n, f, _)| n == "otter_entry" && *f) {
        violations.push("`otter_entry` function export missing".to_string());
    }
    if !facts.exports.iter().any(|(n, _, t)| n == "otter_table" && *t) {
        violations.push("`otter_table` table export missing".to_string());
    }

    // Declared table: exactly one, min == max == manifest.table_size.
    let want = u64::from(manifest.table_size);
    if facts.imported_tables != 0 {
        violations.push(format!(
            "linked artifact IMPORTS {} table(s) — the declared table must be a definition",
            facts.imported_tables
        ));
    }
    match facts.tables.as_slice() {
        [(min, max)] => {
            if *min != want || *max != Some(want) {
                violations.push(format!(
                    "table (min {min}, max {max:?}) != declared shape min == max == \
                     manifest.table_size ({want})"
                ));
            }
        }
        other => violations.push(format!(
            "linked artifact defines {} table(s) — exactly 1 declared table expected",
            other.len()
        )),
    }

    // Single active element segment, table 0, offset 0, item count == table_size.
    match facts.elements.as_slice() {
        [(active_t0, offset, count)] => {
            if !active_t0 {
                violations.push(
                    "element segment is not ACTIVE on table 0 (slot-identity shape violated)".to_string(),
                );
            } else if *offset != Some(0) {
                violations.push(format!(
                    "element segment offset {:?} != `i32.const 0` (slot identity requires \
                     offset 0)",
                    offset
                ));
            }
            if u64::from(*count) != want {
                violations.push(format!(
                    "element segment has {count} item(s), manifest.table_size is {want} — \
                     slot coverage incomplete"
                ));
            }
        }
        other => violations.push(format!(
            "linked artifact has {} element segment(s) — exactly 1 active segment expected",
            other.len()
        )),
    }

    // EH census — operators AND tag infrastructure, both 0.
    for line in &eh_ops {
        violations.push(format!("EH operator present: {line}"));
    }
    if facts.tags != 0 || facts.imported_tags != 0 {
        violations.push(format!(
            "tag section present ({} defined, {} imported) — EH infrastructure must be 0",
            facts.tags, facts.imported_tags
        ));
    }

    // Start check (§5.3 / §6.3 profile split).
    let start = match facts.start {
        None => StartDisposition::Absent,
        Some(idx) if !manifest.shared_memory => {
            violations.push(format!(
                "start section present (func {idx}) on a NON-shared linked artifact — §6.3: \
                 non-shared products and linked artifacts have start 0"
            ));
            StartDisposition::Absent // placeholder; violations make this unreachable as a green
        }
        Some(idx) => {
            if idx < facts.imported_funcs {
                violations.push(format!(
                    "start func {idx} is an IMPORTED function — not inspectable, refusing \
                     (fail-closed)"
                ));
                StartDisposition::Absent
            } else {
                match &start_body_ops {
                    None => {
                        violations.push(format!(
                            "start func {idx} has no code body in this module — malformed"
                        ));
                        StartDisposition::Absent
                    }
                    Some(ops) => {
                        let mut bad = 0usize;
                        for op in ops {
                            if !start_whitelisted(op) {
                                bad += 1;
                                violations
                                    .push(format!("start body operator outside the init whitelist: {op:?}"));
                            }
                        }
                        if bad == 0 {
                            StartDisposition::InitOnly { ops: ops.len() }
                        } else {
                            StartDisposition::Absent
                        }
                    }
                }
            }
        }
    };

    if violations.is_empty() {
        Ok(LinkedReport {
            table_size: manifest.table_size,
            element_items: manifest.table_size,
            exports: facts.exports.len(),
            start,
            shared_memory: manifest.shared_memory,
            memory64: manifest.memory64,
        })
    } else {
        Err(LinkError::Linked(violations))
    }
}

/// The §5.3 EH operator set: the current exceptions proposal + the legacy set. Any one of
/// these in any body is a red — the machine counterpart of the recipe's WAT-grep census.
/// (`pub(crate)` since L-5: `check --component` runs the SAME census over the component's
/// nested core modules — one operator-set truth, §6.2.)
pub(crate) fn is_eh_op(op: &Operator<'_>) -> bool {
    matches!(
        op,
        Operator::TryTable { .. }
            | Operator::Throw { .. }
            | Operator::ThrowRef
            | Operator::Try { .. }
            | Operator::Catch { .. }
            | Operator::CatchAll
            | Operator::Rethrow { .. }
            | Operator::Delegate { .. }
    )
}

/// The §5.3 init-start whitelist (+ the two recorded deviations, module doc): control
/// (block/loop/br/br_if/br_table/end), drop, i32/i64.const, global.get/set,
/// memory.init/data.drop, and the atomic rmw/cmpxchg/store/load/wait/notify families.
/// Everything else — every call-class operator above all — is a violation.
fn start_whitelisted(op: &Operator<'_>) -> bool {
    use Operator as O;
    matches!(
        op,
        // control + stack hygiene
        O::Block { .. } | O::Loop { .. } | O::Br { .. } | O::BrIf { .. } | O::BrTable { .. } | O::End
            | O::Drop
            // constants (i64.const: recorded deviation — the atomic-wait timeout operand)
            | O::I32Const { .. } | O::I64Const { .. }
            | O::GlobalGet { .. } | O::GlobalSet { .. }
            | O::MemoryInit { .. } | O::DataDrop { .. }
            // atomics: wait/notify
            | O::MemoryAtomicNotify { .. } | O::MemoryAtomicWait32 { .. } | O::MemoryAtomicWait64 { .. }
            // atomics: load
            | O::I32AtomicLoad { .. } | O::I64AtomicLoad { .. }
            | O::I32AtomicLoad8U { .. } | O::I32AtomicLoad16U { .. }
            | O::I64AtomicLoad8U { .. } | O::I64AtomicLoad16U { .. } | O::I64AtomicLoad32U { .. }
            // atomics: store
            | O::I32AtomicStore { .. } | O::I64AtomicStore { .. }
            | O::I32AtomicStore8 { .. } | O::I32AtomicStore16 { .. }
            | O::I64AtomicStore8 { .. } | O::I64AtomicStore16 { .. } | O::I64AtomicStore32 { .. }
            // atomics: rmw add/sub/and/or/xor/xchg + cmpxchg (all width variants)
            | O::I32AtomicRmwAdd { .. } | O::I64AtomicRmwAdd { .. }
            | O::I32AtomicRmw8AddU { .. } | O::I32AtomicRmw16AddU { .. }
            | O::I64AtomicRmw8AddU { .. } | O::I64AtomicRmw16AddU { .. } | O::I64AtomicRmw32AddU { .. }
            | O::I32AtomicRmwSub { .. } | O::I64AtomicRmwSub { .. }
            | O::I32AtomicRmw8SubU { .. } | O::I32AtomicRmw16SubU { .. }
            | O::I64AtomicRmw8SubU { .. } | O::I64AtomicRmw16SubU { .. } | O::I64AtomicRmw32SubU { .. }
            | O::I32AtomicRmwAnd { .. } | O::I64AtomicRmwAnd { .. }
            | O::I32AtomicRmw8AndU { .. } | O::I32AtomicRmw16AndU { .. }
            | O::I64AtomicRmw8AndU { .. } | O::I64AtomicRmw16AndU { .. } | O::I64AtomicRmw32AndU { .. }
            | O::I32AtomicRmwOr { .. } | O::I64AtomicRmwOr { .. }
            | O::I32AtomicRmw8OrU { .. } | O::I32AtomicRmw16OrU { .. }
            | O::I64AtomicRmw8OrU { .. } | O::I64AtomicRmw16OrU { .. } | O::I64AtomicRmw32OrU { .. }
            | O::I32AtomicRmwXor { .. } | O::I64AtomicRmwXor { .. }
            | O::I32AtomicRmw8XorU { .. } | O::I32AtomicRmw16XorU { .. }
            | O::I64AtomicRmw8XorU { .. } | O::I64AtomicRmw16XorU { .. } | O::I64AtomicRmw32XorU { .. }
            | O::I32AtomicRmwXchg { .. } | O::I64AtomicRmwXchg { .. }
            | O::I32AtomicRmw8XchgU { .. } | O::I32AtomicRmw16XchgU { .. }
            | O::I64AtomicRmw8XchgU { .. } | O::I64AtomicRmw16XchgU { .. } | O::I64AtomicRmw32XchgU { .. }
            | O::I32AtomicRmwCmpxchg { .. } | O::I64AtomicRmwCmpxchg { .. }
            | O::I32AtomicRmw8CmpxchgU { .. } | O::I32AtomicRmw16CmpxchgU { .. }
            | O::I64AtomicRmw8CmpxchgU { .. } | O::I64AtomicRmw16CmpxchgU { .. }
            | O::I64AtomicRmw32CmpxchgU { .. }
    )
}

/// Evaluate a const expr that is exactly `i32.const N; end` (same contract as facts.rs).
fn eval_i32_const(expr: &wasmparser::ConstExpr<'_>) -> Result<Option<u64>, LinkError> {
    let mut ops = expr.get_operators_reader();
    let first = ops.read().map_err(|e| LinkError::Malformed(e.to_string()))?;
    let Operator::I32Const { value } = first else {
        return Ok(None);
    };
    match ops.read() {
        Ok(Operator::End) => {}
        _ => return Ok(None),
    }
    if !ops.eof() {
        return Ok(None);
    }
    Ok(Some(u64::from(value as u32)))
}
