//! Shared read-only fact extraction over a core module — the common substrate of
//! `lib-manifest` (§2-2), `inject-marker` (§2-1) and `check --scratch` (§5.2). Read-only and
//! deliberately narrow: defined memory shape, defined globals (+ statically evaluable i32
//! inits), exports, data segments (with static spans where they HAVE one — passive segments
//! do not, which is exactly the §2-2 blind spot the `memory_min×65536` floor term absorbs),
//! and name-section global names (pre-strip artifacts only).

use wasmparser::{KnownCustom, Name, Operator, Parser, Payload, TypeRef, ValType};

use crate::LinkError;

/// One defined memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFacts {
    /// Min, in 64KiB pages.
    pub min: u64,
    /// Max, in 64KiB pages (shared memories always declare one).
    pub max: Option<u64>,
    /// Shared (+atomics profile)?
    pub shared: bool,
    /// memory64?
    pub memory64: bool,
    /// Custom page size (log2). [`ModuleFacts::owned_memory`] REFUSES `Some` (L-2 hardening:
    /// the crate's floor/coverage math assumes 64 KiB pages); carried so the refusal can say
    /// what it saw and so any future rewrite path stays faithful.
    pub page_size_log2: Option<u32>,
}

/// One defined global.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalFacts {
    /// Content type.
    pub ty: ValType,
    /// Mutability.
    pub mutable: bool,
    /// Init value IF the init expr is exactly `i32.const N; end` (as unsigned), else `None`.
    pub i32_init: Option<u64>,
}

/// One data segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataFacts {
    /// `Some((memory_index, offset))` for an active segment whose offset expr is exactly
    /// `i32.const N; end`; `Some((idx, ?))` is impossible for other exprs — those yield
    /// `Active { static_offset: None }`; passive segments are `None`.
    pub active: Option<ActiveData>,
    /// Segment payload bytes.
    pub bytes: Vec<u8>,
}

/// Placement facts of an active data segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveData {
    /// Target memory index.
    pub memory_index: u32,
    /// Statically evaluated offset (unsigned), when the offset expr is `i32.const N; end`.
    pub static_offset: Option<u64>,
}

/// Everything the L-1b commands need to know about a module, in one pass.
#[derive(Clone, Debug, Default)]
pub struct ModuleFacts {
    /// Number of IMPORTED memories (a b′ must own its memory: 0 expected).
    pub imported_memories: usize,
    /// Number of IMPORTED globals (offsets the defined-global index space).
    pub imported_globals: usize,
    /// Defined memories, section order.
    pub memories: Vec<MemoryFacts>,
    /// Defined globals, section order (index space starts after `imported_globals`).
    pub globals: Vec<GlobalFacts>,
    /// Export names, section order.
    pub exports: Vec<String>,
    /// Data segments, section order.
    pub data: Vec<DataFacts>,
    /// `(data-count-section present, declared count)`.
    pub data_count: Option<u32>,
    /// Global names from the name section (`(global_index, name)`, subsection order) —
    /// empty on stripped artifacts.
    pub global_names: Vec<(u32, String)>,
}

impl ModuleFacts {
    /// Parse `bytes`. Loud on anything malformed; NOT a validator (callers run wasmparser's
    /// `Validator` where the gate wording demands it).
    pub fn parse(bytes: &[u8]) -> Result<Self, LinkError> {
        let mut facts = ModuleFacts::default();
        for payload in Parser::new(0).parse_all(bytes) {
            let payload = payload.map_err(|e| LinkError::Malformed(e.to_string()))?;
            match payload {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.map_err(|e| LinkError::Malformed(e.to_string()))?;
                        match import.ty {
                            TypeRef::Memory(_) => facts.imported_memories += 1,
                            TypeRef::Global(_) => facts.imported_globals += 1,
                            _ => {}
                        }
                    }
                }
                Payload::MemorySection(reader) => {
                    for mem in reader {
                        let mem = mem.map_err(|e| LinkError::Malformed(e.to_string()))?;
                        facts.memories.push(MemoryFacts {
                            min: mem.initial,
                            max: mem.maximum,
                            shared: mem.shared,
                            memory64: mem.memory64,
                            page_size_log2: mem.page_size_log2,
                        });
                    }
                }
                Payload::GlobalSection(reader) => {
                    for global in reader {
                        let global = global.map_err(|e| LinkError::Malformed(e.to_string()))?;
                        let i32_init = eval_i32_const(&global.init_expr)?;
                        facts.globals.push(GlobalFacts {
                            ty: global.ty.content_type,
                            mutable: global.ty.mutable,
                            i32_init,
                        });
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.map_err(|e| LinkError::Malformed(e.to_string()))?;
                        facts.exports.push(export.name.to_string());
                    }
                }
                Payload::DataCountSection { count, .. } => facts.data_count = Some(count),
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.map_err(|e| LinkError::Malformed(e.to_string()))?;
                        let active = match &data.kind {
                            wasmparser::DataKind::Passive => None,
                            wasmparser::DataKind::Active {
                                memory_index,
                                offset_expr,
                            } => Some(ActiveData {
                                memory_index: *memory_index,
                                static_offset: eval_i32_const(offset_expr)?,
                            }),
                        };
                        facts.data.push(DataFacts {
                            active,
                            bytes: data.data.to_vec(),
                        });
                    }
                }
                Payload::CustomSection(c) => {
                    if let KnownCustom::Name(reader) = c.as_known() {
                        for name in reader {
                            let name = name.map_err(|e| LinkError::Malformed(e.to_string()))?;
                            if let Name::Global(map) = name {
                                for naming in map {
                                    let naming = naming.map_err(|e| LinkError::Malformed(e.to_string()))?;
                                    facts.global_names.push((naming.index, naming.name.to_string()));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(facts)
    }

    /// The single DEFINED memory a memory-owning module must have (b′ and every linked
    /// product — SPK B1 "메모리 소유"). Loud on 0 (memory imported or absent) and on >1.
    ///
    /// L-2 hardening (the L-1b review's carried-over prescriptions, both enforced HERE as the
    /// COMMON wiring point — every consumer path (`lib-manifest --extract`/`--finalize`,
    /// `inject-marker`, `check --scratch`) routes through this method, so one rejection covers
    /// all four (the check-completeness discipline; per-path rejections were the weaker
    /// alternative — judged and rejected as four wiring points for one invariant):
    /// - **memory64 refusal**: outside the O-14 wasm32 contract; before this, only `--extract`
    ///   refused while inject/finalize/check silently proceeded — and inject-marker's memory
    ///   re-encode would have silently REWRITTEN memory64→false (the review's fail-open).
    /// - **custom-page-size refusal** (`page_size_log2 != None`): every floor/coverage formula
    ///   in this crate (`heap_floor`'s `memory_min × 65536` term, coverage's `⌈end/65536⌉`)
    ///   assumes 64 KiB pages — a custom page size makes them silently wrong (fail-open),
    ///   so it is refused loudly. No sanctioned pipeline produces either input.
    pub fn owned_memory(&self, what: &str) -> Result<MemoryFacts, LinkError> {
        if self.imported_memories != 0 {
            return Err(LinkError::Malformed(format!(
                "{what}: module IMPORTS {} memor{} — a memory-owning module must define its \
                 own memory",
                self.imported_memories,
                if self.imported_memories == 1 { "y" } else { "ies" }
            )));
        }
        let memory = match self.memories.as_slice() {
            [one] => *one,
            [] => {
                return Err(LinkError::Malformed(format!(
                    "{what}: module defines no memory (memory-owning module expected)"
                )));
            }
            many => {
                return Err(LinkError::Malformed(format!(
                    "{what}: module defines {} memories — single-memory modules only",
                    many.len()
                )));
            }
        };
        if memory.memory64 {
            return Err(LinkError::Malformed(format!(
                "{what}: memory64 module is outside the O-14 wasm32 contract — refusing \
                 (fail-closed; L-2 hardening: rejected at the common owned_memory seam)"
            )));
        }
        if let Some(log2) = memory.page_size_log2 {
            return Err(LinkError::Malformed(format!(
                "{what}: memory declares a custom page size (2^{log2} B) — every floor/coverage \
                 formula here assumes 64 KiB pages, refusing (fail-closed; L-2 hardening)"
            )));
        }
        Ok(memory)
    }
}

/// Evaluate a const expr that is exactly `i32.const N; end` to `N` (as unsigned); any other
/// shape yields `None` (callers turn that into a LOUD error where static knowledge is
/// mandatory — fail-closed, never a guess).
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
