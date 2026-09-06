# Raw MFT extraction

The raw reader was extracted to the sibling `ntfs-mft` repository from commit
`0e57c8eed844b22e3b261a6823910932b3ad15ef`. This branch retains its journal,
FSCTL enumeration, live-path, error, and public-type refinements over main.

`ntfs-mft` owns `src/raw_mft`, the snapshot directory tree, historical paths,
raw examples, benchmark support, raw integration tests, and performance docs.
Small foundational types and volume helpers are independent copies; neither
library depends on the other. Existing shared public types/errors remain here.

Use `ntfs_mft::{Volume, RawMft, MftError, MftResult}` for raw scans. The old
`usn_journal_rs::raw_mft` and `Volume::raw_mft()` are removed. The separate
crate's types are distinct; compare file IDs through their numeric accessors.

The cross-repository parity tests live in `ntfs-mft/integration-tests` and expect
sibling checkouts. Run from that repository:

```text
cargo test --manifest-path integration-tests/Cargo.toml
```
