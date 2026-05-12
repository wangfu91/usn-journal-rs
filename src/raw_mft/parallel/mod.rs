//! Parallel raw-MFT chunk scanning.

mod chunks;
mod executor;
mod scan;

pub(crate) use executor::ChunkScheduling;
pub use scan::RawMftParallelScan;
