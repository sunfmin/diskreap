#!/usr/bin/env bash
# Holds each pure crate to Flux's summary of it: `0 trusted; 0 ignored`.
#
#   ./flux-summary.sh <log> <crate>...
#
# <log> is what `./verify.sh --message-format=json-render-diagnostics` printed.
# It names the first crate whose summary is missing or exempts anything, and
# exits 1.
#
# The log cannot say whose summary is whose: cargo checks crates in parallel,
# so one crate's summary can come between another's and its artifact line
# (wxread issue #22). A crate's own summary is in the output cargo keeps for the unit
# it built, beside the artifact the log names (`<unit>/out/lib<crate>-….rmeta`
# keeps it at `<unit>/fingerprint/output-lib-<crate>`); cargo replays that
# output when the unit is fresh, so it holds the summary of the code checked.
set -euo pipefail
log=$1
shift
fail() {
  echo "gate: the pure crate $1 must not exempt anything from Flux, and Flux reports: ${2:-no summary of it}" >&2
  exit 1
}
for crate in "$@"; do
  name=${crate//-/_}
  rmetas=$(awk -v n="\"name\":\"$name\"" '
      /^\{"reason":"compiler-artifact"/ && index($0, n) && match($0, /"filenames":\["[^"]*\.rmeta"/) {
          print substr($0, RSTART + 14, RLENGTH - 15)
      }' "$log")
  [ -n "$rmetas" ] || fail "$crate"
  while IFS= read -r rmeta; do
    output="${rmeta%/out/*}/fingerprint/output-lib-$name"
    summary=
    [ ! -f "$output" ] || summary=$(awk '{ gsub(/\033\[[0-9;]*m/, "") } /^summary\./ { s = $0 } END { print s }' "$output")
    case "$summary" in
    *"; 0 trusted; 0 ignored."*) ;;
    *) fail "$crate" "$summary" ;;
    esac
  done <<<"$rmetas"
done
