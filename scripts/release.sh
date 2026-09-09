#!/bin/sh
# scripts/release.sh - build the release tarballs and their SHA256SUMS.
#
#   scripts/release.sh                   build for the version this checkout carries
#   scripts/release.sh --version 0.1.0   stamp that version into the packaged bin/gensokyo
#   scripts/release.sh --out DIR         build into DIR (default: dist/)
#   scripts/release.sh --no-fetch        fail rather than download missing vendored binaries
#
# POSIX sh, like install.sh and vendor.sh. Three tarballs come out, each unpacking into one
# gensokyo-<version>/ directory with install.sh in it: one per platform, and an "all" one for
# copying to a machine that cannot download. The vendored binaries are the only thing that
# differs between them; everything else is this checkout, with the version stamped in.
#
# What ships: bin/ lib/ share/, install.sh, uninstall.sh and README.md, scripts/vendor.sh (install.sh calls
# it when the vendored binaries are missing), vendor/SHA256SUMS + vendor/README.md (so a
# re-fetch on the user's machine still verifies against the pins) and vendor/LICENSES/ (the
# notices for the binaries we ship). The SHA256SUMS of the tarballs themselves stays outside
# them, next to them in the output directory.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
PLATFORMS="macos-arm64 macos-x86_64"   # gensokyo is macOS only

say() { printf '%s\n' "$*"; }
die() { printf 'release.sh: %s\n' "$*" >&2; exit 1; }

sha256_of() {   # the same three-way fallback as vendor.sh: shasum is on every macOS
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | sed 's/.* //'
  else die "need shasum, sha256sum or openssl"
  fi
}

out="$root/dist" version='' fetch=1
while [ $# -gt 0 ]; do
  case $1 in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift ;;
    --version=*) version=${1#--version=} ;;
    --out) [ $# -ge 2 ] || die "--out needs a directory"; out=$2; shift ;;
    --out=*) out=${1#--out=} ;;
    --no-fetch) fetch=0 ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------- the version
# A tag on HEAD is what CI builds from (v0.1.0 -> 0.1.0); a checkout without one builds what
# bin/gensokyo says it is, which is how a local test build gets made.
if [ -z "$version" ]; then
  version=$(git -C "$root" describe --tags --exact-match 2>/dev/null) || version=''
  version=${version#v}
fi
if [ -z "$version" ]; then
  version=$(sed -n 's/^VERSION=//p' "$root/bin/gensokyo" | head -n 1)
  say "note: no --version and no tag on HEAD; building $version, the version in bin/gensokyo"
fi
[ -n "$version" ] || die "cannot work out a version to build"
case $version in
  *[!0-9A-Za-z.+_-]*) die "refusing version '$version': letters, digits and . + _ - only" ;;
esac

# ---------------------------------------------------------------- the binaries to package
missing=''
for plat in $PLATFORMS; do
  [ -x "$root/vendor/$plat/tmux" ] && [ -x "$root/vendor/$plat/jq" ] || missing="$missing $plat"
done
[ -f "$root/vendor/LICENSES/COPYING.tmux" ] || missing="$missing LICENSES"
if [ -n "$missing" ]; then
  [ "$fetch" = 1 ] || die "nothing vendored for:$missing - run scripts/vendor.sh --all (or drop --no-fetch)"
  say "vendored binaries missing for:$missing - fetching"
  sh "$here/vendor.sh" --all
fi

# ---------------------------------------------------------------- stage the tree
work=$(mktemp -d "${TMPDIR:-/tmp}/gensokyo-release.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM
prefix="gensokyo-$version"
stage="$work/$prefix"
mkdir -p "$stage/scripts" "$stage/vendor"
cp -R "$root/bin" "$root/lib" "$root/share" "$stage/"
cp "$root/install.sh" "$root/uninstall.sh" "$root/README.md" "$stage/"
cp "$root/scripts/vendor.sh" "$stage/scripts/vendor.sh"
cp "$root/vendor/SHA256SUMS" "$root/vendor/README.md" "$stage/vendor/"
cp -R "$root/vendor/LICENSES" "$stage/vendor/LICENSES"

# The packaged copy says which release it is; the checkout keeps its own -dev version.
sed "s/^VERSION=.*/VERSION=$version/" "$root/bin/gensokyo" > "$stage/bin/gensokyo"
chmod 755 "$stage/bin/gensokyo" "$stage/install.sh" "$stage/uninstall.sh" "$stage/scripts/vendor.sh"
stamped=$(bash "$stage/bin/gensokyo" version 2>&1) || die "the packaged bin/gensokyo does not run: $stamped"
[ "$stamped" = "gensokyo $version" ] || die "version stamp failed: '$stamped'"

# ---------------------------------------------------------------- pack
mkdir -p "$out" || die "cannot create $out"
out=$(cd "$out" && pwd)

pack() {   # pack <suffix> <platform...>: the vendor dirs are the only thing that varies
  name="$prefix-$1.tar.gz"
  shift
  rm -rf "$stage"/vendor/macos-*
  for p in "$@"; do cp -R "$root/vendor/$p" "$stage/vendor/$p"; done
  rm -f "$out/$name"
  # COPYFILE_DISABLE keeps bsdtar from packing macOS extended attributes as ._ files, which
  # a GNU tar on the other end would unpack as litter.
  ( cd "$work" && COPYFILE_DISABLE=1 tar -czf "$out/$name" "$prefix" )
  list=$(tar -tzf "$out/$name")
  for want in bin/gensokyo lib/rituals.sh share/tmux.conf share/plugin/.claude-plugin/plugin.json \
              install.sh uninstall.sh scripts/vendor.sh vendor/SHA256SUMS vendor/LICENSES/COPYING.tmux; do
    printf '%s\n' "$list" | grep -q "^$prefix/$want\$" || die "$name is missing $want"
  done
  for p in "$@"; do
    printf '%s\n' "$list" | grep -q "^$prefix/vendor/$p/tmux\$" || die "$name is missing vendor/$p/tmux"
    printf '%s\n' "$list" | grep -q "^$prefix/vendor/$p/jq\$" || die "$name is missing vendor/$p/jq"
  done
  printf '%s\n' "$list" | grep -q '/\._' && die "$name carries AppleDouble files"
  say "$name  $(wc -c < "$out/$name" | tr -d ' ') bytes"
}

pack macos-arm64 macos-arm64
pack macos-x86_64 macos-x86_64
# shellcheck disable=SC2086  # PLATFORMS is a word list on purpose
pack macos-all $PLATFORMS

# One tarball must not carry the other platform's binaries.
tar -tzf "$out/$prefix-macos-arm64.tar.gz" | grep -q 'vendor/macos-x86_64/' &&
  die "the arm64 tarball carries the x86_64 binaries"

( cd "$out" && rm -f SHA256SUMS
  for f in "$prefix"-*.tar.gz; do printf '%s  %s\n' "$(sha256_of "$f")" "$f"; done > SHA256SUMS )
say "SHA256SUMS"

# VERSION goes up as a release asset too, and it is how `curl | sh` and `gensokyo update` learn
# which release is the latest one: GitHub serves releases/latest/download/VERSION for whatever
# release is newest, so neither has to call the API and be rate-limited by it.
printf '%s\n' "$version" > "$out/VERSION"
say "VERSION"
say "built $version in $out"
