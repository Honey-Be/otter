//! Minimal core-wasm section scanner — the byte-level substrate of both the §2-6 rewrite
//! ("import (or export) section only — every other section byte-identical") and the
//! [`crate::invariance`] gate. Deliberately NOT a general parser: it only frames sections
//! (id + LEB size + payload) so untouched sections can be copied through VERBATIM — going
//! through a re-encoder for untouched sections could silently renormalize size LEBs, which is
//! exactly the drift class the invariance gate exists to refuse.

use std::ops::Range;

use crate::LinkError;

/// One framed section of a core wasm module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionSpan {
    /// Section id byte (0 = custom, 2 = import, 7 = export, …).
    pub id: u8,
    /// Byte range of the WHOLE section (id byte + size LEB + payload).
    pub full: Range<usize>,
    /// Byte range of the payload only.
    pub payload: Range<usize>,
}

const HEADER_LEN: usize = 8;
const MAGIC: [u8; 4] = *b"\0asm";
const VERSION: [u8; 4] = [1, 0, 0, 0];

/// Frame every section of `bytes`. Fails loudly on anything that is not a well-framed core
/// module (truncated LEB, section overruns, wrong magic/version — including the component
/// layer's version tag, which is NOT a core module and must not silently pass here).
pub fn scan_sections(bytes: &[u8]) -> Result<Vec<SectionSpan>, LinkError> {
    if bytes.len() < HEADER_LEN {
        return Err(LinkError::Malformed(format!(
            "{} bytes is shorter than the 8-byte header",
            bytes.len()
        )));
    }
    if bytes[0..4] != MAGIC {
        return Err(LinkError::Malformed("bad magic (not \\0asm)".into()));
    }
    if bytes[4..8] != VERSION {
        return Err(LinkError::Malformed(format!(
            "unsupported version/layer bytes {:02x?} (core module 1 expected)",
            &bytes[4..8]
        )));
    }
    let mut spans = Vec::new();
    let mut off = HEADER_LEN;
    while off < bytes.len() {
        let start = off;
        let id = bytes[off];
        off += 1;
        let (size, size_len) = decode_leb_u32(bytes, off)
            .map_err(|m| LinkError::Malformed(format!("section id {id} at offset {start}: {m}")))?;
        off += size_len;
        let payload_start = off;
        let payload_end = payload_start.checked_add(size as usize).ok_or_else(|| {
            LinkError::Malformed(format!("section id {id} at offset {start}: size overflow"))
        })?;
        if payload_end > bytes.len() {
            return Err(LinkError::Malformed(format!(
                "section id {id} at offset {start}: payload ends at {payload_end}, module is {} bytes",
                bytes.len()
            )));
        }
        spans.push(SectionSpan {
            id,
            full: start..payload_end,
            payload: payload_start..payload_end,
        });
        off = payload_end;
    }
    Ok(spans)
}

/// Decode an unsigned LEB128 u32 at `off`; returns (value, encoded length).
fn decode_leb_u32(bytes: &[u8], off: usize) -> Result<(u32, usize), String> {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    for i in 0..5 {
        let Some(&b) = bytes.get(off + i) else {
            return Err("truncated LEB128 size".into());
        };
        let low = u32::from(b & 0x7f);
        if shift == 28 && (b & 0x70) != 0 {
            return Err("LEB128 u32 overflow".into());
        }
        result |= low << shift;
        if b & 0x80 == 0 {
            return Ok((result, i + 1));
        }
        shift += 7;
    }
    Err("LEB128 u32 longer than 5 bytes".into())
}
