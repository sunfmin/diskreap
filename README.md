# diskreap

An agent CLI that frees disk space on a Mac by Rules kept in a private gist,
and deletes only what is safe. A Rule names one kind of thing that may go
(a file glob, a marker, a minimum age, programs and apps that hold it) and
why; every Rule is one a person agreed to. Each command prints one JSON
object; while it walks, every Match, deletion and finding is printed as it
happens, one JSON object a line on stderr. `diskreap --help` is the manual.

```sh
diskreap doctor                     # what is missing, each with its fix
diskreap init                       # make the private gist of Rules
diskreap scan                       # large directories no Rule covers
diskreap rule check '<rule>'        # what a Rule would match, adding nothing
diskreap rule add '<rule>'          # add it, once a person agreed
diskreap plan                       # what clean would delete and hold
diskreap clean                      # delete each Match nothing holds, as it is found
```

A Match goes only when nothing in it changed within the Rule's min_age, no
process holds it open or works in it, its Rule's programs are not running and
its apps not installed, and the floor lets it: never `~`, `~/Library` or what
is directly in them, never outside `~` and your `/var/folders` entry, never a
git repository or anything git tracks, never through a symlink or onto
another volume, never another user's file
([ADR 0001](docs/adr/0001-safety-from-confirmed-rules-and-a-fixed-floor.md)).

## Install

```sh
brew install sunfmin/tap/diskreap
gh auth login        # once: the Rules live in a gist of yours
diskreap doctor
```

**For an AI agent**: a skill that sends an agent to `diskreap --help` installs
with:

```sh
npx skills add sunfmin/diskreap
```

macOS only, Apache-2.0 ([ADR 0002](docs/adr/0002-macos-only-and-pure-crates-checked-by-flux.md)).

## Developing

`./gate.sh` decides whether a change may land, run where the change is made: `cargo fmt
--check`, `./verify.sh` (Flux over the pure crates, each of which must report `0
trusted; 0 ignored`, as `./flux-summary.sh` reads it; `./flux-summary-test.sh`
tests that reading), `cargo clippy -D warnings` and `cargo nextest run`. It
runs on rustup's `nightly-2026-09-25` and needs the checker Cargo.toml pins:

- cargo-flux, built from the Flux revision pinned there:
  `git clone https://github.com/flux-rs/flux && cd flux && git checkout <rev> && cargo xtask install`
- fixpoint, built at the commit pinned there:
  https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install
- z3 at the version pinned there: https://github.com/Z3Prover/z3/releases
- cargo-nextest: `cargo install cargo-nextest --locked`

`./verify.sh` names the one to install when one is missing or another build.
Each pure crate is listed in `verify.sh`'s `PURE`: a new one is added there,
or Flux never checks it. Terms are in [GLOSSARY.md](GLOSSARY.md), decisions
in [docs/adr](docs/adr). Tests run the real binary with `gh`, `lsof`, `ps`
and `mdfind` stood in for by `crates/cli/examples/standin.rs`; a debug build
reads `DISKREAP_NOW` and `DISKREAP_LARGE` so tests can move the clock and
shrink a gigabyte, and a release build reads neither.

## Cutting a release

Set `version` in `Cargo.toml`, merge, then push the tag `v<version>`. The
[release workflow](.github/workflows/release.yml) builds both macOS binaries
on stable Rust, makes this repository's release `v<version>` of the two
archives, `SHA256SUMS` and the formula generated from them, and pushes the
formula to [`sunfmin/homebrew-tap`](https://github.com/sunfmin/homebrew-tap)
as `Formula/diskreap.rb`. The release needs nothing but the workflow's own
token; the tap is a second repository, which that token cannot write, so its
step pushes with the repository secret `HOMEBREW_TAP_DEPLOY_KEY`, an SSH
deploy key that may write the tap, and fails without it, naming the line
below that does it by hand.

By hand it is the same steps, on a Mac, with the tag's commit checked out and
from a shell where `gh` and `git` may write this repository and the tap:

```sh
packaging/release.sh check "$(git describe --tags --exact-match)"   # the commit checked out is v<version>
packaging/release.sh build      # both targets, on rustup's stable toolchain
packaging/release.sh package    # dist/diskreap-<version>-macos-<arch>.tar.gz, each checked; SHA256SUMS; diskreap.rb
packaging/release.sh publish    # release, then tap: the files to the release v<version>, the formula to the tap
```

`publish` is two steps, each of which runs alone too, and again after a
failure: `release` alone makes or refills the GitHub release, and

```sh
packaging/release.sh release    # dist/ to this repository's release v<version>
packaging/release.sh tap        # dist/diskreap.rb to the tap: what CI does with HOMEBREW_TAP_DEPLOY_KEY
```

`build` uses rustup's stable toolchain whatever `cargo` is first on `PATH`,
and names the line that installs it when it, or the Intel target, is
missing. `package` refuses a binary of the wrong arch, or one whose
`--version` is not the release: build at the tag, with no changes.
