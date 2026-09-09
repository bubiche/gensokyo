#!/bin/sh
# install.sh - install gensokyo, from a release or from a checkout.
#
#   curl -fsSL https://github.com/bubiche/gensokyo/releases/latest/download/install.sh | sh
#                                 download the latest release into ~/.gensokyo and link it
#   ./install.sh                  from a checkout or an unpacked tarball: link this copy
#   ./install.sh --bin-dir DIR    put the link in DIR (also: GENSOKYO_BIN_DIR)
#   ./install.sh --dir DIR        download into DIR instead of ~/.gensokyo (also: GENSOKYO_DIR)
#   ./install.sh --version 0.1.0  download that release instead of the latest (GENSOKYO_VERSION)
#   ./install.sh --no-fetch       never download: fail unless vendored or acceptable system
#                                 binaries are already here
#   (piped into sh, options go after `sh -s --`: ... | sh -s -- --bin-dir ~/bin)
#
# POSIX sh on purpose (a release pipes this to `sh`). Nothing is written outside the gensokyo
# directory except the one symlink. ~/.tmux.conf and ~/.claude are never touched.
# tmux >= 3.3 and jq >= 1.6 are needed; if vendor/<os>-<arch>/ has them they are used, else an
# acceptable system pair is accepted, else scripts/vendor.sh downloads and verifies them.

set -eu

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

# The usage lines of the header, by their shape rather than by line number, so that editing the
# header cannot silently print the wrong thing. Piped into `sh` there is no $0 to read at all,
# which is why --help has a second answer below.
usage() { sed -n 's/^#   //p' "$0"; }

# Where the releases are. Overridable so that a test can point at a directory of tarballs
# (curl speaks file://) and so that a fork can install its own build.
REPO=${GENSOKYO_REPO:-bubiche/gensokyo}
BASE=${GENSOKYO_RELEASE_BASE:-https://github.com/$REPO/releases}

bin_dir=${GENSOKYO_BIN_DIR:-$HOME/.local/bin}
dir=${GENSOKYO_DIR:-$HOME/.gensokyo}
version=${GENSOKYO_VERSION:-}
fetch=1
while [ $# -gt 0 ]; do
  case $1 in
    --bin-dir) [ $# -ge 2 ] || die "--bin-dir needs a directory"; bin_dir=$2; shift ;;
    --bin-dir=*) bin_dir=${1#--bin-dir=} ;;
    --dir) [ $# -ge 2 ] || die "--dir needs a directory"; dir=$2; shift ;;
    --dir=*) dir=${1#--dir=} ;;
    --version) [ $# -ge 2 ] || die "--version needs a release version"; version=$2; shift ;;
    --version=*) version=${1#--version=} ;;
    --no-fetch) fetch=0 ;;
    -h|--help)
      if [ -f "$0" ] && grep -q '^# install.sh' "$0" 2>/dev/null; then usage
      else say "install.sh: curl -fsSL $BASE/latest/download/install.sh | sh -s -- [--bin-dir DIR] [--dir DIR] [--version VER]"
      fi
      exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------- platform
os=$(uname -s) arch=$(uname -m)
case $os in Darwin) os=macos ;; *) die "unsupported OS: $os (gensokyo is macOS only)" ;; esac
case $arch in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x86_64 ;; *) die "unsupported architecture: $arch (arm64 and x86_64 only)" ;; esac
platform="$os-$arch"

command -v bash >/dev/null 2>&1 || die "bash is required (bin/gensokyo runs under bash 3.2 or newer)"

# ver_ge <a> <b>: dotted version a >= b, letters ignored (3.7c >= 3.3). The same awk program
# is in bin/gensokyo.
ver_ge() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    gsub(/[^0-9.]/, "", a); gsub(/[^0-9.]/, "", b)
    split(a, x, "."); split(b, y, ".")
    for (i = 1; i <= 3; i++) { p = x[i] + 0; q = y[i] + 0; if (p > q) exit 0; if (p < q) exit 1 }
    exit 0 }'
}
system_ok() { # <name> <min> <version flag>: an acceptable <name> is on PATH
  v=$("$1" "$3" 2>/dev/null | head -n 1 | sed 's/^[^0-9]*//') || return 1
  [ -n "$v" ] && ver_ge "$v" "$2"
}

sha256_of() {   # the same three-way fallback as scripts/vendor.sh
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | sed 's/.* //'
  else die "need shasum, sha256sum or openssl to verify the download"
  fi
}

# ---------------------------------------------------------------- am I next to a gensokyo?
# Piped into `sh`, $0 is the shell itself and there is no tree here: that is the download path.
# A file $0 with bin/gensokyo beside it is a checkout or an unpacked tarball, which installs
# itself where it lies.
root=''
if [ -f "$0" ]; then
  here=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || here=''
  [ -n "$here" ] && [ -f "$here/bin/gensokyo" ] && root=$here
fi

# ---------------------------------------------------------------- the download path
# GitHub serves releases/latest/download/<asset> for whatever release is newest, so the version
# comes from a one-line VERSION asset rather than from the API, which rate-limits by IP.
download_release() {
  command -v curl >/dev/null 2>&1 || die "curl is required to download a release"
  if [ -e "$dir/bin/gensokyo" ]; then
    die "$dir already holds gensokyo: run 'gensokyo update' for the latest release, or --dir for somewhere else"
  fi
  if [ -z "$version" ]; then
    version=$(curl -fsSL "$BASE/latest/download/VERSION" 2>/dev/null | head -n 1 | tr -d ' \r') || version=''
    [ -n "$version" ] || die "cannot tell which release is the latest ($BASE/latest/download/VERSION did not answer); pass --version, or unpack a tarball and run its ./install.sh"
  fi
  name="gensokyo-$version-$platform.tar.gz"
  work=$(mktemp -d "${TMPDIR:-/tmp}/gensokyo-install.XXXXXX") || die "cannot make a temporary directory"
  trap 'rm -rf "$work"' EXIT INT TERM
  say "gensokyo $version: downloading $name"
  curl -fsSL --retry 3 -o "$work/$name" "$BASE/download/v$version/$name" ||
    die "cannot download $BASE/download/v$version/$name (no such release, or no build for $platform)"
  curl -fsSL --retry 3 -o "$work/SHA256SUMS" "$BASE/download/v$version/SHA256SUMS" ||
    die "cannot download the release's SHA256SUMS; refusing to install an unverified tarball"
  want=$(awk -v f="$name" '$2 == f { print $1 }' "$work/SHA256SUMS")
  [ -n "$want" ] || die "the release's SHA256SUMS has no line for $name"
  got=$(sha256_of "$work/$name")
  [ "$want" = "$got" ] || die "checksum mismatch for $name
  expected $want
  got      $got
Refusing to install. Do not bypass this."
  mkdir -p "$work/x"
  tar -xzf "$work/$name" -C "$work/x" || die "cannot unpack $name"
  [ -f "$work/x/gensokyo-$version/bin/gensokyo" ] || die "$name does not contain gensokyo-$version/bin/gensokyo"
  parent=$(dirname "$dir")
  mkdir -p "$parent" || die "cannot create $parent"
  # Move into place rather than copying over anything: nothing half-installed is ever left.
  [ -d "$dir" ] && { rmdir "$dir" 2>/dev/null || die "$dir exists and is not empty"; }
  mv "$work/x/gensokyo-$version" "$dir" || die "cannot move the unpacked tree into $dir"
  root=$(cd "$dir" && pwd)
  say "gensokyo $version: unpacked into $root"
}

if [ -z "$root" ]; then
  [ "$fetch" = 1 ] ||
    die "no gensokyo here and --no-fetch given: run this from an unpacked gensokyo directory (./install.sh)"
  download_release
fi

# ---------------------------------------------------------------- tmux + jq
vendor="$root/vendor/$platform"
if [ -x "$vendor/tmux" ] && [ -x "$vendor/jq" ]; then
  say "tmux + jq: vendored ($vendor)"
elif system_ok tmux 3.3 -V && system_ok jq 1.6 --version; then
  say "tmux + jq: using the system's ($(command -v tmux), $(command -v jq))"
elif [ "$fetch" = 1 ]; then
  say "tmux + jq: fetching for $platform (verified against vendor/SHA256SUMS)"
  sh "$root/scripts/vendor.sh" "$platform"
else
  die "no tmux >= 3.3 and jq >= 1.6 found and --no-fetch given: run scripts/vendor.sh"
fi
# Browser-downloaded tarballs carry the quarantine attribute; curl-fetched files do not.
if [ "$os" = macos ] && [ -d "$vendor" ] && command -v xattr >/dev/null 2>&1; then
  xattr -dr com.apple.quarantine "$vendor" 2>/dev/null || :
fi

# ---------------------------------------------------------------- the symlink
mkdir -p "$bin_dir" || die "cannot create $bin_dir"
bin_dir=$(cd "$bin_dir" && pwd)
link="$bin_dir/gensokyo"
if [ -e "$link" ] && [ ! -L "$link" ]; then
  die "$link exists and is not a symlink; remove it or use --bin-dir"
fi
ln -sfn "$root/bin/gensokyo" "$link"
say "linked $link -> $root/bin/gensokyo"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) say "note: $bin_dir is not on your PATH; add this to your shell rc:"
     say "  export PATH=\"$bin_dir:\$PATH\"" ;;
esac
command -v claude >/dev/null 2>&1 ||
  say "note: claude is not on PATH; gensokyo needs Claude Code >= 2.1.224 (https://code.claude.com)"

say "done. Try: gensokyo doctor, then gensokyo"
