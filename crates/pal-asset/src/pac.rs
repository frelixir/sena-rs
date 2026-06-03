use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AssetError, Result};
use crate::key::{first_bucket_byte, key_display_lossy, PacKey};
use crate::nls::Nls;

const PAC_BUCKET_TABLE_OFFSET: usize = 0x0C;
const PAC_BUCKET_COUNT: usize = 255;
const PAC_BUCKET_TABLE_SIZE: usize = PAC_BUCKET_COUNT * 8;
const PAC_RECORD_BASE: usize = PAC_BUCKET_TABLE_OFFSET + PAC_BUCKET_TABLE_SIZE;
const PAC_RECORD_SIZE: usize = 40;

#[derive(Clone, Debug)]
pub struct PacBucket {
    pub first_record_index: u32,
    pub record_count: u32,
}

#[derive(Clone, Debug)]
pub struct PacEntry {
    pub key: PacKey,
    pub data_size: u32,
    pub data_offset: u32,
    pub bucket: u8,
}

impl PacEntry {
    pub fn display_name_lossy(&self, nls: Nls) -> String {
        key_display_lossy(&self.key, nls)
    }
}

#[derive(Debug)]
pub struct PacArchive {
    path: PathBuf,
    bytes: Vec<u8>,
    buckets: Vec<PacBucket>,
    entries: Vec<PacEntry>,
    entry_by_key: HashMap<PacKey, usize>,
}

impl PacArchive {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path).map_err(|e| AssetError::io(&path, e))?;
        Self::from_bytes(path, bytes)
    }

    pub fn from_bytes(path: PathBuf, bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < PAC_RECORD_BASE {
            return Err(AssetError::PacTooSmall {
                path,
                len: bytes.len(),
            });
        }

        let mut buckets = Vec::with_capacity(PAC_BUCKET_COUNT);
        for bucket_index in 0..PAC_BUCKET_COUNT {
            let off = PAC_BUCKET_TABLE_OFFSET + bucket_index * 8;
            let first_record_index = read_u32_at(&bytes, off);
            let record_count = read_u32_at(&bytes, off + 4);
            buckets.push(PacBucket {
                first_record_index,
                record_count,
            });
        }

        let mut entries = Vec::new();
        let mut entry_by_key = HashMap::new();

        for (bucket_index, bucket) in buckets.iter().enumerate() {
            let first = bucket.first_record_index as usize;
            let count = bucket.record_count as usize;
            if count == 0 {
                continue;
            }

            let last_exclusive =
                first
                    .checked_add(count)
                    .ok_or_else(|| AssetError::PacBucketOutOfRange {
                        path: path.clone(),
                        bucket: bucket_index,
                        first: bucket.first_record_index,
                        count: bucket.record_count,
                    })?;
            let last_record_end = PAC_RECORD_BASE
                .checked_add(last_exclusive.checked_mul(PAC_RECORD_SIZE).ok_or_else(|| {
                    AssetError::PacBucketOutOfRange {
                        path: path.clone(),
                        bucket: bucket_index,
                        first: bucket.first_record_index,
                        count: bucket.record_count,
                    }
                })?)
                .ok_or_else(|| AssetError::PacBucketOutOfRange {
                    path: path.clone(),
                    bucket: bucket_index,
                    first: bucket.first_record_index,
                    count: bucket.record_count,
                })?;
            if last_record_end > bytes.len() {
                return Err(AssetError::PacBucketOutOfRange {
                    path,
                    bucket: bucket_index,
                    first: bucket.first_record_index,
                    count: bucket.record_count,
                });
            }

            for record_index in first..last_exclusive {
                let off = PAC_RECORD_BASE + record_index * PAC_RECORD_SIZE;
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes[off..off + 32]);
                let data_size = read_u32_at(&bytes, off + 32);
                let data_offset = read_u32_at(&bytes, off + 36);

                let data_end = (data_offset as usize)
                    .checked_add(data_size as usize)
                    .ok_or_else(|| AssetError::PacDataOutOfRange {
                        path: path.clone(),
                        offset: data_offset,
                        size: data_size,
                    })?;
                if data_end > bytes.len() {
                    return Err(AssetError::PacDataOutOfRange {
                        path: path.clone(),
                        offset: data_offset,
                        size: data_size,
                    });
                }

                let entry_index = entries.len();
                entries.push(PacEntry {
                    key,
                    data_size,
                    data_offset,
                    bucket: bucket_index as u8,
                });
                entry_by_key.insert(key, entry_index);
            }
        }

        Ok(Self {
            path,
            bytes,
            buckets,
            entries,
            entry_by_key,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn buckets(&self) -> &[PacBucket] {
        &self.buckets
    }

    pub fn entries(&self) -> &[PacEntry] {
        &self.entries
    }

    pub fn find_entry(&self, key: &PacKey) -> Result<Option<&PacEntry>> {
        let bucket = first_bucket_byte(key)? as usize;
        if bucket >= PAC_BUCKET_COUNT {
            return Err(AssetError::PacUnsupportedBucket { first_byte: key[0] });
        }
        Ok(self
            .entry_by_key
            .get(key)
            .map(|&index| &self.entries[index]))
    }

    pub fn read_entry(&self, entry: &PacEntry) -> Vec<u8> {
        let start = entry.data_offset as usize;
        let end = start + entry.data_size as usize;
        self.bytes[start..end].to_vec()
    }

    pub fn read_key(&self, key: &PacKey) -> Result<Option<Vec<u8>>> {
        Ok(self.find_entry(key)?.map(|entry| self.read_entry(entry)))
    }
}

fn read_u32_at(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}
