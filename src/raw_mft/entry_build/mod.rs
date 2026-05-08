//! Entry materialization and batch-building helpers for raw-MFT records.

mod batch;
mod capture;
mod entry;
mod fold;
mod names;

pub(crate) use batch::RawMftBatchScratch;
pub use batch::{RawMftBatchEntry, RawMftChunkBatch};
pub use entry::{AdsInfo, RawMftEntry, RawMftLink};
pub(crate) use entry::{AttributeListInfo, EntryBuildOptions};
