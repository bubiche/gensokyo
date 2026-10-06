#!/bin/sh
# scripts/release.sh - build the release tarball, its SHA256SUMS and VERSION.
#
#   scripts/release.sh                   build the version Cargo.toml carries into dist/
#   scripts/release.sh --version 0.2.0   refuse unless that is the version being built
#   scripts/release.sh --out DIR         build into DIR (default: dist/)
#   scripts/release.sh --bin PATH        package that binary instead of building one
#
# One tarball per architecture, each unpacking into one gensokyo-<version>/ directory:
#   bin/gensokyo  share/  install.sh  uninstall.sh  README.md  LICENSES/  VERSION
# VERSION in the tree is what marks it as a release install, for `gensokyo update`.
#
# Only arm64 is built: an x86_64 build would need the x86_64-apple-darwin Rust target and Zig
# building libghostty-vt for it, and nobody has run one.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)

say() { printf '%s\n' "$*"; }
die() { printf 'release.sh: %s\n' "$*" >&2; exit 1; }

out="$root/dist" version='' bin=''
while [ $# -gt 0 ]; do
  case $1 in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift ;;
    --version=*) version=${1#--version=} ;;
    --out) [ $# -ge 2 ] || die "--out needs a directory"; out=$2; shift ;;
    --out=*) out=${1#--out=} ;;
    --bin) [ $# -ge 2 ] || die "--bin needs a path"; bin=$2; shift ;;
    --bin=*) bin=${1#--bin=} ;;
    -h|--help) sed -n 's/^#   //p' "$0"; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------- the version
# The binary says its version from Cargo.toml at build time, so a release is only ever of the
# version Cargo.toml carries; a tag that says otherwise is refused rather than mislabelled.
cargo=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)
[ -n "$cargo" ] || die "no version in Cargo.toml"
[ -n "$version" ] || version=$cargo
# GENSOKYO_RELEASE_ANY_VERSION lets the tests label one binary as two releases, to update from
# one to the other.
[ "$version" = "$cargo" ] || [ -n "${GENSOKYO_RELEASE_ANY_VERSION:-}" ] ||
  die "asked for $version, but Cargo.toml is $cargo"
case $version in
  *[!0-9A-Za-z.+_-]*|'') die "refusing version '$version': letters, digits and . + _ - only" ;;
esac

if [ -z "$bin" ]; then
  ( cd "$root" && cargo build --release --locked ) || die "cargo build failed"
  bin="$root/target/release/gensokyo"
fi
[ -x "$bin" ] || die "no binary at $bin"
said=$("$bin" --version) || die "$bin does not run"
[ "$said" = "gensokyo $cargo" ] || die "$bin says '$said', not gensokyo $cargo"
license="$root/vendor/ghostty-739603b8a/LICENSE"
[ -f "$license" ] || die "no $license: run scripts/ghostty.sh"

# ---------------------------------------------------------------- stage the tree
work=$(mktemp -d "${TMPDIR:-/tmp}/gensokyo-release.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM
prefix="gensokyo-$version"
stage="$work/$prefix"
mkdir -p "$stage/bin" "$stage/LICENSES"
cp "$bin" "$stage/bin/gensokyo"
cp -R "$root/share" "$stage/"
cp "$root/install.sh" "$root/uninstall.sh" "$root/README.md" "$stage/"
cp "$license" "$stage/LICENSES/ghostty.txt"
printf '%s\n' "$version" > "$stage/VERSION"
chmod 755 "$stage/bin/gensokyo" "$stage/install.sh" "$stage/uninstall.sh"
# An arm64 binary whose signature does not check is killed as it starts.
codesign -v "$stage/bin/gensokyo" || die "the packaged binary's signature does not verify"
[ "$("$stage/bin/gensokyo" --version)" = "gensokyo $cargo" ] || die "the packaged binary does not run"

# ---------------------------------------------------------------- pack
mkdir -p "$out" || die "cannot create $out"
out=$(cd "$out" && pwd)
name="$prefix-macos-arm64.tar.gz"
rm -f "$out/$name"
# COPYFILE_DISABLE keeps bsdtar from packing macOS extended attributes as ._ files, which
# a GNU tar on the other end would unpack as litter.
( cd "$work" && COPYFILE_DISABLE=1 tar -czf "$out/$name" "$prefix" )
list=$(tar -tzf "$out/$name")
for want in bin/gensokyo share/names.txt share/plugin/.claude-plugin/plugin.json install.sh \
            uninstall.sh README.md LICENSES/ghostty.txt VERSION share/probes/gh-prs; do
  printf '%s\n' "$list" | grep -q "^$prefix/$want\$" || die "$name is missing $want"
done
# A probe that is not executable is refused when a ritual names it.
tar -tvzf "$out/$name" | grep " $prefix/share/probes/gh-prs\$" | grep -q '^-rwx' ||
  die "$name has share/probes/gh-prs without its executable bit"
[ "$(printf '%s\n' "$list" | grep -c "^$prefix/bin/.")" = 1 ] || die "$name has more than gensokyo in bin/"
printf '%s\n' "$list" | grep -q '/\._' && die "$name carries AppleDouble files"
say "$name  $(wc -c < "$out/$name" | tr -d ' ') bytes"

( cd "$out" && shasum -a 256 "$name" > SHA256SUMS )
# VERSION goes up as a release asset too: GitHub serves releases/latest/download/VERSION for
# whatever release is newest, so install.sh and `gensokyo update` never call the API.
printf '%s\n' "$version" > "$out/VERSION"
cp "$root/install.sh" "$root/uninstall.sh" "$out/"
say "built $version in $out"
