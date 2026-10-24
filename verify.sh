#!/bin/sh
# Checks the pure crates' refinement types with Flux
# (docs/adr/0002-pure-crates-checked-by-flux.md).
# Needs `cargo-flux`, `fixpoint` and `z3` installed, each the one Cargo.toml
# pins: it refuses to check with any other.
#
#   ./verify.sh                 check every pure crate
#   ./verify.sh positive        check the functions whose path contains a name
set -e
cd "$(dirname "$0")"
# The pure crates: every one is checked, none is taken on trust.
PURE="diskreap-rules diskreap-judge"
# cargo-flux re-invokes `cargo +<its nightly>`, which only rustup's cargo
# understands, and rustup needs some toolchain to start from.
PATH="$HOME/.cargo/bin:$PATH"
: "${RUSTUP_TOOLCHAIN:=$(rustup toolchain list | head -1 | cut -d' ' -f1)}"
export PATH RUSTUP_TOOLCHAIN
# The checker is the pinned one, or nothing is checked: another build of it
# can pass or fail the same tree differently. Each tool is asked what it is:
# cargo-flux reports the first seven digits of its Flux revision, fixpoint its
# version and commit, z3 its version.
flux_rev=$(sed -n 's/^flux-rs = .*rev = "\([0-9a-f]*\)".*/\1/p' Cargo.toml)
fixpoint_pin=$(sed -n 's/^fixpoint = "\(.*\)"$/\1/p' Cargo.toml)
z3_pin=$(sed -n 's/^z3 = "\(.*\)"$/\1/p' Cargo.toml)
if [ -z "$flux_rev" ] || [ -z "$fixpoint_pin" ] || [ -z "$z3_pin" ]; then
    echo "verify: Cargo.toml must pin the Flux revision, fixpoint and z3, and one of them is not read from it" >&2
    exit 1
fi
flux_is=$(cargo flux --version 2>/dev/null | cut -d' ' -f2) || true
fixpoint_is=$(fixpoint --version 2>/dev/null | sed -n 's/^fixpoint //p') || true
z3_is=$(z3 --version 2>/dev/null | sed -n 's/^Z3 version \([^ ]*\).*/\1/p') || true
pinned=yes
case "$flux_is" in
"") pinned= ;;
*) case "$flux_rev" in "$flux_is"*) ;; *) pinned= ;; esac ;;
esac
if [ -z "$pinned" ]; then
    cat >&2 <<MSG
verify: cargo-flux is built from Flux ${flux_is:-(nothing reported)}, and Cargo.toml pins $flux_rev. Install the pinned one:
    git clone https://github.com/flux-rs/flux && cd flux && git checkout $flux_rev && cargo xtask install
MSG
fi
if [ "$fixpoint_is" != "$fixpoint_pin" ]; then
    pinned=
    cat >&2 <<MSG
verify: fixpoint is ${fixpoint_is:-(nothing reported)}, and Cargo.toml pins $fixpoint_pin. Install the pinned one, built at the commit in the brackets:
    https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install
MSG
fi
if [ "$z3_is" != "$z3_pin" ]; then
    pinned=
    cat >&2 <<MSG
verify: z3 is ${z3_is:-(nothing reported)}, and Cargo.toml pins $z3_pin. Install the pinned one:
    https://github.com/Z3Prover/z3/releases/tag/z3-$z3_pin
MSG
fi
[ -n "$pinned" ] || exit 1
packages=$(for p in $PURE; do printf -- '-p %s ' "$p"; done)
case "${1:-}" in
"" | -*) ;;
*)
    only="--only-check=def:$1"
    shift
    set -- "$only" "$@"
    ;;
esac
# shellcheck disable=SC2086
exec cargo flux $packages "$@"
