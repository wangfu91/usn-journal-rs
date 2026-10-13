# Contributing

Use Windows and Rust 1.88 or later. Keep `Cargo.lock` in version control so CI
and release verification use the same dependency versions.

## Validation

```powershell
cargo fmt --all -- --check
cargo build --locked --all-targets --all-features
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
$env:RUSTDOCFLAGS = '-D warnings'
cargo doc --locked --no-deps --all-features
cargo package --locked
cargo publish --locked --registry crates-io --dry-run
cargo +1.88.0 check --locked --lib
```

For uncommitted review changes, add `--allow-dirty` to the package and publish
dry-run commands. Neither command uploads a crate. CI also checks the library
for Windows ARM64 and i686; these are compile checks, not runtime tests.

Run device tests from an Administrator process on suitable volumes:
`USN_TEST_DRIVE` selects NTFS (default `C`) and `USN_REFS_TEST_DRIVE` selects ReFS
(default `D`). Tests report a skip when prerequisites are missing; unexpected
errors after NTFS prerequisites pass fail the test. A passing test command with
skipped device cases does not establish live-volume coverage. Journal tests may
create a journal if the selected volume has none.
The ReFS enumeration test compares the library with a direct Win32 call. It
validates 128-bit IDs when supported, or verifies the original unsupported-operation
error when ReFS rejects `FSCTL_ENUM_USN_DATA`.

## Release

1. Configure the repository Actions secret `CARGO_REGISTRY_TOKEN` with permission
   to publish `usn-journal-rs` on crates.io. The workflow supplies it only to the
   final upload step.
2. Update the package version and regenerate `Cargo.lock`. Finalize the changelog
   with the release date and a versioned comparison link; update README examples.
3. Review the changes and require successful Windows, MSRV, and cross-target CI
   on the release commit before tagging it.
4. Create and push a new annotated tag matching the package version, for example
   `v0.5.0`. Only a newly created, non-deleted `v*` tag push may publish. Branch
   pushes, PR merges, tag updates, and tag deletions cannot authorize an upload.

   ```powershell
   git tag --annotate v0.5.0 --message "usn-journal-rs 0.5.0"
   git push origin v0.5.0
   ```

5. Observe the **Publish to crates.io** workflow. It validates the tag and
   lockfile, runs checks and a dry run, then publishes. Check the version on
   [crates.io](https://crates.io/crates/usn-journal-rs) and its docs.rs build.

If upload times out, check crates.io before retrying: a timeout can happen after
the upload succeeded. Keep the original release tag and inspect the workflow
failure instead of moving or recreating it.
