#!/bin/sh
# What a release is made of, one step per subcommand, so the workflow
# (.github/workflows/release.yml) and a human at a terminal run the same thing.
#
#   release.sh version          the version Cargo builds
#   release.sh check <tag>      fail unless <tag> is v<version>
#   release.sh build            both macOS binaries, Apple Silicon and Intel,
#                               on rustup's stable toolchain
#   release.sh package          what was built, checked, to
#                               dist/diskreap-<version>-macos-<arch>.tar.gz, with
#                               dist/SHA256SUMS and the formula dist/diskreap.rb
#   release.sh formula          the Homebrew formula for the archives in dist/
#   release.sh release          what is in dist/ to this repository's release
#                               v<version>, which the formula downloads from
#   release.sh tap              dist/diskreap.rb to the tap, as Formula/diskreap.rb
#   release.sh publish          release, then tap
#
# A release by hand is `check`, `build`, `package`, `publish`, at the tag's
# commit, from a shell where `gh` and `git` may write this repository and the
# tap. Each step can run again: a publish that failed half way is run again.
#
# DIST names another directory than dist/; CARGO_TARGET_DIR is where `build`
# put the binaries, as for cargo.
set -eu
cd "$(dirname "$0")/.."

NAME=diskreap
REPO=sunfmin/diskreap
TAP=sunfmin/homebrew-tap
ARCHES="arm64 x86_64"
DIST=${DIST:-dist}
TARGET_DIR=${CARGO_TARGET_DIR:-target}

version() {
    cargo pkgid -p "$NAME" | sed 's/.*[#@]//'
}

# The Rust target that builds for an arch.
target() {
    case "$1" in
    arm64) echo aarch64-apple-darwin ;;
    *) echo "$1-apple-darwin" ;;
    esac
}

archive() {
    echo "$NAME-$(version)-macos-$1.tar.gz"
}

sha() {
    shasum -a 256 "$1" | cut -d' ' -f1
}

# Fails, naming the step to run first, unless every archive is in dist/.
packaged() {
    for arch in $ARCHES; do
        [ -f "$DIST/$(archive "$arch")" ] || {
            echo "release: $DIST/$(archive "$arch") is missing: package it first (packaging/release.sh build, then package)" >&2
            exit 1
        }
    done
}

case "${1:-}" in
version)
    version
    ;;
check)
    tag=${2:?usage: release.sh check <tag>}
    if [ "$tag" != "v$(version)" ]; then
        echo "release: the tag $tag is not v$(version), the version in Cargo.toml: set the version there, or tag v$(version)" >&2
        exit 1
    fi
    ;;
build)
    # The toolchain's own bin goes first on PATH. A `cargo` that is ahead of
    # rustup's there (Homebrew's) has no Intel target, and `rustup run stable
    # cargo` is not enough: the cargo it starts still finds the `rustc` that
    # is first on PATH. RUSTUP_TOOLCHAIN would override stable: the gate's
    # nightly is not what a release is built with.
    unset RUSTUP_TOOLCHAIN
    targets=$(for arch in $ARCHES; do target "$arch"; done)
    rustup=$(PATH="$PATH:$HOME/.cargo/bin" command -v rustup) || rustup=
    stable=$([ -n "$rustup" ] && "$rustup" which --toolchain stable cargo 2>/dev/null) || stable=
    installed=$([ -n "$stable" ] && "$rustup" target list --installed --toolchain stable) || installed=
    for target in $targets; do
        echo "$installed" | grep -qx "$target" && continue
        # shellcheck disable=SC2086
        cat >&2 <<EOF
release: build needs rustup's stable toolchain with the target $target, and ${rustup:+that is not installed}${rustup:-no rustup is installed (https://rustup.rs)}. With rustup, get it with:
    rustup toolchain install stable --profile minimal --target $(echo $targets | tr " " ,)
EOF
        exit 1
    done
    PATH="$(dirname "$stable"):$PATH"
    export PATH
    for target in $targets; do
        cargo build --release --locked -p "$NAME" --target "$target"
    done
    ;;
package)
    for arch in $ARCHES; do
        built=$TARGET_DIR/$(target "$arch")/release/$NAME
        [ -f "$built" ] || {
            echo "release: $built is missing: build it first (packaging/release.sh build)" >&2
            exit 1
        }
    done
    for arch in $ARCHES; do
        built=$TARGET_DIR/$(target "$arch")/release/$NAME
        # Each is the arch it is packaged as, and `diskreap --version` says the
        # release. A binary this machine cannot run (Intel, with no Rosetta)
        # is only checked for its arch.
        lipo -archs "$built" | grep -qx "$arch" || {
            echo "release: $built is not an $arch binary: build it again (packaging/release.sh build)" >&2
            exit 1
        }
        if arch "-$arch" /usr/bin/true 2>/dev/null; then
            said=$(arch "-$arch" "$built" --version)
            if [ "$said" != "$NAME $(version)" ]; then
                echo "release: $built --version says '$said', not '$NAME $(version)': build it at the commit tagged v$(version), with no changes" >&2
                exit 1
            fi
        else
            echo "release: this machine cannot run $arch: its --version was not checked" >&2
        fi
    done
    mkdir -p "$DIST"
    for arch in $ARCHES; do
        built=$TARGET_DIR/$(target "$arch")/release/$NAME
        tar -czf "$DIST/$(archive "$arch")" -C "$(dirname "$built")" "$NAME"
        echo "$DIST/$(archive "$arch")"
    done
    (cd "$DIST" && for arch in $ARCHES; do shasum -a 256 "$(archive "$arch")"; done) > "$DIST/SHA256SUMS"
    "$0" formula > "$DIST/$NAME.rb"
    ruby -c "$DIST/$NAME.rb" >/dev/null
    echo "$DIST/SHA256SUMS"
    echo "$DIST/$NAME.rb"
    ;;
formula)
    packaged
    base=https://github.com/$REPO/releases/download/v$(version)
    cat <<FORMULA
class Diskreap < Formula
  desc "Agent CLI that frees disk space by Rules kept in a private gist, safely"
  homepage "https://github.com/$REPO"
  version "$(version)"
  license "Apache-2.0"

  depends_on :macos
  depends_on "gh"

  on_arm do
    url "$base/$(archive arm64)"
    sha256 "$(sha "$DIST/$(archive arm64)")"
  end
  on_intel do
    url "$base/$(archive x86_64)"
    sha256 "$(sha "$DIST/$(archive x86_64)")"
  end

  def install
    bin.install "diskreap"
  end

  def caveats
    <<~EOS
      diskreap keeps its Rules in a private gist of yours, reached through gh.
      Log gh in with the gist scope once:
        gh auth login
      then \`diskreap doctor\` names whatever else is missing, with its fix,
      and \`diskreap init\` makes the gist.
    EOS
  end

  test do
    assert_equal "diskreap #{version}", shell_output("#{bin}/diskreap --version").strip
  end
end
FORMULA
    ;;
release)
    packaged
    for file in SHA256SUMS "$NAME.rb"; do
        [ -f "$DIST/$file" ] || {
            echo "release: $DIST/$file is missing: package it first (packaging/release.sh package)" >&2
            exit 1
        }
    done
    files="$DIST/SHA256SUMS $DIST/$NAME.rb"
    for arch in $ARCHES; do
        files="$files $DIST/$(archive "$arch")"
    done
    # A release that is there already takes the files again.
    if gh release view "v$(version)" -R "$REPO" >/dev/null 2>&1; then
        # shellcheck disable=SC2086
        gh release upload "v$(version)" -R "$REPO" --clobber $files
    else
        # shellcheck disable=SC2086
        gh release create "v$(version)" -R "$REPO" --verify-tag --title "$NAME $(version)" \
            --generate-notes $files
    fi
    ;;
tap)
    formula=$DIST/$NAME.rb
    [ -f "$formula" ] || {
        echo "release: $formula is missing: package it first (packaging/release.sh build, then package)" >&2
        exit 1
    }
    tap=$(mktemp -d)
    trap 'rm -rf "$tap"' EXIT
    # CI pushes with an SSH deploy key (HOMEBREW_TAP_SSH_KEY, a key file),
    # because its own repository token cannot write another repository; a
    # person at a terminal sets none and pushes over HTTPS with gh's auth.
    if [ -n "${HOMEBREW_TAP_SSH_KEY:-}" ]; then
        GIT_SSH_COMMAND="ssh -i $HOMEBREW_TAP_SSH_KEY -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"
        export GIT_SSH_COMMAND
        git clone --quiet --depth 1 "git@github.com:$TAP.git" "$tap"
    else
        gh repo clone "$TAP" "$tap" -- --quiet --depth 1
    fi
    mkdir -p "$tap/Formula"
    cp "$formula" "$tap/Formula/$NAME.rb"
    git -C "$tap" add "Formula/$NAME.rb"
    if git -C "$tap" diff --cached --quiet; then
        echo "release: the tap's formula is $NAME $(version) already"
    else
        git -C "$tap" commit --quiet -m "$NAME $(version)"
        git -C "$tap" push --quiet origin HEAD
    fi
    ;;
publish)
    "$0" release
    "$0" tap
    ;;
*)
    sed -n '2,24p' "$0" >&2
    exit 2
    ;;
esac
