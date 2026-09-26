#!/bin/bash
# Fetch the Ghostty source that libghostty-vt is built from into vendor/ghostty-<commit>, and
# check it.
#
# libghostty-vt 0.2.1's bindings match Ghostty's C API only between the Zig 0.16 port and
# 03d5fa268 (which changed ghostty_terminal_new's signature). A mismatch links fine and fails at
# runtime, so the source is pinned by hand here instead of letting the crate clone its own pin.
# .cargo/config.toml points GHOSTTY_SOURCE_DIR at the result.
#
# The commit is in the directory name because libghostty-vt-sys rebuilds Ghostty only when the
# value of GHOSTTY_SOURCE_DIR changes, not when the files under it do. A new pin is a new path,
# so it can never link a stale build.
#
# Idempotent: exits 0 at once when the directory is already at the pin and clean.
set -eu

COMMIT=739603b8a2b643b167031a99718127cc0ca311a5
TREE=7cff9e6286f9ee88067479adf559e7f6dfa0f0ed
REPO=https://github.com/ghostty-org/ghostty.git

root=$(cd "$(dirname "$0")/.." && pwd)
name=ghostty-${COMMIT:0:9}
dir=$root/vendor/$name

grep -q "\"vendor/$name\"" "$root/.cargo/config.toml" ||
  { echo "ghostty.sh: .cargo/config.toml does not point GHOSTTY_SOURCE_DIR at vendor/$name" >&2; exit 1; }

check() {
  [ "$(git -C "$dir" rev-parse HEAD)" = "$COMMIT" ] || { echo "ghostty.sh: $dir is not at $COMMIT" >&2; return 1; }
  [ "$(git -C "$dir" rev-parse 'HEAD^{tree}')" = "$TREE" ] || { echo "ghostty.sh: tree of $COMMIT is not $TREE" >&2; return 1; }
  git -C "$dir" diff --quiet HEAD || { echo "ghostty.sh: $dir has local changes" >&2; return 1; }
}

if [ -d "$dir/.git" ]; then
  if check 2>/dev/null; then echo "ghostty.sh: vendor/$name ok"; exit 0; fi
  echo "ghostty.sh: vendor/$name is not at the pin or is dirty; remove it and rerun" >&2
  check
  exit 1
fi

mkdir -p "$root/vendor"
tmp=$root/vendor/.ghostty.part
rm -rf "$tmp"
git init -q "$tmp"
git -C "$tmp" remote add origin "$REPO"
git -C "$tmp" fetch -q --depth 1 origin "$COMMIT"
git -C "$tmp" -c advice.detachedHead=false checkout -q FETCH_HEAD
mv "$tmp" "$dir"
check
echo "ghostty.sh: fetched vendor/$name, tree ${TREE:0:9} ok"
