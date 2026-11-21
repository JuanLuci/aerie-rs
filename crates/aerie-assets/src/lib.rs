//! The player's own Hytale assets, read at run time (never shipped), and the
//! registries built from them (spec 07).
//!
//! - [`pack`]: the `Assets.zip` of an installation.
//! - [`store`]: one asset folder: JSON files, `Parent` inheritance, indexes.
//! - [`schema`]: the asset formats; [`contained`]: assets written inside others.
//! - [`registries`]: asset → protocol conversion, one module per registry.

pub mod chm;
pub mod contained;
pub mod ctx;
pub mod pack;
pub mod registries;
pub mod schema;
pub mod store;
pub mod tags;
pub mod values;

/// Hash of the code registries are built from (see `build.rs`): the
/// server's start-up cache is keyed by it.
pub const CODE_HASH: &str = env!("AERIE_ASSETS_CODE_HASH");

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{file}: {msg}")]
    Json { file: String, msg: String },
    #[error("{asset}: parent {parent} not found")]
    MissingParent { asset: String, parent: String },
    #[error("{asset}: {msg}")]
    Invalid { asset: String, msg: String },
    #[error("protocol: {0}")]
    Protocol(#[from] aerie_protocol::Error),
}
