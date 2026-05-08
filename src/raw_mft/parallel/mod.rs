//! Parallel raw-MFT chunk scanning.

mod chunks;
mod executor;
mod scan;

pub use scan::RawMftParallelScan;
