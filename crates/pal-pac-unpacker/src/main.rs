use clap::{Parser, ValueEnum};
use encoding_rs::{Encoding, GBK, SHIFT_JIS, UTF_8};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

const BUCKET_TABLE_OFFSET: usize = 0x0c;
const BUCKET_TABLE_SIZE: usize = 0x7f8;
const RECORD_BASE: usize = BUCKET_TABLE_OFFSET + BUCKET_TABLE_SIZE; // 0x804
const BUCKET_COUNT: usize = 255;
const RECORD_SIZE: usize = 40;
const NAME_SIZE: usize = 32;
const ENCRYPTED_MARKER: u8 = b'$';
const CRYPTO_HEADER_SIZE: usize = 0x10;

#[derive(Debug, Parser)]
#[command(name = "pal-pac-unpacker")]
#[command(about = "Unpack PAL .pac archives.")]
struct Args {
    #[arg(long)]
    input: PathBuf,

    #[arg(long)]
    output: PathBuf,

    #[arg(long, value_enum, default_value_t = Nls::Sjis)]
    nls: Nls,

    #[arg(long)]
    overwrite: bool,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    decrypt: bool,

    #[arg(long)]
    list_only: bool,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, ValueEnum)]
enum Nls {
    Sjis,
    Gbk,
    Utf8,
    Raw,
}

#[derive(Debug, Clone)]
struct PacEntry {
    bucket: usize,
    record_index: u32,
    raw_name: [u8; NAME_SIZE],
    display_name: String,
    size: u32,
    offset: u32,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse();
    let pac = fs::read(&args.input)
        .map_err(|e| format!("failed to read {}: {e}", args.input.display()))?;

    let entries = parse_pac(&pac, args.nls)?;
    if entries.is_empty() {
        return Err("no entries found in pac archive".to_string());
    }

    if args.list_only {
        for entry in &entries {
            println!(
                "bucket={:02X} record={} offset=0x{:08X} size=0x{:08X} name={}",
                entry.bucket, entry.record_index, entry.offset, entry.size, entry.display_name
            );
        }
        return Ok(());
    }

    fs::create_dir_all(&args.output).map_err(|e| {
        format!(
            "failed to create output directory {}: {e}",
            args.output.display()
        )
    })?;

    let mut written = 0usize;
    let mut decrypted = 0usize;

    for entry in &entries {
        let start = entry.offset as usize;
        let end = start
            .checked_add(entry.size as usize)
            .ok_or_else(|| format!("entry size overflows for {}", entry.display_name))?;
        if end > pac.len() {
            return Err(format!(
                "entry {} points outside archive: offset=0x{:08X} size=0x{:08X} pac_size=0x{:X}",
                entry.display_name,
                entry.offset,
                entry.size,
                pac.len()
            ));
        }

        let mut data = pac[start..end].to_vec();
        let was_decrypted = if args.decrypt && is_pal_encrypted_resource(&data) {
            decrypt_pal_dollar_resource(&mut data)?;
            true
        } else {
            false
        };

        let out_path = safe_output_path(&args.output, &entry.display_name)?;
        if out_path.exists() && !args.overwrite {
            return Err(format!(
                "output file already exists: {} (use --overwrite to replace)",
                out_path.display()
            ));
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
        write_file_atomic(&out_path, &data)
            .map_err(|e| format!("failed to write {}: {e}", out_path.display()))?;

        written += 1;
        if was_decrypted {
            decrypted += 1;
        }
        println!(
            "{}{}",
            out_path.display(),
            if was_decrypted { " [decrypted]" } else { "" }
        );
    }

    eprintln!(
        "done: wrote {} file(s), decrypted {} '$' resource(s)",
        written, decrypted
    );
    Ok(())
}

fn parse_pac(bytes: &[u8], nls: Nls) -> Result<Vec<PacEntry>, String> {
    if bytes.len() < RECORD_BASE {
        return Err(format!(
            "file too small for PAL pac header/table: size=0x{:X}, need at least 0x{:X}",
            bytes.len(),
            RECORD_BASE
        ));
    }

    let mut entries = Vec::new();
    let mut seen_records: BTreeMap<u32, usize> = BTreeMap::new();

    for bucket in 0..BUCKET_COUNT {
        let off = BUCKET_TABLE_OFFSET + bucket * 8;
        let first = read_u32_le(bytes, off)?;
        let count = read_u32_le(bytes, off + 4)?;
        if count == 0 {
            continue;
        }

        for rel in 0..count {
            let record_index = first
                .checked_add(rel)
                .ok_or_else(|| format!("record index overflow in bucket {bucket}"))?;
            if seen_records.insert(record_index, bucket).is_some() {
                continue;
            }
            let record_off = RECORD_BASE
                .checked_add(record_index as usize * RECORD_SIZE)
                .ok_or_else(|| format!("record offset overflow for record {record_index}"))?;
            if record_off + RECORD_SIZE > bytes.len() {
                return Err(format!(
                    "record {} from bucket {:02X} is outside archive: record_off=0x{:X}, pac_size=0x{:X}",
                    record_index,
                    bucket,
                    record_off,
                    bytes.len()
                ));
            }

            let mut raw_name = [0u8; NAME_SIZE];
            raw_name.copy_from_slice(&bytes[record_off..record_off + NAME_SIZE]);
            let size = read_u32_le(bytes, record_off + 0x20)?;
            let offset = read_u32_le(bytes, record_off + 0x24)?;
            let display_name = decode_pac_name(&raw_name, nls);
            if display_name.is_empty() {
                return Err(format!("empty filename in record {record_index}"));
            }

            entries.push(PacEntry {
                bucket,
                record_index,
                raw_name,
                display_name,
                size,
                offset,
            });
        }
    }

    entries.sort_by_key(|entry| entry.offset);
    Ok(entries)
}

fn read_u32_le(bytes: &[u8], off: usize) -> Result<u32, String> {
    let chunk = bytes
        .get(off..off + 4)
        .ok_or_else(|| format!("u32 read outside input at offset 0x{off:X}"))?;
    Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn decode_pac_name(raw: &[u8; NAME_SIZE], nls: Nls) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let name = &raw[..end];
    match nls {
        Nls::Raw => name
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(""),
        Nls::Sjis => decode_with_encoding(name, SHIFT_JIS),
        Nls::Gbk => decode_with_encoding(name, GBK),
        Nls::Utf8 => decode_with_encoding(name, UTF_8),
    }
}

fn decode_with_encoding(bytes: &[u8], encoding: &'static Encoding) -> String {
    let (decoded, had_errors) = encoding.decode_without_bom_handling(bytes);
    if had_errors {
        bytes
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join("")
    } else {
        decoded.into_owned()
    }
}

fn safe_output_path(root: &Path, display_name: &str) -> Result<PathBuf, String> {
    let normalized = display_name.replace('\\', "/");
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        return Err(format!(
            "absolute path in pac entry is not allowed: {display_name}"
        ));
    }

    let mut out = root.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "path traversal in pac entry is not allowed: {display_name}"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "invalid path component in pac entry: {display_name}"
                ));
            }
        }
    }
    Ok(out)
}

fn is_pal_encrypted_resource(data: &[u8]) -> bool {
    data.first().copied() == Some(ENCRYPTED_MARKER) && data.len() >= CRYPTO_HEADER_SIZE
}

fn decrypt_pal_dollar_resource(data: &mut [u8]) -> Result<(), String> {
    if data.len() < CRYPTO_HEADER_SIZE {
        return Err("encrypted PAL resource is shorter than 0x10-byte header".to_string());
    }

    let payload_len = data.len() - CRYPTO_HEADER_SIZE;
    let aligned_len = payload_len & !3;
    let payload = &mut data[CRYPTO_HEADER_SIZE..CRYPTO_HEADER_SIZE + aligned_len];
    let mut rotate = 4u32;

    for chunk in payload.chunks_exact_mut(4) {
        chunk[0] = chunk[0].rotate_left(rotate & 7);
        rotate = rotate.wrapping_add(1);

        let mut value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        value ^= 0x084D_F873;
        value ^= 0xFF98_7DEE;
        chunk.copy_from_slice(&value.to_le_bytes());
    }

    Ok(())
}

fn write_file_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("out")
    ));
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(data)?;
        file.flush()?;
    }
    fs::rename(tmp, path)
}
