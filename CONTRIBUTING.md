# Contributing

## Workflow

1. Open an issue or PR describing the change.
2. Keep commits narrow; each commit should establish one invariant.
3. Add or update tests with every behavioral change.
4. Document user-visible behavior in `docs/`.

## Building and testing

```sh
cargo build --workspace
cargo test --workspace --all-features
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo doc --workspace --no-deps
```

## Coding rules

* No `unsafe` outside `crates/jobwrap-pty/src/ffi.rs`; any exception there has
  a written safety invariant and focused tests.
* No unstructured dictionaries for domain data; use structs and enums.
* Typed errors with `thiserror`; `anyhow` only at executable boundaries.
* No `unwrap()`, `expect()`, `panic!()`, `todo!()`, or `unimplemented!()` in
  production paths.
* Route all authorization through `jobwrap_core::authorize`.
* Never run user arguments through an implicit shell.
* Preserve the child process whenever monitoring infrastructure fails.
* Treat terminal data as arbitrary bytes.
* Treat every PID as potentially stale or reused.
* Never weaken a security default to make a test pass.

## Dependencies

* All versions are pinned in the workspace manifest and `Cargo.lock`.
* No wildcard versions.
* Add a dependency only with a documented purpose and a review of its
  maintenance, license, and security record.

## Recording decisions

Record unresolved assumptions in `docs/decisions/` as architecture decision
records.
