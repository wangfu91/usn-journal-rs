[![Crates.io](https://img.shields.io/crates/v/usn-journal-rs.svg)](https://crates.io/crates/usn-journal-rs)
[![Docs.rs](https://docs.rs/usn-journal-rs/badge.svg)](https://docs.rs/usn-journal-rs)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

# usn-journal-rs 🚀

A Rust library for working with the NTFS USN change journal and enumerating the MFT.

## Overview 📝

**usn-journal-rs** provides safe, ergonomic abstractions for manipulating the USN change journal and accessing MFT records on NTFS volumes. It enables applications to efficiently enumerate file entries and monitor file system changes on Windows systems.

## Features ✨

- 🔍 Read and monitor USN journal records
- 📂 Enumerate NTFS MFT entries
- 🧬 Read raw `$MFT` for rich metadata (timestamps, sizes, ADS, sparse/compressed flags)
- 🏷️ Resolve file IDs to full paths
- 🦀 High-level, idiomatic Rust API
- 🛡️ Safe abstractions over Windows FFI

## Examples 🧑‍💻

### Enumerate USN Journal Entries

```rust
use usn_journal_rs::{volume::Volume, journal::UsnJournal};

let drive_letter = 'C';
let volume = Volume::from_drive_letter(drive_letter)?;
let journal = UsnJournal::new(&volume);
for entry_result in journal.iter()? {
    match entry_result {
        Ok(entry) => println!("USN entry: {:?}", entry),
        Err(e) => eprintln!("Error reading USN entry: {e}"),
    }
}
```

### Enumerate MFT Entries

```rust
use usn_journal_rs::{volume::Volume, mft::Mft};

let drive_letter = 'C';
let volume = Volume::from_drive_letter(drive_letter)?;
let mft = Mft::new(&volume);
for entry_result in mft.iter()? {
    match entry_result {
        Ok(entry) => println!("MFT entry: {:?}", entry),
        Err(e) => eprintln!("Error reading MFT entry: {e}"),
    }
}
```

You can find more usage examples in the [`examples`](examples/) directory. To run an example, use:

```sh
sudo cargo run --example change_monitor
```

Replace `change_monitor` with any example file name in the directory.

### Enumerate Raw `$MFT` Records (Rich Metadata)

For applications that need timestamps, real / allocated sizes, alternate
data streams, sparse / compressed flags, and the namespace of each file
name, `RawMft` reads the `$MFT` file directly from the volume:

```rust
use usn_journal_rs::{volume::Volume, raw_mft::RawMft};

let volume = Volume::from_drive_letter('C')?;
let mft = RawMft::new(&volume)?;
for entry in mft.iter()? {
    let entry = entry?;
    if entry.is_used && !entry.file_name.is_empty() {
        println!(
            "{:>8} {} size={} ads={} created={:?}",
            entry.record_number,
            entry.file_name.to_string_lossy(),
            entry.real_size,
            entry.alternate_data_streams.len(),
            entry.si_created,
        );
    }
}
```

This path is **NTFS only**; ReFS volumes have no `$MFT` and return
`UsnError::UnsupportedFilesystem`.

### Benchmarks

Benchmarks use [Divan](https://github.com/nvzqz/divan). Run with:

```sh
sudo cargo bench --bench raw_mft
```

Set `USN_TEST_DRIVE` to choose the drive letter (default `C`).

## Platform Support 🖥️

- 🪟 **Windows** NTFS/ReFS volumes
- 🔑 Requires administrator privilege to access the USN journal or MFT.

## Documentation 📚

See [docs.rs/usn-journal-rs](https://docs.rs/usn-journal-rs) for full API documentation.

## Contributing 🤝

Contributions are welcome! Please open issues or pull requests on [GitHub](https://github.com/wangfu91/usn-journal-rs).

## License 📝

MIT License. See [LICENSE](LICENSE) for details.

---

> **Note:** 
 - This crate is Windows-only.
 - ReFS does not have a Master File Table (MFT).
