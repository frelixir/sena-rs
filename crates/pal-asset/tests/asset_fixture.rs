use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pal_asset::{make_pac_key, parse_archive_dat_paths, Nls, PacArchive, ResourceManager};

#[test]
fn archive_dat_parser_matches_pal_delimiters() {
    let bytes = b" data | bg\r\n| fgimage\t |";
    let paths = parse_archive_dat_paths(bytes, Nls::ShiftJis).unwrap();
    assert_eq!(paths, vec!["data", "bg", "fgimage"]);
}

#[test]
fn pac_archive_reads_bucketed_entry() {
    let dir = temp_dir("pal_asset_pac");
    let pac_path = dir.join("data.pac");
    let key = make_pac_key("script.src", Nls::ShiftJis).unwrap();
    fs::write(&pac_path, make_one_entry_pac(key, b"Sv20fixture")).unwrap();

    let pac = PacArchive::from_file(&pac_path).unwrap();
    let data = pac.read_key(&key).unwrap().unwrap();
    assert_eq!(data, b"Sv20fixture");

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn resource_manager_bootstraps_archive_dat_and_opens_from_pac() {
    let dir = temp_dir("pal_asset_resource");
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("data").join("archive.dat"), b"data|bg|").unwrap();

    let key = make_pac_key("script.src", Nls::ShiftJis).unwrap();
    fs::write(dir.join("data.pac"), make_one_entry_pac(key, b"Sv20script")).unwrap();

    let mut manager = ResourceManager::bootstrap(&dir, Nls::ShiftJis).unwrap();
    let asset = manager.open("script.src").unwrap();
    assert_eq!(asset.bytes, b"Sv20script");
    assert_eq!(manager.paths(), &["data".to_string(), "bg".to_string()]);

    fs::remove_dir_all(dir).unwrap();
}

fn make_one_entry_pac(key: [u8; 32], payload: &[u8]) -> Vec<u8> {
    const TABLE_OFF: usize = 0x0C;
    const TABLE_SIZE: usize = 255 * 8;
    const RECORD_BASE: usize = TABLE_OFF + TABLE_SIZE;
    const RECORD_SIZE: usize = 40;

    let data_offset = RECORD_BASE + RECORD_SIZE;
    let mut bytes = vec![0u8; data_offset];

    let bucket = key[0] as usize;
    let bucket_off = TABLE_OFF + bucket * 8;
    bytes[bucket_off..bucket_off + 4].copy_from_slice(&0u32.to_le_bytes());
    bytes[bucket_off + 4..bucket_off + 8].copy_from_slice(&1u32.to_le_bytes());

    bytes[RECORD_BASE..RECORD_BASE + 32].copy_from_slice(&key);
    bytes[RECORD_BASE + 32..RECORD_BASE + 36]
        .copy_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes[RECORD_BASE + 36..RECORD_BASE + 40].copy_from_slice(&(data_offset as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{}_{}", prefix, nonce));
    fs::create_dir_all(&path).unwrap();
    path
}
