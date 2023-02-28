#!/usr/bin/env bash

# Exit when any command fails.
set -e

# Run lints.
cargo fmt --all -- --check # run `cargo fmt` to auto-correct the code. 
cargo clippy               # run `cargo clippy --fix` to auto-correct the code.

# Run tests.
RUST_MIN_STACK=33554432 cargo test --release test_examples_write_c -- --nocapture
RUST_MIN_STACK=33554432 python3 tests/fuzz.py --print -n80 --seed 22
