#!/usr/bin/env sh

set -e

test_with_toolchain() {
    cargo $1 test $2 --target x86_64-unknown-linux-gnu -- --nocapture
    cargo $1 test $2 --release --target x86_64-unknown-linux-gnu -- --nocapture
}

export RUST_BACKTRACE=1
export RUSTFLAGS='-C debuginfo=2 -Z sanitizer=address'
export ASAN_OPTIONS='fast_unwind_on_malloc=0 detect_odr_violation=0'
export LSAN_OPTIONS="$ASAN_OPTIONS"

test_with_toolchain +nightly --lib

export RUSTFLAGS='-C debuginfo=2'

test_with_toolchain +stable
