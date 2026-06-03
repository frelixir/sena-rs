//! Asset loading support for the reversed PAL engine.
//!
//! This crate intentionally handles the original single-language PAL search path only.
//! It does not implement the CN/TC path lists from `archive_cn.dat` and `archive_tc.dat`.
//! External asset names are accepted as UTF-8 strings. The selected [`Nls`] controls how
//! those names are encoded into the original byte-level PAL lookup keys.

mod archive_dat;
mod decrypt;
mod error;
mod key;
mod nls;
mod pac;
mod resource;

pub use archive_dat::parse_archive_dat_paths;
pub use decrypt::{decrypt_dollar_payload, decrypt_pal_dollar_file};
pub use error::{AssetError, Result};
pub use key::{make_pac_key, normalize_pal_name_bytes, PacKey};
pub use nls::Nls;
pub use pac::{PacArchive, PacBucket, PacEntry};
pub use resource::{AssetSource, LoadedAsset, ResourceManager};
