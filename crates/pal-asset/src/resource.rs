use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::archive_dat::parse_archive_dat_paths;
use crate::decrypt::decrypt_pal_dollar_file;
use crate::error::{AssetError, Result};
use crate::key::{make_pac_key, PacKey};
use crate::nls::Nls;
use crate::pac::PacArchive;

#[derive(Clone, Debug)]
pub enum AssetSource {
    Loose { path: PathBuf },
    Pac { pac_path: PathBuf, key: PacKey },
}

#[derive(Clone, Debug)]
pub struct LoadedAsset {
    pub name: String,
    pub bytes: Vec<u8>,
    pub source: AssetSource,
}

#[derive(Debug)]
pub struct ResourceManager {
    root: PathBuf,
    nls: Nls,
    paths: Vec<String>,
    pacs: HashMap<PathBuf, PacArchive>,
}

impl ResourceManager {
    pub fn new(root: impl Into<PathBuf>, nls: Nls) -> Self {
        Self {
            root: root.into(),
            nls,
            paths: Vec::new(),
            pacs: HashMap::new(),
        }
    }

    pub fn bootstrap(root: impl Into<PathBuf>, nls: Nls) -> Result<Self> {
        let mut manager = Self::new(root, nls);
        manager.add_path("data");

        let archive = manager.open("archive.dat")?;
        let paths = parse_archive_dat_paths(&archive.bytes, nls)?;
        for path in paths {
            manager.add_path(path);
        }
        Ok(manager)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn nls(&self) -> Nls {
        self.nls
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn add_path(&mut self, path: impl AsRef<str>) {
        let normalized = normalize_relative_path(path.as_ref());
        if normalized.is_empty() {
            return;
        }
        if !self.paths.iter().any(|existing| existing == &normalized) {
            self.paths.push(normalized);
        }
    }

    pub fn preload_pacs(&mut self) -> Result<()> {
        let paths = self.paths.clone();
        for path in paths {
            let pac_path = self.pac_path_for_base(&path);
            if pac_path.exists() {
                self.ensure_pac_loaded(&pac_path)?;
            }
        }
        Ok(())
    }

    pub fn open(&mut self, name: &str) -> Result<LoadedAsset> {
        let key = make_pac_key(name, self.nls)?;

        for base in self.paths.clone() {
            if let Some(asset) = self.try_open_in_base(&base, name, &key)? {
                return Ok(asset);
            }
        }

        if let Some(asset) = self.try_open_in_root(name, &key)? {
            return Ok(asset);
        }

        Err(AssetError::AssetNotFound {
            name: name.to_owned(),
        })
    }

    pub fn open_decrypting(&mut self, name: &str) -> Result<LoadedAsset> {
        let mut asset = self.open(name)?;
        // Loose files are already decrypted by the PAC unpacker; only PAC-sourced
        // data carries the raw (encrypted) payload and needs the dollar-file transform.
        if matches!(asset.source, AssetSource::Pac { .. }) {
            decrypt_pal_dollar_file(name, &mut asset.bytes)?;
        }
        Ok(asset)
    }

    pub fn list_loaded_pacs(&self) -> impl Iterator<Item = &PacArchive> {
        self.pacs.values()
    }

    fn try_open_in_base(
        &mut self,
        base: &str,
        name: &str,
        key: &PacKey,
    ) -> Result<Option<LoadedAsset>> {
        let loose = self.loose_path_for_base(base, name);
        if loose.is_file() {
            let bytes = fs::read(&loose).map_err(|e| AssetError::io(&loose, e))?;
            return Ok(Some(LoadedAsset {
                name: name.to_owned(),
                bytes,
                source: AssetSource::Loose { path: loose },
            }));
        }

        let pac_path = self.pac_path_for_base(base);
        if !pac_path.is_file() {
            return Ok(None);
        }
        let pac = self.ensure_pac_loaded(&pac_path)?;
        if let Some(bytes) = pac.read_key(key)? {
            return Ok(Some(LoadedAsset {
                name: name.to_owned(),
                bytes,
                source: AssetSource::Pac {
                    pac_path,
                    key: *key,
                },
            }));
        }
        Ok(None)
    }

    fn try_open_in_root(&mut self, name: &str, key: &PacKey) -> Result<Option<LoadedAsset>> {
        let loose = join_pal_path(&self.root, name);
        if loose.is_file() {
            let bytes = fs::read(&loose).map_err(|e| AssetError::io(&loose, e))?;
            return Ok(Some(LoadedAsset {
                name: name.to_owned(),
                bytes,
                source: AssetSource::Loose { path: loose },
            }));
        }

        let pac_path = append_pac_extension(&self.root);
        if pac_path.is_file() {
            let pac = self.ensure_pac_loaded(&pac_path)?;
            if let Some(bytes) = pac.read_key(key)? {
                return Ok(Some(LoadedAsset {
                    name: name.to_owned(),
                    bytes,
                    source: AssetSource::Pac {
                        pac_path,
                        key: *key,
                    },
                }));
            }
        }
        Ok(None)
    }

    fn loose_path_for_base(&self, base: &str, name: &str) -> PathBuf {
        let base_path = join_pal_path(&self.root, base);
        join_pal_path(&base_path, name)
    }

    fn pac_path_for_base(&self, base: &str) -> PathBuf {
        append_pac_extension(&join_pal_path(&self.root, base))
    }

    fn ensure_pac_loaded(&mut self, pac_path: &Path) -> Result<&PacArchive> {
        if !self.pacs.contains_key(pac_path) {
            let pac = PacArchive::from_file(pac_path)?;
            self.pacs.insert(pac_path.to_path_buf(), pac);
        }
        Ok(self
            .pacs
            .get(pac_path)
            .expect("pac cache entry was just inserted"))
    }
}

fn normalize_relative_path(path: &str) -> String {
    path.trim_matches(|c| c == '\\' || c == '/')
        .replace('/', "\\")
}

fn join_pal_path(base: &Path, pal_path: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in pal_path.split(|c| c == '\\' || c == '/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            out.pop();
            continue;
        }
        out.push(part);
    }
    out
}

fn append_pac_extension(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".pac");
    PathBuf::from(s)
}

#[allow(dead_code)]
fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}
