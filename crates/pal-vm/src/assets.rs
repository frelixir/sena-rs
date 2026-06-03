use std::collections::BTreeMap;

use pal_asset::{LoadedAsset, ResourceManager};
use pal_script::{PointTable, ScriptImage};

use crate::config::SystemDat;

#[derive(Debug)]
pub struct CoreAssets {
    pub script: LoadedAsset,
    pub file_dat: LoadedAsset,
    pub text_dat: LoadedAsset,
    pub mem_dat: LoadedAsset,
    pub point_dat: LoadedAsset,
    pub graphic_dat: Option<LoadedAsset>,
    pub script_check_value: u32,
    pub script_entry_pc: u32,
    pub point_table: PointTable,
    pub graphic_index: Option<GraphicIndex>,
}

impl CoreAssets {
    pub fn load(
        resource_manager: &mut ResourceManager,
        system_dat: Option<&SystemDat>,
    ) -> anyhow::Result<Self> {
        let script = resource_manager.open("Script.src")?;
        let script_image = ScriptImage::parse(&script.bytes)?;

        if let Some(system_dat) = system_dat {
            if !system_dat.matches_script_check(script_image.check_value()) {
                return Err(anyhow::anyhow!(
                    "system.dat check value 0x{:08X} does not match Script.src check value 0x{:08X}",
                    system_dat.script_check_value,
                    script_image.check_value()
                ));
            }
        }

        let file_dat = resource_manager.open_decrypting("File.dat")?;
        let text_dat = resource_manager.open_decrypting("Text.dat")?;
        let mem_dat = resource_manager.open_decrypting("Mem.dat")?;
        let point_dat = resource_manager.open_decrypting("Point.dat")?;
        let point_table = PointTable::parse(&point_dat.bytes)?;

        let graphic_dat = match resource_manager.open_decrypting("graphic.dat") {
            Ok(asset) => Some(asset),
            Err(pal_asset::AssetError::AssetNotFound { .. }) => None,
            Err(err) => return Err(err.into()),
        };
        let graphic_index = match graphic_dat.as_ref() {
            Some(asset) => match GraphicIndex::parse(&asset.bytes) {
                Ok(index) => Some(index),
                Err(err) => {
                    log::warn!("graphic.dat parse failed (non-fatal): {err}");
                    None
                }
            },
            None => None,
        };

        // Extract values before moving `script`; script_image borrows script.bytes.
        let script_check_value = script_image.check_value();
        let script_entry_pc = script_image.entry_pc();
        let _ = script_image; // release borrow of script.bytes before moving script

        Ok(Self {
            script,
            file_dat,
            text_dat,
            mem_dat,
            point_dat,
            graphic_dat,
            script_check_value,
            script_entry_pc,
            point_table,
            graphic_index,
        })
    }

    pub fn script_image(&self) -> anyhow::Result<ScriptImage<'_>> {
        Ok(ScriptImage::parse(&self.script.bytes)?)
    }
}

#[derive(Debug)]
pub struct GraphicIndex {
    pub buckets: Vec<GraphicBucket>,
    pub data_offset: usize,
    pub entries_by_key: BTreeMap<[u8; 32], GraphicEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphicBucket {
    pub first_record_index: u32,
    pub record_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphicEntry {
    pub key: [u8; 32],
    pub data_size: u32,
    pub data_offset: u32,
    pub bucket: u8,
}

impl GraphicIndex {
    pub const BUCKET_COUNT: usize = 255;
    pub const BUCKET_TABLE_SIZE: usize = 0x7F8;
    pub const RECORD_BASE_AFTER_DOLLAR_HEADER: usize = 0x10 + Self::BUCKET_TABLE_SIZE;
    pub const RECORD_SIZE: usize = 40;

    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        if bytes.len() < 0x10 + Self::BUCKET_TABLE_SIZE {
            return Err(anyhow::anyhow!(
                "graphic.dat is too small: got 0x{:X}, expected at least 0x{:X}",
                bytes.len(),
                0x10 + Self::BUCKET_TABLE_SIZE
            ));
        }

        let bucket_base = 0x10;
        let mut buckets = Vec::with_capacity(Self::BUCKET_COUNT);
        for bucket in 0..Self::BUCKET_COUNT {
            let offset = bucket_base + bucket * 8;
            buckets.push(GraphicBucket {
                first_record_index: read_u32(bytes, offset)?,
                record_count: read_u32(bytes, offset + 4)?,
            });
        }

        let mut entries_by_key = BTreeMap::new();
        for (bucket, info) in buckets.iter().enumerate() {
            let first = info.first_record_index as usize;
            let count = info.record_count as usize;
            let last = first
                .checked_add(count)
                .ok_or_else(|| anyhow::anyhow!("graphic.dat bucket range overflows"))?;
            for record_index in first..last {
                let offset =
                    Self::RECORD_BASE_AFTER_DOLLAR_HEADER
                        .checked_add(record_index.checked_mul(Self::RECORD_SIZE).ok_or_else(
                            || anyhow::anyhow!("graphic.dat record offset overflows"),
                        )?)
                        .ok_or_else(|| anyhow::anyhow!("graphic.dat record offset overflows"))?;
                if offset + Self::RECORD_SIZE > bytes.len() {
                    return Err(anyhow::anyhow!(
                        "graphic.dat record {} is out of range at 0x{:X}",
                        record_index,
                        offset
                    ));
                }
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes[offset..offset + 32]);
                let data_size = read_u32(bytes, offset + 32)?;
                let data_offset = read_u32(bytes, offset + 36)?;
                entries_by_key.insert(
                    key,
                    GraphicEntry {
                        key,
                        data_size,
                        data_offset,
                        bucket: bucket as u8,
                    },
                );
            }
        }

        Ok(Self {
            buckets,
            data_offset: Self::RECORD_BASE_AFTER_DOLLAR_HEADER,
            entries_by_key,
        })
    }

    pub fn len(&self) -> usize {
        self.entries_by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries_by_key.is_empty()
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| anyhow::anyhow!("read offset overflows"))?;
    if end > bytes.len() {
        return Err(anyhow::anyhow!("read out of range at 0x{:X}", offset));
    }
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}
