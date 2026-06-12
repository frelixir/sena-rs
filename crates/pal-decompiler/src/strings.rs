//! String ID resolution and Lua string escaping.

use pal_asset::Nls;

/// Escape a string for use inside Lua double-quoted string literals.
pub fn lua_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x00..=0x1F | 0x7F..=0xFF => {
                out.push('\\');
                out.push_str(&b.to_string());
            }
            _ => out.push(b as char),
        }
    }
    out
}

/// Resolve a string id to a Lua expression using the legacy file_entries table.
///
/// Rules:
///   - `id & 0x10000000 != 0`  → `dynamic_string(0x{id:08X})`
///   - `id == 0x0FFFFFFF`       → `""`
///   - otherwise                → look up FILE.DAT entry at index `id`
pub fn resolve_string_id(
    id: u32,
    file_entries: &[String],
    text_entries: &[(u32, String)],
    nls: Option<Nls>,
) -> String {
    let _ = text_entries;
    let _ = nls;

    if id & 0x1000_0000 != 0 {
        return format!("dynamic_string(0x{id:08X})");
    }
    if id == 0x0FFF_FFFF {
        return "\"\"".to_owned();
    }

    let idx = id as usize;
    if idx < file_entries.len() {
        format!("\"{}\"", lua_escape(&file_entries[idx]))
    } else {
        // Out of range: fallback
        format!("file_str({id})")
    }
}

/// Read a NUL-terminated byte slice, stopping at the first zero byte.
pub fn read_nul_terminated(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&b| b == 0) {
        Some(pos) => &bytes[..pos],
        None => bytes,
    }
}

/// Read the string stored at the given slot in a raw FILE.DAT byte blob.
///
/// File.dat layout:
///   - bytes 0x00..0x0F : 16-byte header
///   - slot i           : offset 0x10 + i * 0x20, NUL-terminated, 0x20 bytes max
pub fn file_slot_string(file_bytes: &[u8], slot: u32, nls: Nls) -> Option<String> {
    const HEADER: usize = 0x10;
    const SLOT_SIZE: usize = 0x20;

    let offset = HEADER + slot as usize * SLOT_SIZE;
    if offset + SLOT_SIZE > file_bytes.len() {
        return None;
    }
    let raw = read_nul_terminated(&file_bytes[offset..offset + SLOT_SIZE]);
    nls.decode(raw).ok()
}

/// Return true if `s` looks like a valid resource name (filename, BGM id, etc.)
pub fn is_plausible_resource_name(s: &str) -> bool {
    if s.is_empty() || s.starts_with('$') || s.contains('*') || s.contains("__3I") {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'#' | b'.'))
}

/// Resolve a parameter typed `ResourceStringFromFileDat` using raw file_bytes.
pub fn resolve_resource_str(file_bytes: &[u8], value: u32, nls: Nls) -> String {
    // 0xFFFFFFFF: signed -1, used as "no resource" / "default slot" sentinel.
    // dynamic_string_index(-1) = Some(0x0FFFFFFF) which is always OOB.
    // Runtime resolve_resource_string returns None → handler treats as no-op.
    if value == 0xFFFF_FFFF {
        return "nil --[[no_resource]]".to_owned();
    }
    if value & 0x1000_0000 != 0 {
        return format!("dynamic_string(0x{value:08X})");
    }
    if value == 0x0FFF_FFFF {
        return "\"\"".to_owned();
    }
    // Try slot rule first
    if let Some(s) = file_slot_string(file_bytes, value, nls) {
        if is_plausible_resource_name(&s) {
            return format!("\"{}\"", lua_escape(&s));
        }
    }
    // Fallback: treat value as a byte offset with NUL-padded string
    let offset = value as usize;
    if offset < file_bytes.len() {
        let raw = read_nul_terminated(&file_bytes[offset..]);
        if let Ok(s) = nls.decode(raw) {
            if is_plausible_resource_name(&s) {
                return format!("\"{}\"", lua_escape(&s));
            }
        }
    }
    // Cannot resolve — output integer with annotation
    format!("{value} --[[resource_id unresolved]]")
}

/// Resolve a parameter typed `TextStringFromTextDat` using raw text_bytes.
///
/// text_bytes is the raw (decrypted) TEXT.DAT content.  `value` is treated
/// as a byte offset into that buffer.  At that offset we expect a
/// `[u32 key][NUL-terminated string]` TEXT record, so the string starts at
/// offset + 4.
pub fn resolve_text_str(text_bytes: &[u8], value: u32, nls: Nls) -> String {
    if value & 0x1000_0000 != 0 {
        return format!("dynamic_string(0x{value:08X})");
    }
    if value == 0x0FFF_FFFF {
        return "\"\"".to_owned();
    }
    let offset = value as usize;
    if offset >= text_bytes.len() {
        return format!("{value} --[[text_id out of range]]");
    }
    // Try offset+4 (past the u32 key field) first
    if offset + 4 <= text_bytes.len() {
        let raw = read_nul_terminated(&text_bytes[offset + 4..]);
        if let Ok(s) = nls.decode(raw) {
            if !s.is_empty() {
                return format!("\"{}\"", lua_escape(&s));
            }
        }
    }
    // Try directly at offset (might already point to string bytes)
    let raw = read_nul_terminated(&text_bytes[offset..]);
    if let Ok(s) = nls.decode(raw) {
        if !s.is_empty() {
            return format!("\"{}\"", lua_escape(&s));
        }
    }
    format!("{value} --[[text_id unresolved]]")
}

/// Decode a NUL-terminated byte slice into a String, tolerating SJIS/GBK via `nls`.
pub fn decode_nul_string(bytes: &[u8], nls: Nls) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    nls.decode(slice).unwrap_or_else(|_| {
        // Fallback: lossy ASCII
        slice
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    b as char
                } else {
                    '?'
                }
            })
            .collect()
    })
}
