#!/bin/sh
# install.sh - install gensokyo from a release, or link an unpacked one.
#
#   curl -fsSL https://github.com/bubiche/gensokyo/releases/latest/download/install.sh | sh
#                                 download the latest release into ~/.gensokyo and link it
#   ./install.sh                  from an unpacked tarball: link this copy
#   ./install.sh --bin-dir DIR    put the link in DIR (also: GENSOKYO_BIN_DIR)
#   ./install.sh --dir DIR        download into DIR instead of ~/.gensokyo (also: GENSOKYO_DIR)
#   ./install.sh --version 0.2.0  download that release instead of the latest (GENSOKYO_VERSION)
#   ./install.sh --no-fetch       never download: fail unless run from an unpacked tarball
#   ./install.sh --update         from an installed copy: replace it with the latest release
#                                 (or --version), which is what `gensokyo update` runs
#   (piped into sh, options go after `sh -s --`: ... | sh -s -- --bin-dir ~/bin)
#
# POSIX sh on purpose (a release pipes this to `sh`). Nothing is written outside the gensokyo
# directory except the one symlink. ~/.claude is never touched.

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
fetch=1 update=0
while [ $# -gt 0 ]; do
  case $1 in
    --bin-dir) [ $# -ge 2 ] || die "--bin-dir needs a directory"; bin_dir=$2; shift ;;
    --bin-dir=*) bin_dir=${1#--bin-dir=} ;;
    --dir) [ $# -ge 2 ] || die "--dir needs a directory"; dir=$2; shift ;;
    --dir=*) dir=${1#--dir=} ;;
    --version) [ $# -ge 2 ] || die "--version needs a release version"; version=$2; shift ;;
    --version=*) version=${1#--version=} ;;
    --no-fetch) fetch=0 ;;
    --update) update=1 ;;
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
case $arch in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x86_64 ;; *) die "unsupported architecture: $arch" ;; esac
platform="$os-$arch"

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | sed 's/.* //'
  else die "need shasum, sha256sum or openssl to verify the download"
  fi
}

# ---------------------------------------------------------------- am I in a release tree?
# Piped into `sh`, $0 is the shell itself and there is no tree here: that is the download path.
# A file $0 with bin/gensokyo and VERSION beside it is an unpacked tarball, which installs
# itself where it lies.
root=''
if [ -f "$0" ]; then
  here=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || here=''
  [ -n "$here" ] && [ -f "$here/bin/gensokyo" ] && [ -f "$here/VERSION" ] && root=$here
fi

# ---------------------------------------------------------------- downloading
# GitHub serves releases/latest/download/<asset> for whatever release is newest, so the version
# comes from a one-line VERSION asset rather than from the API, which rate-limits by IP.
latest() {
  version=$(curl -fsSL "$BASE/latest/download/VERSION" 2>/dev/null | head -n 1 | tr -d ' \r') || version=''
  [ -n "$version" ] || die "cannot tell which release is the latest ($BASE/latest/download/VERSION did not answer); pass --version"
}

# fetch_release <workdir>: download $version for this platform and the release's SHA256SUMS,
# verify one against the other and unpack it into <workdir>/x. Nothing that did not verify is
# ever unpacked.
fetch_release() {
  command -v curl >/dev/null 2>&1 || die "curl is required to download a release"
  name="gensokyo-$version-$platform.tar.gz"
  say "gensokyo $version: downloading $name"
  curl -fsSL --retry 3 -o "$1/$name" "$BASE/download/v$version/$name" ||
    die "cannot download $BASE/download/v$version/$name (no such release, or no build for $platform)"
  curl -fsSL --retry 3 -o "$1/SHA256SUMS" "$BASE/download/v$version/SHA256SUMS" ||
    die "cannot download the release's SHA256SUMS; refusing to install an unverified tarball"
  want=$(awk -v f="$name" '$2 == f { print $1 }' "$1/SHA256SUMS")
  [ -n "$want" ] || die "the release's SHA256SUMS has no line for $name"
  got=$(sha256_of "$1/$name")
  [ "$want" = "$got" ] || die "checksum mismatch for $name
  expected $want
  got      $got
Refusing to install. Do not bypass this."
  mkdir -p "$1/x"
  tar -xzf "$1/$name" -C "$1/x" || die "cannot unpack $name"
  [ -x "$1/x/gensokyo-$version/bin/gensokyo" ] || die "$name does not contain gensokyo-$version/bin/gensokyo"
}

# A version goes into paths and URLs, whether it was given or read off the VERSION asset.
checked() {
  case $version in
    *[!0-9A-Za-z.+_-]*|'') die "refusing version '$version': letters, digits and . + _ - only" ;;
  esac
}

# ---------------------------------------------------------------- --update
# The new tree is unpacked next to the old one so that putting it in place is a rename on the
# same filesystem: the swap is one instant, not a copy that could be interrupted half-way. The
# link points into the tree by path, and config and state live outside it, so neither changes.
if [ "$update" = 1 ]; then
  [ -n "$root" ] || die "--update runs from an installed copy (its install.sh); this is not one"
  have=$(head -n 1 "$root/VERSION")
  [ -n "$version" ] || latest
  checked
  if [ "$version" = "$have" ]; then say "gensokyo $have is already the release you asked for"; exit 0; fi
  parent=$(dirname "$root")
  [ -w "$parent" ] || die "cannot write in $parent"
  work=$(mktemp -d "$parent/.gensokyo-update.XXXXXX") || die "cannot make a directory in $parent"
  trap 'rm -rf "$work"' EXIT INT TERM
  fetch_release "$work"
  old="$root.old.$$"
  mv "$root" "$old" || die "cannot move $root aside"
  if ! mv "$work/x/gensokyo-$version" "$root"; then
    mv "$old" "$root" || say "install.sh: and the old copy is still at $old" >&2
    die "cannot move the new tree into $root; nothing was changed"
  fi
  rm -rf "$old"
  say "gensokyo $have -> $version in $root"
  exit 0
fi

# ---------------------------------------------------------------- the download path
if [ -z "$root" ]; then
  [ "$fetch" = 1 ] ||
    die "no gensokyo here and --no-fetch given: run this from an unpacked gensokyo directory (./install.sh)"
  if [ -e "$dir/bin/gensokyo" ]; then
    die "$dir already holds gensokyo: run 'gensokyo update' for the latest release, or --dir for somewhere else"
  fi
  [ -n "$version" ] || latest
  checked
  work=$(mktemp -d "${TMPDIR:-/tmp}/gensokyo-install.XXXXXX") || die "cannot make a temporary directory"
  trap 'rm -rf "$work"' EXIT INT TERM
  fetch_release "$work"
  parent=$(dirname "$dir")
  mkdir -p "$parent" || die "cannot create $parent"
  # Move into place rather than copying over anything: nothing half-installed is ever left.
  [ -d "$dir" ] && { rmdir "$dir" 2>/dev/null || die "$dir exists and is not empty"; }
  mv "$work/x/gensokyo-$version" "$dir" || die "cannot move the unpacked tree into $dir"
  root=$(cd "$dir" && pwd)
  say "gensokyo $version: unpacked into $root"
fi

# Browser-downloaded tarballs carry the quarantine attribute; curl-fetched files do not.
command -v xattr >/dev/null 2>&1 && xattr -dr com.apple.quarantine "$root" 2>/dev/null || :

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
  say "note: claude is not on PATH; gensokyo runs Claude Code (https://code.claude.com)"

say "done. Try: gensokyo doctor, then gensokyo"
