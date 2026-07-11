//! Worker/executor internals for deterministic parallel raw-MFT chunk scans.

use std::{
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use crate::{
    errors::UsnError,
    raw_mft::{RawMft, RawMftWorkChunk, io::VolumeReader, options::RawMftScanOptions},
    volume::Volume,
};

/// Internal worker scheduling mode for chunk execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkScheduling {
    /// Workers fetch the next remaining chunk from a shared atomic cursor.
    Dynamic,
    /// Each worker receives one contiguous band of chunk indices.
    Contiguous,
}

/// Reopenable source information for worker-local volume handles.
#[derive(Debug, Clone)]
pub(in crate::raw_mft) enum ParallelVolumeSource {
    /// Reopen by drive letter (e.g. `C`).
    #[cfg(windows)]
    DriveLetter(char),
    /// Reopen by mount-point path.
    MountPoint(PathBuf),
    /// Reopen a Linux block device directly.
    #[cfg(target_os = "linux")]
    DevicePath(PathBuf),
}

/// Run chunk work in parallel and visit results in original chunk order.
///
/// The work closure must be [`Sync`] because one shared closure reference is
/// called by multiple worker threads. `T` must be [`Send`] because worker
/// results cross thread boundaries before they are visited in chunk order.
pub(super) fn run_parallel_chunks_in_order<T, Work, Visit>(
    mft: &RawMft<'_>,
    chunks: Vec<RawMftWorkChunk>,
    options: RawMftScanOptions,
    worker_count: NonZeroUsize,
    scheduling: ChunkScheduling,
    work_chunk: Work,
    mut visit: Visit,
) -> Result<(), UsnError>
where
    for<'m> Work: Fn(
            &RawMft<'m>,
            RawMftWorkChunk,
            &RawMftScanOptions,
            &mut VolumeReader,
            &mut VolumeReader,
        ) -> Result<T, UsnError>
        + Sync,
    T: Send,
    Visit: FnMut(T) -> Result<(), UsnError>,
{
    if chunks.is_empty() {
        return Ok(());
    }

    let worker_count = worker_count.get().min(chunks.len()).max(1);
    if worker_count == 1 {
        let (mut reader, mut attr_reader) = mft.buffered_readers_for_options(&options)?;
        for chunk in chunks {
            let result = work_chunk(mft, chunk, &options, &mut reader, &mut attr_reader)?;
            visit(result)?;
        }
        return Ok(());
    }

    let source = reusable_parallel_volume_source(mft.volume)?;
    let next_index = AtomicUsize::new(0);
    let chunk_count = chunks.len();
    let chunks = chunks.into_boxed_slice();
    let boot = mft.boot.clone();
    let extent_map = Arc::clone(&mft.extent_map);
    let bitmap = Arc::clone(&mft.bitmap);

    thread::scope(|scope| -> Result<(), UsnError> {
        let (tx, rx) = mpsc::channel::<Result<(usize, T), UsnError>>();
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let next_index = &next_index;
            let chunks = &chunks;
            let tx = tx.clone();
            let options = options.clone();
            let source = source.clone();
            let boot = boot.clone();
            let extent_map = Arc::clone(&extent_map);
            let bitmap = Arc::clone(&bitmap);
            let work_chunk = &work_chunk;
            handles.push(scope.spawn(move || {
                let volume = match open_parallel_volume(&source) {
                    Ok(volume) => volume,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return;
                    }
                };
                let worker_mft = RawMft {
                    volume: &volume,
                    boot,
                    extent_map,
                    bitmap,
                };
                let (mut reader, mut attr_reader) =
                    match worker_mft.buffered_readers_for_options(&options) {
                        Ok(readers) => readers,
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            return;
                        }
                    };

                match scheduling {
                    ChunkScheduling::Dynamic => loop {
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        if index >= chunks.len() {
                            break;
                        }

                        let chunk = chunks[index];
                        let result =
                            work_chunk(&worker_mft, chunk, &options, &mut reader, &mut attr_reader)
                                .map(|result| (index, result));
                        if tx.send(result).is_err() {
                            break;
                        }
                    },
                    ChunkScheduling::Contiguous => {
                        let (start, end) =
                            contiguous_worker_range(chunks.len(), worker_count, worker_index);
                        for index in start..end {
                            let chunk = chunks[index];
                            let result = work_chunk(
                                &worker_mft,
                                chunk,
                                &options,
                                &mut reader,
                                &mut attr_reader,
                            )
                            .map(|result| (index, result));
                            if tx.send(result).is_err() {
                                break;
                            }
                        }
                    }
                }
            }));
        }
        drop(tx);

        drain_parallel_results_in_order(rx, chunk_count, &mut visit)?;

        for handle in handles {
            if let Err(payload) = handle.join() {
                return Err(worker_panicked(payload));
            }
        }

        Ok(())
    })
}

/// Return the half-open chunk-index range assigned to one worker under contiguous scheduling.
fn contiguous_worker_range(
    chunk_count: usize,
    worker_count: usize,
    worker_index: usize,
) -> (usize, usize) {
    let base = chunk_count / worker_count;
    let extra = chunk_count % worker_count;
    let start = worker_index * base + worker_index.min(extra);
    let len = base + usize::from(worker_index < extra);
    (start, start + len)
}

/// Build a stable error when available parallelism cannot be queried.
pub(crate) fn available_parallelism_error(error: io::Error) -> UsnError {
    UsnError::Io(io::Error::other(format!(
        "failed to query available parallelism: {error}"
    )))
}

/// Drain worker results, buffering out-of-order completions until they can be
/// yielded in original chunk order.
fn drain_parallel_results_in_order<T, Visit>(
    rx: mpsc::Receiver<Result<(usize, T), UsnError>>,
    chunk_count: usize,
    visit: &mut Visit,
) -> Result<(), UsnError>
where
    Visit: FnMut(T) -> Result<(), UsnError>,
{
    let mut next_expected = 0usize;
    let mut pending = Vec::with_capacity(chunk_count);
    pending.resize_with(chunk_count, || None);

    while next_expected < chunk_count {
        match rx.recv() {
            Ok(Ok((index, result))) => {
                pending[index] = Some(result);
                while next_expected < chunk_count {
                    let Some(result) = pending[next_expected].take() else {
                        break;
                    };
                    visit(result)?;
                    next_expected += 1;
                }
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(channel_closed()),
        }
    }

    Ok(())
}

/// Resolve the original volume into a reopenable source for worker threads.
pub(in crate::raw_mft) fn reusable_parallel_volume_source(
    volume: &Volume,
) -> Result<ParallelVolumeSource, UsnError> {
    #[cfg(windows)]
    let source = volume.drive_letter().map(ParallelVolumeSource::DriveLetter);
    #[cfg(not(windows))]
    let source: Option<ParallelVolumeSource> = None;
    let source = source.or_else(|| {
            volume
                .mount_point()
                .map(|path| ParallelVolumeSource::MountPoint(path.to_path_buf()))
        });
    #[cfg(target_os = "linux")]
    let source = source.or_else(|| volume.device_path().map(|path| {
        ParallelVolumeSource::DevicePath(path.to_path_buf())
    }));
    source
        .ok_or_else(|| {
            UsnError::Io(io::Error::other(
                "raw_mft parallel chunk parsing requires a reusable volume source",
            ))
        })
}

/// Reopen the original volume source for one worker thread.
pub(in crate::raw_mft) fn open_parallel_volume(
    source: &ParallelVolumeSource,
) -> Result<Volume, UsnError> {
    match source {
        #[cfg(windows)]
        ParallelVolumeSource::DriveLetter(drive_letter) => Volume::from_drive_letter(*drive_letter),
        ParallelVolumeSource::MountPoint(path) => Volume::from_mount_point(path),
        #[cfg(target_os = "linux")]
        ParallelVolumeSource::DevicePath(path) => Volume::from_device_path(path),
    }
}

/// Build the channel-closed error used by the ordered parallel executor.
fn channel_closed() -> UsnError {
    UsnError::Io(io::Error::other(
        "raw_mft parallel chunk channel closed unexpectedly",
    ))
}

/// Build the panic-propagation error used by the ordered parallel executor.
fn worker_panicked(payload: Box<dyn std::any::Any + Send + 'static>) -> UsnError {
    let details = if let Some(message) = payload.downcast_ref::<&str>() {
        *message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else {
        "unknown panic payload"
    };
    UsnError::Io(io::Error::other(format!(
        "raw_mft parallel worker panicked: {details}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_worker_range_partitions_evenly() {
        // 9 chunks across 3 workers: three contiguous bands of 3.
        assert_eq!(contiguous_worker_range(9, 3, 0), (0, 3));
        assert_eq!(contiguous_worker_range(9, 3, 1), (3, 6));
        assert_eq!(contiguous_worker_range(9, 3, 2), (6, 9));
    }

    #[test]
    fn contiguous_worker_range_spreads_remainder_to_early_workers() {
        // 10 chunks across 3 workers: the extra chunk goes to worker 0.
        assert_eq!(contiguous_worker_range(10, 3, 0), (0, 4));
        assert_eq!(contiguous_worker_range(10, 3, 1), (4, 7));
        assert_eq!(contiguous_worker_range(10, 3, 2), (7, 10));
    }

    #[test]
    fn contiguous_worker_ranges_cover_all_chunks_without_gaps() {
        let chunk_count = 37;
        let worker_count = 8;
        let mut covered = Vec::new();
        for worker in 0..worker_count {
            let (start, end) = contiguous_worker_range(chunk_count, worker_count, worker);
            assert!(start <= end);
            covered.extend(start..end);
        }
        let expected: Vec<usize> = (0..chunk_count).collect();
        assert_eq!(covered, expected);
    }

    #[test]
    fn contiguous_worker_range_handles_more_workers_than_chunks() {
        // 2 chunks, 4 workers: workers 2 and 3 get empty ranges.
        assert_eq!(contiguous_worker_range(2, 4, 0), (0, 1));
        assert_eq!(contiguous_worker_range(2, 4, 1), (1, 2));
        assert_eq!(contiguous_worker_range(2, 4, 2), (2, 2));
        assert_eq!(contiguous_worker_range(2, 4, 3), (2, 2));
    }

    #[test]
    fn worker_panicked_extracts_str_and_string_payloads() {
        let from_str = worker_panicked(Box::new("boom"));
        assert!(from_str.to_string().contains("boom"));

        let from_string = worker_panicked(Box::new(String::from("kaboom")));
        assert!(from_string.to_string().contains("kaboom"));

        let from_unknown = worker_panicked(Box::new(42u32));
        assert!(from_unknown.to_string().contains("unknown panic payload"));
    }

    #[test]
    fn channel_closed_and_parallelism_errors_are_io() {
        assert!(channel_closed().is_io_error());
        let err = available_parallelism_error(io::Error::other("nope"));
        assert!(err.is_io_error());
        assert!(err.to_string().contains("available parallelism"));
    }
}
