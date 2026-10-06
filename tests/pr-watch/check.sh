#!/bin/sh
# The page as the viewer wraps it, with the stand-in db in front: tests/pr-watch/check.sh <dir>
# writes <dir>/index.html. Serve <dir> (python3 -m http.server) and open it with #full,
# #failing, #failed, #stale, #zero, #none or #nodb; shim("stale") in the console is a live write.
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?a directory to write into}
mkdir -p "$out"
{ cat "$here/shim.html"; cat "$here/../../share/plugin/skills/gensokyo-ritual/pr-watch.html"; printf '</body></html>\n'; } > "$out/index.html"
echo "$out/index.html"
