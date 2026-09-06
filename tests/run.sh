#!/bin/bash
# tests/run.sh - gensokyo's test runner. Pure bash 3.2, no bats.
#
#   tests/run.sh            run everything
#   tests/run.sh unit       unit tests only (no tmux needed)
#   tests/run.sh smoke      the headless smoke test (vendored or system tmux + jq, stub claude)
#   tests/run.sh -v ...     print each test name as it runs
#
# This file is the harness: the scratch state dir every test runs against, the assertions and
# the helpers that read a shrine frame or a pane. The tests themselves live one group per file
# in tests/cases/, sourced at the bottom. Unit tests call gensokyo's functions directly; the
# smoke test starts a real tmux server on its own socket with tests/stub-claude standing in
# for claude, summons residents and reads the bar and list.

# shellcheck source-path=SCRIPTDIR   # `shellcheck -x` then follows the tests/cases/*.sh sources
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
verbose=''
what=all
for a in "$@"; do case $a in -v) verbose=1 ;; unit|smoke|all) what=$a ;; *) echo "usage: tests/run.sh [-v] [unit|smoke|all]" >&2; exit 2 ;; esac; done

scratch=${TMPDIR:-/tmp}/gensokyo-tests.$$
mkdir -p "$scratch"; scratch=$(cd "$scratch" && pwd)   # TMPDIR may end in a slash
export GENSOKYO_STATE_DIR=$scratch/state GENSOKYO_CONFIG_DIR=$scratch/config
export GENSOKYO_SOCKET=gtest$$ GENSOKYO_CLAUDE=$root/tests/stub-claude
export CLAUDE_CONFIG_DIR=$scratch/cc   # a stand-in ~/.claude: the status line tests need a known settings.json
export STUB_STATE=$scratch/stub
export HOME=${HOME:-/tmp}
unset TMUX TMUX_PANE
# A fake osascript / notify-send logs "subtitle|text" (its last two arguments) instead of
# showing a desktop notification.
mkdir -p "$scratch/fakebin"
printf '#!/bin/bash\nprintf "%%s|%%s\\n" "${@: -2:1}" "${@: -1}" >> "%s"\n' "$scratch/notify.log" > "$scratch/fakebin/osascript"
cp "$scratch/fakebin/osascript" "$scratch/fakebin/notify-send"; chmod +x "$scratch/fakebin"/*
# The stand-in user status line command: keeps the JSON it was given, prints one known line.
printf '#!/bin/bash\ncat > "%s/sl.in"; echo "USER LINE"\n' "$scratch" > "$scratch/fakebin/userline"; chmod +x "$scratch/fakebin/userline"
mkdir -p "$scratch/cc" "$scratch/cc-empty"
printf '{"statusLine": {"type": "command", "command": "%s/fakebin/userline", "padding": 1}, "advisorModel": "opus"}\n' "$scratch" > "$scratch/cc/settings.json"
export PATH=$scratch/fakebin:$PATH

pass=0 fail=0 skipped=0 current=''
ok()   { pass=$((pass + 1)); [ -n "$verbose" ] && printf 'ok    %s\n' "$current"; return 0; }
bad()  { fail=$((fail + 1)); printf 'FAIL  %s\n      %s\n' "$current" "$1"; return 0; }
skip() { skipped=$((skipped + 1)); printf 'skip  %s: %s\n' "$current" "$1"; return 0; }
t()    { current=$1; }
assert_eq()    { if [ "$1" = "$2" ]; then ok; else bad "expected: $2"$'\n'"      actual:   $1"; fi; }
assert_match() { case $1 in *"$2"*) ok ;; *) bad "expected to contain: $2"$'\n'"      actual: $1" ;; esac; }
assert_nomatch() { case $1 in *"$2"*) bad "expected NOT to contain: $2"$'\n'"      actual: $1" ;; *) ok ;; esac; }
assert_re()    { if printf '%s\n' "$1" | grep -E -q -- "$2"; then ok; else bad "expected to match /$2/"$'\n'"      actual: $1"; fi; }
assert_ok()    { if "$@" >/dev/null 2>&1; then ok; else bad "command failed: $*"; fi; }
assert_fails() { if "$@" >/dev/null 2>&1; then bad "command should fail: $*"; else ok; fi; }

pause() { perl -e "select(undef,undef,undef,$1)" 2>/dev/null || sleep 1; }
# wait_for <seconds> <test>: poll until the test passes. A summon or a close finishes in its
# own time, and a fixed pause long enough on an idle machine is not long enough on a busy one -
# which showed up as a resident taking the slot the closing one had not yet given back, and a
# dozen assertions failing after it. The test is evalled in the caller, so it reads the locals.
wait_for() {
  local limit=$1 n=0
  while [ "$n" -lt $((limit * 10)) ]; do
    eval "$2" >/dev/null 2>&1 && return 0
    pause 0.1; n=$((n + 1))
  done
  return 1
}

# The shrine frame is read the way a click reads it: shrine_map_find gives the map entry for an
# action (and argument), shrine_at cuts the columns that entry names out of the row it names.
shrine_map_find() { printf '%s' "$SHRINE_MAP" | awk -F'|' -v a="$1" -v g="${2:-}" '$4 == a && $5 == g { print; exit }'; }
# The widest row of the frame, in columns - which is what a pane is measured in, and neither what
# awk counts (the bytes of ⛩) nor what ${#line} counts (幻 想 郷 as one each).
shrine_widest() {
  local line n max=0
  while IFS= read -r line; do n=$(shrine_text_width "$line"); [ "$n" -gt "$max" ] && max=$n; done <<EOF
$SHRINE_TEXT
EOF
  printf '%s' "$max"
}
# shrine_event reads the terminal; give it one written down instead.
shrine_typed() { printf '%s' "$1" > "$scratch/typed"; shrine_event < "$scratch/typed"; }
shrine_at() {
  local row c1 c2 line
  [ -n "$1" ] || return 0
  IFS='|' read -r row c1 c2 _ _ <<EOF
$1
EOF
  line=$(printf '%s\n' "$SHRINE_TEXT" | sed -n "${row}p")
  printf '%s' "${line:$((c1 - 1)):$((c2 - c1 + 1))}"
}

# ---------------------------------------------------------------- the tests
# One file per group under tests/cases/, sourced rather than run: they share the harness above,
# this scratch state dir and this one copy of bin/gensokyo (it skips main when sourced).
# shellcheck source=../bin/gensokyo
# shellcheck disable=SC1091
. "$root/bin/gensokyo"
set_bins
ensure_dirs
rec() { printf '%s\n' "$@" | sed 's/^ *//' > "$RES_DIR/$1"; }   # rec <id> key=v ...
fresh() { rm -rf "$RES_DIR"; mkdir -p "$RES_DIR"; }

# shellcheck source=cases/core.sh
. "$here/cases/core.sh"
# shellcheck source=cases/shrine.sh
. "$here/cases/shrine.sh"
# shellcheck source=cases/hooks.sh
. "$here/cases/hooks.sh"
# shellcheck source=cases/telemetry.sh
. "$here/cases/telemetry.sh"
# shellcheck source=cases/recall.sh
. "$here/cases/recall.sh"
# shellcheck source=cases/install.sh
. "$here/cases/install.sh"
# shellcheck source=cases/smoke.sh
. "$here/cases/smoke.sh"

case $what in
  unit) core_tests; shrine_tests; hook_tests; telemetry_tests; recall_tests; install_tests ;;
  smoke) smoke_tests ;;
  all) core_tests; shrine_tests; hook_tests; telemetry_tests; recall_tests; install_tests; smoke_tests ;;
esac
printf '\n%s passed, %s failed, %s skipped\n' "$pass" "$fail" "$skipped"
[ "$fail" -eq 0 ]
