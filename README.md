[![Crates.io](https://img.shields.io/crates/v/usn-journal-rs.svg)](https://crates.io/crates/usn-journal-rs)
[![Docs.rs](https://docs.rs/usn-journal-rs/badge.svg)](https://docs.rs/usn-journal-rs)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

# usn-journal-rs 🚀

A Rust library for working with the Windows NTFS/ReFS USN change journal and enumerating the NTFS Master File Table (MFT).

## Overview 📝

**usn-journal-rs** provides safe, ergonomic abstractions for manipulating the USN change journal and accessing MFT records on NTFS volumes. It enables applications to efficiently enumerate file entries and monitor file system changes on Windows systems.

## Features ✨

- 🔍 Read and monitor USN journal records
- 📂 Enumerate NTFS MFT entries
- 🏷️ Resolve file IDs to full paths
- 🦀 High-level, idiomatic Rust API
- 🛡️ Safe abstractions over Windows FFI

## Examples 🧑‍💻

### Enumerate USN Journal

```rust
use usn_journal_rs::{usn_journal::UsnJournal};

let drive_letter = 'C';
let journal = UsnJournal::new_from_drive_letter(drive_letter).unwrap();
for entry in journal.iter().take(10) {
    println!("USN entry: {:?}", entry);
}
```

### Enumerate MFT Entries

```rust
use usn_journal_rs::mft::Mft;

let drive_letter = 'C';
let mft = Mft::new_from_drive_letter(drive_letter).unwrap();
for entry in mft.iter().take(10) {
    println!("{:?}", entry);
}
```

You can find more usage examples in the [`examples/`](examples/) directory. To run an example, use:

```sh
cargo run --example change_monitor
```

Replace `change_monitor` with any example file name in the directory.

## Platform Support 🖥️

- 🪟 **Windows** NTFS/ReFS volumes
- 🔑 Requires appropriate privileges to access the USN journal or MFT.

## Installation 📦

Add the following to your `Cargo.toml`:

```toml
usn-journal-rs = "0.1"
```

## Documentation 📚

See [docs.rs/usn-journal-rs](https://docs.rs/usn-journal-rs) for full API documentation.

## Contributing 🤝

Contributions are welcome! Please open issues or pull requests on [GitHub](https://github.com/wangfu91/usn-journal-rs).

1. 🍴 Fork the repository
2. 🌱 Create your feature branch (`git checkout -b feature/foo`)
3. 💾 Commit your changes (`git commit -am 'Add new feature'`)
4. 🚀 Push to the branch (`git push origin feature/foo`)
5. 📬 Open a pull request

## License 📝

MIT License. See [LICENSE](LICENSE) for details.

---

> **Note:** This crate is Windows-only.
