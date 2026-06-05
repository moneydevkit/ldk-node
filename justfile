default:
  @just --list --unsorted

# Validate: rustfmt check + clippy with warnings denied (matches CI's deny-warnings gate).
check:
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings

# Format all sources in place.
fmt:
  cargo fmt --all

# Apply clippy autofixes across all targets.
fix:
  cargo clippy --all-targets --fix --allow-dirty --allow-staged

# Run library unit tests only. No bitcoind/electrs required.
test:
  RUSTFLAGS="-D warnings" cargo test --lib

# Run full suite incl. integration tests. Needs bitcoind+electrs on PATH (CI's --cfg no_download).
test-all:
  RUSTFLAGS="--cfg no_download -D warnings" cargo test
