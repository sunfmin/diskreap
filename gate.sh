#!/usr/bin/env bash
# The definition of "may land": the same for a person and for afkconvoy.
set -euo pipefail
cd "$(dirname "$0")"

# It runs on rustup's nightly, the one cargo-flux is built for,
# whatever the shell that started it has on its PATH: a Homebrew `cargo` ahead
# of rustup's breaks when Homebrew moves a library under it. The release
# builds on stable, where Flux's attributes expand to nothing.
PATH="$HOME/.cargo/bin:$PATH"
RUSTUP_TOOLCHAIN=nightly-2026-09-25
export PATH RUSTUP_TOOLCHAIN

cargo fmt --all --check

# The gate's reading of Flux's summaries is itself tested.
./flux-summary-test.sh >/dev/null

# Flux runs first, alone: beside the tests it shares the cores, and its time
# measures them. A solver query past twenty seconds fails naming its function
# (`-Ffixpoint-timeout`), it checks on past an error so one run names every
# slow function (`-Fcatch-bugs`), and it reports the slowest (`-Ftimings`).
# Every function of every pure crate is checked: Flux's summary of each one
# must say `0 trusted; 0 ignored` (./flux-summary.sh reads it).
FLUXFLAGS="-Ftimings -Ffixpoint-timeout=20 -Fcatch-bugs -Flog-dir=$PWD/target/flux-log"
export FLUXFLAGS
flux_log=$(mktemp)
trap 'rm -f "$flux_log"' EXIT
./verify.sh --message-format=json-render-diagnostics >"$flux_log" 2>&1 && flux_status=0 || flux_status=$?
awk '!/^\{/' "$flux_log" >&2
[ "$flux_status" -eq 0 ] || exit "$flux_status"
# shellcheck disable=SC2046
./flux-summary.sh "$flux_log" $(sed -n 's/^PURE="\(.*\)"$/\1/p' verify.sh)

cargo clippy --workspace --all-targets --quiet -- -D warnings

# nextest holds each test to the limit .config/nextest.toml gives.
cargo nextest run --workspace --cargo-quiet --status-level fail --final-status-level fail --show-progress none
