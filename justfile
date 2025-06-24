check:
  cargo fmt -- --check
  cargo clippy --all-targets -- -D warnings
  cargo check --all-features

run build="release":
  just _run-{{build}}

_run-debug:
  cargo run

_run-release:
  cargo run --release

# Run a test suite: unit, msrv, min-versions
test suite="unit":
  just _test-{{suite}}

_test-unit:
  cargo test --lib
  cargo test --doc
  cargo test --examples

_test-min-versions:
  just _delete-lockfile
  cargo +nightly check --all-features -Z direct-minimal-versions

_test-msrv:
  cargo install cargo-msrv@0.18.4
  cargo msrv verify --all-features

# Delete unused files or branches: data, lockfile, branches
delete item="branches":
  just _delete-{{item}}

_delete-lockfile:
  rm -f Cargo.lock

_delete-branches:
  git branch --merged | grep -v \* | xargs git branch -d
