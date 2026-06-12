//! Asset loading — from a PAC archive or from explicit file paths.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use pal_asset::{decrypt_pal_dollar_file, make_pac_key, Nls, PacArchive};

use crate::dat::{parse_file_dat, parse_mem_dat, parse_text_dat};

/// All raw byte data needed for decompilation.
pub struct ScriptAssets {
    pub script_bytes: Vec<u8>,
    pub point_bytes: Vec<u8>,
    /// Raw (decrypted) FILE.DAT bytes — for signature-driven resource resolution.
    pub file_bytes: Vec<u8>,
    /// Raw (decrypted) TEXT.DAT bytes — for signature-driven text resolution.
    pub text_bytes: Vec<u8>,
    /// Parsed FILE.DAT entries (NLS-decoded) — for legacy LiteralSlot resolver.
    pub file_entries: Vec<String>,
    /// Parsed TEXT.DAT entries — for legacy resolver.
    pub text_entries: Vec<(u32, String)>,
    pub mem_values: Vec<i32>,
}

impl ScriptAssets {
    /// Load from a single PAC archive (reads SCRIPT.SRC, POINT.DAT, FILE.DAT, TEXT.DAT, MEM.DAT).
    pub fn from_pac(path: &Path, nls: Nls) -> Result<Self> {
        let pac =
            PacArchive::from_file(path).with_context(|| format!("open PAC: {}", path.display()))?;

        let script_bytes =
            read_pac_key(&pac, "SCRIPT.SRC", nls, false).context("read SCRIPT.SRC")?;

        let mut point_bytes = read_pac_key(&pac, "$POINT.DAT", nls, true)
            .or_else(|_| read_pac_key(&pac, "POINT.DAT", nls, true))
            .context("read POINT.DAT")?;
        decrypt_pal_dollar_file("POINT.DAT", &mut point_bytes).context("decrypt POINT.DAT")?;

        let mut file_dat = read_pac_key(&pac, "$FILE.DAT", nls, true)
            .or_else(|_| read_pac_key(&pac, "FILE.DAT", nls, true))
            .context("read FILE.DAT")?;
        decrypt_pal_dollar_file("FILE.DAT", &mut file_dat).context("decrypt FILE.DAT")?;

        let mut text_dat = read_pac_key(&pac, "$TEXT.DAT", nls, true)
            .or_else(|_| read_pac_key(&pac, "TEXT.DAT", nls, true))
            .context("read TEXT.DAT")?;
        decrypt_pal_dollar_file("TEXT.DAT", &mut text_dat).context("decrypt TEXT.DAT")?;

        let mut mem_dat = read_pac_key(&pac, "$MEM.DAT", nls, true)
            .or_else(|_| read_pac_key(&pac, "MEM.DAT", nls, true))
            .context("read MEM.DAT")?;
        decrypt_pal_dollar_file("MEM.DAT", &mut mem_dat).context("decrypt MEM.DAT")?;

        let file_entries = parse_file_dat(&file_dat, nls).context("parse FILE.DAT")?;
        let text_entries = parse_text_dat(&text_dat, nls).context("parse TEXT.DAT")?;
        let mem_values = parse_mem_dat(&mem_dat);

        Ok(Self {
            script_bytes,
            point_bytes,
            file_bytes: file_dat,
            text_bytes: text_dat,
            file_entries,
            text_entries,
            mem_values,
        })
    }

    /// Load from explicit file paths (each file is read from disk and decrypted as needed).
    pub fn from_files(
        script_path: &Path,
        point_path: &Path,
        file_dat_path: &Path,
        text_dat_path: &Path,
        mem_dat_path: &Path,
        nls: Nls,
    ) -> Result<Self> {
        let script_bytes =
            fs::read(script_path).with_context(|| format!("read {}", script_path.display()))?;

        let mut point_bytes =
            fs::read(point_path).with_context(|| format!("read {}", point_path.display()))?;
        decrypt_pal_dollar_file("POINT.DAT", &mut point_bytes).context("decrypt POINT.DAT")?;

        let mut file_dat =
            fs::read(file_dat_path).with_context(|| format!("read {}", file_dat_path.display()))?;
        decrypt_pal_dollar_file("FILE.DAT", &mut file_dat).context("decrypt FILE.DAT")?;

        let mut text_dat =
            fs::read(text_dat_path).with_context(|| format!("read {}", text_dat_path.display()))?;
        decrypt_pal_dollar_file("TEXT.DAT", &mut text_dat).context("decrypt TEXT.DAT")?;

        let mut mem_dat =
            fs::read(mem_dat_path).with_context(|| format!("read {}", mem_dat_path.display()))?;
        decrypt_pal_dollar_file("MEM.DAT", &mut mem_dat).context("decrypt MEM.DAT")?;

        let file_entries = parse_file_dat(&file_dat, nls).context("parse FILE.DAT")?;
        let text_entries = parse_text_dat(&text_dat, nls).context("parse TEXT.DAT")?;
        let mem_values = parse_mem_dat(&mem_dat);

        Ok(Self {
            script_bytes,
            point_bytes,
            file_bytes: file_dat,
            text_bytes: text_dat,
            file_entries,
            text_entries,
            mem_values,
        })
    }
}

fn read_pac_key(pac: &PacArchive, name: &str, nls: Nls, _encrypted: bool) -> Result<Vec<u8>> {
    let key = make_pac_key(name, nls).with_context(|| format!("make key for {name}"))?;
    match pac
        .read_key(&key)
        .with_context(|| format!("PAC lookup for {name}"))?
    {
        Some(data) => Ok(data),
        None => bail!("entry '{}' not found in PAC", name),
    }
}
