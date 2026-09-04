#!/bin/sh
# scripts/vendor.sh - fetch and verify gensokyo's vendored binaries (tmux, jq).
#
# POSIX sh on purpose: install.sh is piped to `sh` and calls this.
#
#   scripts/vendor.sh                   fetch for this machine's platform (skips if present)
#   scripts/vendor.sh --all             fetch all four platforms (release builds)
#   scripts/vendor.sh macos-arm64 ...   fetch specific platforms
#   scripts/vendor.sh --force ...       re-download even if present
#   scripts/vendor.sh --status          show what is vendored and its versions
#   scripts/vendor.sh --check-upstream  report newer upstream releases than the pins
#   scripts/vendor.sh --print-sums      download everything and print a fresh SHA256SUMS (for bumps)
#
# Pins. Bump these, then regenerate vendor/SHA256SUMS with --print-sums.
TMUX_VERSION="3.7c"
JQ_VERSION="1.8.2"

TMUX_BASE="https://github.com/tmux/tmux-builds/releases/download/v${TMUX_VERSION}"
JQ_BASE="https://github.com/jqlang/jq/releases/download/jq-${JQ_VERSION}"
JQ_LICENSE_URL="https://raw.githubusercontent.com/jqlang/jq/jq-${JQ_VERSION}/COPYING"
PLATFORMS="macos-arm64 macos-x86_64 linux-arm64 linux-x86_64"

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
vendor="${GENSOKYO_VENDOR_DIR:-$root/vendor}"
sums="$root/vendor/SHA256SUMS"

die() { printf 'vendor.sh: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

detect_platform() {
  os=$(uname -s) arch=$(uname -m)
  case $os in
    Darwin) os=macos ;;
    Linux)  os=linux ;;
    *) die "unsupported OS: $os (gensokyo supports macOS and Linux)" ;;
  esac
  case $arch in
    arm64|aarch64) arch=arm64 ;;
    x86_64|amd64)  arch=x86_64 ;;
    *) die "unsupported architecture: $arch (arm64 and x86_64 only)" ;;
  esac
  printf '%s-%s\n' "$os" "$arch"
}

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$1" | sed 's/.* //'
  else
    die "need shasum, sha256sum or openssl to verify downloads"
  fi
}

expected_sum() {
  [ -f "$sums" ] || die "missing $sums"
  awk -v f="$1" '$2 == f { print $1 }' "$sums"
}

# fetch <asset-name> <url> <dest>: download, verify against SHA256SUMS, move into place.
fetch() {
  want=$(expected_sum "$1")
  [ -n "$want" ] || die "no pinned checksum for '$1' in vendor/SHA256SUMS"
  tmp="$3.part"
  curl -fsSL --retry 3 -o "$tmp" "$2" || die "download failed: $2"
  got=$(sha256_of "$tmp")
  if [ "$got" != "$want" ]; then
    rm -f "$tmp"
    die "checksum mismatch for $1
  expected $want
  got      $got
Refusing to install. Upstream may have re-published the asset; do not bypass this."
  fi
  mv "$tmp" "$3"
}

jq_asset() { # <platform> -> upstream jq asset name (jq says amd64, we say x86_64)
  case $1 in
    *-x86_64) printf 'jq-%s-amd64\n' "${1%-*}" ;;
    *)        printf 'jq-%s\n' "$1" ;;
  esac
}

install_platform() {
  plat=$1
  case " $PLATFORMS " in *" $plat "*) ;; *) die "unknown platform '$plat' (one of: $PLATFORMS)" ;; esac
  dest="$vendor/$plat"
  if [ -x "$dest/tmux" ] && [ -x "$dest/jq" ] && [ "$force" != 1 ]; then
    say "vendor/$plat: already present (use --force to re-download)"
    return 0
  fi
  # Stage in $work; only touch $dest once every file has verified.
  stage="$work/$plat"; mkdir -p "$stage"
  tarball="tmux-${TMUX_VERSION}-${plat}.tar.gz"
  fetch "$tarball" "$TMUX_BASE/$tarball" "$work/$tarball"
  tar -xzf "$work/$tarball" -C "$stage" tmux
  jqname=$(jq_asset "$plat")
  fetch "$jqname" "$JQ_BASE/$jqname" "$stage/jq"
  chmod 755 "$stage/tmux" "$stage/jq"
  mkdir -p "$dest"
  mv -f "$stage/tmux" "$dest/tmux"
  mv -f "$stage/jq" "$dest/jq"
  if [ "${plat%-*}" = macos ] && command -v xattr >/dev/null 2>&1; then
    xattr -d com.apple.quarantine "$dest/tmux" "$dest/jq" 2>/dev/null || :
  fi
  say "vendor/$plat: tmux $TMUX_VERSION, jq $JQ_VERSION"
}

install_licenses() {
  dest="$vendor/LICENSES"
  if [ -f "$dest/COPYING.tmux" ] && [ -f "$dest/COPYING.jq" ] && [ "$force" != 1 ]; then
    return 0
  fi
  mkdir -p "$dest"
  fetch "LICENSES.tar.gz" "$TMUX_BASE/LICENSES.tar.gz" "$work/LICENSES.tar.gz"
  tar -xzf "$work/LICENSES.tar.gz" -C "$dest"
  fetch "jq-COPYING" "$JQ_LICENSE_URL" "$dest/COPYING.jq"
  say "vendor/LICENSES: tmux (ISC + bundled libs), jq (MIT)"
}

status() {
  for plat in $PLATFORMS; do
    d="$vendor/$plat"
    if [ -x "$d/tmux" ] && [ -x "$d/jq" ]; then
      if [ "$plat" = "$(detect_platform)" ]; then
        say "$plat: $("$d/tmux" -V) / jq $("$d/jq" --version) (runs here)"
      else
        say "$plat: present (foreign platform, not run)"
      fi
    else
      say "$plat: missing"
    fi
  done
  if [ -d "$vendor/LICENSES" ]; then say "LICENSES: present"; else say "LICENSES: missing"; fi
}

latest_tag() { # <owner/repo>
  curl -fsSL "https://api.github.com/repos/$1/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

check_upstream() {
  t=$(latest_tag tmux/tmux-builds); j=$(latest_tag jqlang/jq)
  say "tmux: pinned v$TMUX_VERSION, latest $t"
  say "jq:   pinned jq-$JQ_VERSION, latest $j"
  if [ "$t" = "v$TMUX_VERSION" ] && [ "$j" = "jq-$JQ_VERSION" ]; then
    say "pins are current"
  else
    say "newer releases exist; bump deliberately (see vendor/README.md)"
  fi
}

# --print-sums: download every asset without verifying and print a SHA256SUMS block.
print_sums() {
  for plat in $PLATFORMS; do
    a="tmux-${TMUX_VERSION}-${plat}.tar.gz"
    curl -fsSL -o "$work/$a" "$TMUX_BASE/$a" && printf '%s  %s\n' "$(sha256_of "$work/$a")" "$a"
  done
  curl -fsSL -o "$work/LICENSES.tar.gz" "$TMUX_BASE/LICENSES.tar.gz" && printf '%s  %s\n' "$(sha256_of "$work/LICENSES.tar.gz")" "LICENSES.tar.gz"
  for plat in $PLATFORMS; do
    a=$(jq_asset "$plat")
    curl -fsSL -o "$work/$a" "$JQ_BASE/$a" && printf '%s  %s\n' "$(sha256_of "$work/$a")" "$a"
  done
  curl -fsSL -o "$work/jq-COPYING" "$JQ_LICENSE_URL" && printf '%s  %s\n' "$(sha256_of "$work/jq-COPYING")" "jq-COPYING"
}

force=0 mode=install targets=""
for arg in "$@"; do
  case $arg in
    --all)            targets="$PLATFORMS" ;;
    --force)          force=1 ;;
    --status)         mode=status ;;
    --check-upstream) mode=upstream ;;
    --print-sums)     mode=sums ;;
    -h|--help)        sed -n '2,13p' "$0"; exit 0 ;;
    -*)               die "unknown option $arg" ;;
    *)                targets="$targets $arg" ;;
  esac
done

case $mode in
  status)   status; exit 0 ;;
  upstream) check_upstream; exit 0 ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is required to fetch vendored binaries"
work=$(mktemp -d "${TMPDIR:-/tmp}/gensokyo-vendor.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

case $mode in
  sums) print_sums; exit 0 ;;
esac

[ -n "$targets" ] || targets=$(detect_platform)
for plat in $targets; do install_platform "$plat"; done
install_licenses
