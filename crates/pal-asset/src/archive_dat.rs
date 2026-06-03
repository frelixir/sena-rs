use crate::error::{AssetError, Result};
use crate::nls::Nls;

pub fn parse_archive_dat_paths(bytes: &[u8], nls: Nls) -> Result<Vec<String>> {
    if bytes.is_empty() {
        return Err(AssetError::InvalidArchiveDat {
            reason: "file is empty".to_owned(),
        });
    }

    let compact: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !matches!(*b, b'\r' | b'\n' | b' ' | b'\t' | 0))
        .collect();

    let mut paths = Vec::new();
    for item in compact.split(|&b| b == b'|') {
        if item.is_empty() {
            continue;
        }
        let path = nls.decode(item)?;
        paths.push(path.replace('/', "\\"));
    }

    if paths.is_empty() {
        return Err(AssetError::InvalidArchiveDat {
            reason: "no path entries".to_owned(),
        });
    }
    Ok(paths)
}
