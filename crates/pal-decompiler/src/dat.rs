//! Parsers for FILE.DAT, TEXT.DAT, and MEM.DAT.

use anyhow::{bail, Result};
use pal_asset::Nls;

// ─── FILE.DAT ────────────────────────────────────────────────────────────────

/// Parse FILE.DAT and return a Vec of entry strings (one per 0x20-byte slot).
///
/// Layout:
///   - bytes 0x00..0x0F  : 16-byte header (ignored)
///   - entry i at offset : 0x10 + i * 0x20, NUL-terminated, up to 0x20 bytes
///
/// Uses `nls` for proper multibyte decoding instead of `from_utf8_lossy`.
/// If a slot fails to decode, a placeholder comment string is stored.
pub fn parse_file_dat(bytes: &[u8], nls: Nls) -> Result<Vec<String>> {
    const HEADER: usize = 0x10;
    const SLOT: usize = 0x20;

    if bytes.len() < HEADER {
        bail!("FILE.DAT too small: {} bytes", bytes.len());
    }

    let payload = &bytes[HEADER..];
    let n = payload.len() / SLOT;
    let mut entries = Vec::with_capacity(n);

    for i in 0..n {
        let off = i * SLOT;
        let slot = &payload[off..off + SLOT];
        let end = slot.iter().position(|&b| b == 0).unwrap_or(SLOT);
        let s = nls
            .decode(&slot[..end])
            .unwrap_or_else(|_| "--[[decode_error]]".to_owned());
        entries.push(s);
    }

    Ok(entries)
}

// ─── TEXT.DAT ────────────────────────────────────────────────────────────────

/// Parse TEXT.DAT and return a Vec of `(key, text)` pairs.
///
/// Layout:
///   - bytes 0x00..0x0B : magic `$TEXT_LIST__`
///   - bytes 0x0C..0x0F : u32 LE entry count
///   - entries: `[u32 LE key][NUL-terminated string]` repeated
pub fn parse_text_dat(bytes: &[u8], nls: Nls) -> Result<Vec<(u32, String)>> {
    const MAGIC: &[u8] = b"$TEXT_LIST__";

    if bytes.len() < 16 {
        bail!("TEXT.DAT too small: {} bytes", bytes.len());
    }
    if &bytes[0..12] != MAGIC {
        bail!(
            "TEXT.DAT bad magic: got {:?}",
            String::from_utf8_lossy(&bytes[0..12])
        );
    }

    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let mut entries = Vec::with_capacity(count);
    let mut pos = 16usize;

    for i in 0..count {
        if pos + 4 > bytes.len() {
            entries.push((
                0,
                format!("--[[truncated TEXT.DAT at entry {i} pos={pos}]]"),
            ));
            break;
        }
        let key = u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
        pos += 4;

        // Find NUL terminator for the string
        let start = pos;
        while pos < bytes.len() && bytes[pos] != 0 {
            pos += 1;
        }
        let raw = &bytes[start..pos];
        if pos < bytes.len() {
            pos += 1; // skip NUL
        }

        let text = nls
            .decode(raw)
            .unwrap_or_else(|_| format!("--[[decode_error key={key:#010X}]]"));
        entries.push((key, text));
    }

    Ok(entries)
}

/// Look up a text entry by key.
pub fn lookup_text(entries: &[(u32, String)], key: u32) -> Option<&str> {
    entries
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

// ─── MEM.DAT ─────────────────────────────────────────────────────────────────

/// Parse MEM.DAT as a flat array of i32 LE values (includes header).
pub fn parse_mem_dat(bytes: &[u8]) -> Vec<i32> {
    let n = bytes.len() / 4;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = i * 4;
        let v = i32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        out.push(v);
    }
    out
}
