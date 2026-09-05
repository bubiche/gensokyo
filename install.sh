#!/bin/sh
# install.sh - install gensokyo from a git checkout or an extracted release tarball.
#
#   ./install.sh                  link ~/.local/bin/gensokyo -> bin/gensokyo; fetch tmux + jq if needed
#   ./install.sh --bin-dir DIR    put the link in DIR instead (also: GENSOKYO_BIN_DIR)
#   ./install.sh --no-fetch       never download: fail unless vendored or acceptable system binaries exist
#
# POSIX sh on purpose (a release will pipe this to `sh`). Nothing is written outside the
# gensokyo directory except the one symlink. ~/.tmux.conf and ~/.claude are never touched.
# tmux >= 3.3 and jq >= 1.6 are needed; if vendor/<os>-<arch>/ has them they are used, else an
# acceptable system pair is accepted, else scripts/vendor.sh downloads and verifies them.

set -eu

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

root=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || root=
[ -n "$root" ] && [ -f "$root/bin/gensokyo" ] ||
  die "run this from an unpacked gensokyo directory (./install.sh); 'curl | sh' installs are not available yet"

bin_dir=${GENSOKYO_BIN_DIR:-$HOME/.local/bin}
fetch=1
while [ $# -gt 0 ]; do
  case $1 in
    --bin-dir) [ $# -ge 2 ] || die "--bin-dir needs a directory"; bin_dir=$2; shift ;;
    --bin-dir=*) bin_dir=${1#--bin-dir=} ;;
    --no-fetch) fetch=0 ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------- platform
os=$(uname -s) arch=$(uname -m)
case $os in Darwin) os=macos ;; Linux) os=linux ;; *) die "unsupported OS: $os (macOS and Linux only)" ;; esac
case $arch in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x86_64 ;; *) die "unsupported architecture: $arch (arm64 and x86_64 only)" ;; esac
platform="$os-$arch"

command -v bash >/dev/null 2>&1 || die "bash is required (bin/gensokyo runs under bash 3.2 or newer)"

# ver_ge <a> <b>: dotted version a >= b, letters ignored (3.7c >= 3.3).
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
