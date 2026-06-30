//! Parallel raw-MFT chunk scanning.

mod chunks;
mod executor;
mod cached_enrich;
mod scan;

pub(crate) use executor::ChunkScheduling;
pub use scan::{RawMftParallelScan, RawMftParallelScheduling};
