#!/bin/sh
# RecordAgent installer.
#
#   curl -fsSL https://raw.githubusercontent.com/alexromer0/recordagent/main/install.sh | sh
#
# Downloads the release binary for this platform, verifies its checksum,
# and puts it somewhere on PATH. Nothing else — no daemon started, no
# config written, no service registered. `recordagent init` does that,
# and it should be a decision you make rather than one an installer makes
# for you.
#
# Environment:
#   RECORDAGENT_VERSION   tag to install (default: latest release)
#   RECORDAGENT_BIN_DIR   where to put it (default: ~/.local/bin, or
#                         /usr/local/bin when running as root)
#
# POSIX sh, not bash: this is piped into whatever /bin/sh is on a machine
# nobody has looked at yet.
set -eu

REPO="alexromer0/recordagent"
VERSION="${RECORDAGENT_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "this installer needs $1"
}

need uname
need mktemp

# curl or wget, whichever exists. Alpine images routinely have only wget.
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_stdout() { wget -qO- "$1"; }
else
    die "this installer needs curl or wget"
fi

# --- platform --------------------------------------------------------

os=$(uname -s)
arch=$(uname -m)

case "$os" in
    Linux)  os_target="unknown-linux-gnu" ;;
    Darwin) os_target="apple-darwin" ;;
    *)      die "unsupported OS: $os. Build from source, or use the Docker image:
    docker run -p 7070:7070 ghcr.io/$REPO" ;;
esac

case "$arch" in
    x86_64|amd64)  arch_target="x86_64" ;;
    arm64|aarch64) arch_target="aarch64" ;;
    *)             die "unsupported architecture: $arch" ;;
esac

# There is no x86_64 macOS build. Rosetta runs the arm64 one, and an
# Intel Mac is a rarer thing than a cross-compile target worth
# maintaining — say so rather than 404ing on a download.
if [ "$os" = "Darwin" ] && [ "$arch_target" = "x86_64" ]; then
    die "no Intel macOS build is published. Use the Docker image:
    docker run -p 7070:7070 ghcr.io/$REPO"
fi

target="${arch_target}-${os_target}"
asset="recordagent-${target}.tar.gz"

# --- version ---------------------------------------------------------

if [ "$VERSION" = "latest" ]; then
    say "==> resolving the latest release"
    # Parsed out of the API response rather than following the /latest
    # redirect, so the version can be printed and echoed in the checksum
    # URL below.
    VERSION=$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
        | head -1)
    [ -n "$VERSION" ] || die "could not determine the latest version.
Set RECORDAGENT_VERSION=v0.1.0 to install a specific one."
fi

base="https://github.com/$REPO/releases/download/$VERSION"

# --- destination -----------------------------------------------------

if [ -n "${RECORDAGENT_BIN_DIR:-}" ]; then
    bin_dir="$RECORDAGENT_BIN_DIR"
elif [ "$(id -u)" = "0" ]; then
    bin_dir="/usr/local/bin"
else
    bin_dir="$HOME/.local/bin"
fi

mkdir -p "$bin_dir" || die "cannot create $bin_dir"
[ -w "$bin_dir" ] || die "$bin_dir is not writable.
Set RECORDAGENT_BIN_DIR to somewhere you can write, or re-run with sudo."

# --- download --------------------------------------------------------

tmp=$(mktemp -d)
# Cleans up on failure too. A partially downloaded tarball left in /tmp
# is the kind of thing that makes the *next* run mysterious.
trap 'rm -rf "$tmp"' EXIT INT TERM

say "==> downloading recordagent $VERSION ($target)"
fetch "$base/$asset" "$tmp/$asset" \
    || die "could not download $base/$asset
Check that $VERSION exists and publishes a $target build."

# --- verify ----------------------------------------------------------
#
# Not optional. This script is piped into a shell from the internet; a
# tarball that arrived over a hijacked CDN would otherwise run as you.
say "==> verifying checksum"
if fetch "$base/$asset.sha256" "$tmp/$asset.sha256" 2>/dev/null; then
    expected=$(cut -d' ' -f1 < "$tmp/$asset.sha256")

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
    else
        die "no sha256sum or shasum available to verify the download.
Install one, or download and verify manually from
$base/$asset"
    fi

    [ "$expected" = "$actual" ] || die "checksum mismatch.
  expected $expected
  actual   $actual
Do not run this binary. Report it at https://github.com/$REPO/issues"
    say "    ok"
else
    die "no checksum published for $asset at $base/$asset.sha256.
Refusing to install an unverified binary."
fi

# --- install ---------------------------------------------------------

tar -xzf "$tmp/$asset" -C "$tmp" || die "could not extract $asset"
[ -f "$tmp/recordagent" ] || die "the archive did not contain a recordagent binary"

chmod +x "$tmp/recordagent"
# `mv` then `chmod` rather than `install`, which busybox lacks.
mv "$tmp/recordagent" "$bin_dir/recordagent" \
    || die "could not write $bin_dir/recordagent"

say "==> installed $bin_dir/recordagent"

# --- next ------------------------------------------------------------

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
        say ""
        say "$bin_dir is not on your PATH. Add it:"
        say "    export PATH=\"$bin_dir:\$PATH\""
        ;;
esac

say ""
say "Next:"
say "    recordagent init                 # write a config and data dir"
say "    recordagent serve &              # start the daemon"
say "    recordagent user add \$USER"
say "    recordagent key issue --user \$USER --scopes read,write"
say ""
say "Docs: https://github.com/$REPO#readme"
