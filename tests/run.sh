#!/bin/bash
# tests/run.sh - gensokyo's test runner. Pure bash 3.2, no bats.
#
#   tests/run.sh            run everything
#   tests/run.sh unit       unit tests only (no tmux needed)
#   tests/run.sh smoke      the headless smoke test (vendored or system tmux + jq, stub claude)
#   tests/run.sh -v ...     print each test name as it runs
#
# Unit tests source bin/gensokyo (it skips main when sourced) and call its functions against
# a scratch state dir. The smoke test starts a real tmux server on its own socket with
# tests/stub-claude standing in for claude, summons residents and reads the bar and list.

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
export STUB_STATE=$scratch/stub
export HOME=${HOME:-/tmp}
unset TMUX TMUX_PANE

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

# ---------------------------------------------------------------- unit
# shellcheck source=../bin/gensokyo
# shellcheck disable=SC1091
. "$root/bin/gensokyo"
set_bins
ensure_dirs
rec() { printf '%s\n' "$@" | sed 's/^ *//' > "$RES_DIR/$1"; }   # rec <id> key=v ...
fresh() { rm -rf "$RES_DIR"; mkdir -p "$RES_DIR"; }

unit_tests() {
  local out f

  t "ver_ge compares dotted versions, letters ignored"
  assert_ok ver_ge 3.7c 3.3
  assert_ok ver_ge 2.1.260 2.1.224
  assert_fails ver_ge 1.6 1.8
  assert_fails ver_ge 2.0.9 2.1

  t "tilde shortens HOME and truncates from the left"
  assert_eq "$(tilde "$HOME/dev/x")" "~/dev/x"
  assert_eq "$(tilde "$HOME")" "~"
  assert_eq "$(tilde /opt/other)" '/opt/other'
  assert_eq "$(tilde "$HOME/a/very/long/path/name" 10)" "…path/name"

  t "lower"
  assert_eq "$(lower MaRiSa)" marisa

  t "new_uuid is a lowercase v4-shaped uuid"
  assert_re "$(new_uuid)" '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
  assert_ok test "$(new_uuid)" != "$(new_uuid)"

  t "stage_of_slot: four residents per stage"
  assert_eq "$(stage_of_slot 1) $(stage_of_slot 4) $(stage_of_slot 5) $(stage_of_slot 9)" 'stage1 stage1 stage2 stage3'

  t "glyph_for"
  assert_eq "$(glyph_for busy)$(glyph_for waiting)$(glyph_for idle)$(glyph_for departed)$(glyph_for starting)" '●✦○·○'

  t "records: set, get, del, load"
  fresh; rec r1 slot=1 name=Reimu cwd=/tmp/a
  rec_set "$RES_DIR/r1" pane %3; rec_set "$RES_DIR/r1" name Marisa
  assert_eq "$(rec_get "$RES_DIR/r1" name)" Marisa
  assert_eq "$(rec_get "$RES_DIR/r1" pane)" %3
  rec_del "$RES_DIR/r1" pane
  assert_eq "$(rec_get "$RES_DIR/r1" pane)" ''
  rec_load "$RES_DIR/r1"
  assert_eq "$R_slot|$R_name|$R_cwd|$R_pane" '1|Marisa|/tmp/a|'

  t "next_slot is the smallest free number"
  fresh; rec a slot=1; rec b slot=2; rec c slot=4
  assert_eq "$(next_slot)" 3
  fresh; assert_eq "$(next_slot)" 1

  t "find_resident by name (any case), slot and session-id prefix"
  fresh; rec 11111111-aaaa-4000-8000-000000000001 slot=1 name=Reimu; rec 22222222-bbbb-4000-8000-000000000002 slot=2 name=Marisa
  assert_eq "$(find_resident marisa)" "$RES_DIR/22222222-bbbb-4000-8000-000000000002"
  assert_eq "$(find_resident 1)" "$RES_DIR/11111111-aaaa-4000-8000-000000000001"
  assert_eq "$(find_resident 2222)" "$RES_DIR/22222222-bbbb-4000-8000-000000000002"
  assert_fails find_resident Nobody

  t "pick_name skips names in use (case-insensitive) and falls back to ResidentNNN"
  fresh
  printf 'Reimu\nmarisa\n# comment\n\nSakuya\n' > "$CONFIG_DIR/names.txt"
  rec a slot=1 name=reimu; rec b slot=2 name=Marisa
  assert_eq "$(pick_name)" Sakuya
  rec c slot=3 name=SAKUYA
  assert_re "$(pick_name)" '^Resident[0-9]{3}$'
  rm -f "$CONFIG_DIR/names.txt"
  fresh; assert_re "$(pick_name)" '^[A-Z][A-Za-z]+$'

  t "load_config: KEY=value lines, comments, bad keys ignored with a warning"
  printf '# gensokyo\nPREFIX=C-a\nSTAGE_SIZE=6\nbad key=1\n\n' > "$CONFIG_DIR/config"
  out=$(load_config 2>&1; printf '%s|%s' "$CFG_PREFIX" "$CFG_STAGE_SIZE")
  assert_match "$out" 'ignoring line: bad key=1'
  assert_match "$out" 'C-a|6'
  rm -f "$CONFIG_DIR/config"
  CFG_PREFIX=C-Space; CFG_STAGE_SIZE=4
  assert_eq "$(prefix_label)" Ctrl-Space
  CFG_PREFIX=C-a; assert_eq "$(prefix_label)" Ctrl-a; CFG_PREFIX=C-Space

  if [ -z "$JQ_BIN" ]; then t "jq-based tests"; skip "no jq (run scripts/vendor.sh)"; return; fi

  t "registry_filter: one id|status|name|cwd|pid line per session, null status and missing name handled"
  cp "$here/fixtures/agents.json" "$REGISTRY"
  assert_eq "$(registry_filter)" 'ed82e343-81ce-4b9e-8fdb-9b32d8136a5c|idle|Marisa|/Users/me/dev/alpha|74525
f2fe56c9-466e-4333-bed0-4a89460dd0b8|waiting|Sakuya|/Users/me/dev/beta|85270
0b1c2d3e-0000-4000-8000-000000000003|null||/Users/me/dev/gamma|90001'

  t "registry_row and outsider_count"
  REG=$'\n'$(registry_filter)
  assert_eq "$(registry_row f2fe56c9-466e-4333-bed0-4a89460dd0b8)" 'waiting|Sakuya|/Users/me/dev/beta|85270'
  assert_eq "$(registry_row nope)" ''
  fresh; rec ed82e343-81ce-4b9e-8fdb-9b32d8136a5c slot=1 name=Marisa cwd=/Users/me/dev/alpha pane=%1 window=stage1
  assert_eq "$(outsider_count)" 2

  t "resident_rows: registry state and name win over the record; departed wins over both"
  rec f2fe56c9-466e-4333-bed0-4a89460dd0b8 slot=2 name=OldName cwd=/Users/me/dev/beta pane=%2 window=stage1
  rec 33333333-cccc-4000-8000-000000000003 slot=3 name=Youmu cwd=/tmp pane=%3 window=stage1 departed=1
  rec 44444444-dddd-4000-8000-000000000004 slot=4 name=Cirno cwd=/tmp window=stage1
  assert_eq "$(resident_rows)" '1|ed82e343-81ce-4b9e-8fdb-9b32d8136a5c|Marisa|idle|/Users/me/dev/alpha|%1|stage1
2|f2fe56c9-466e-4333-bed0-4a89460dd0b8|Sakuya|waiting|/Users/me/dev/beta|%2|stage1
3|33333333-cccc-4000-8000-000000000003|Youmu|departed|/tmp|%3|stage1
4|44444444-dddd-4000-8000-000000000004|Cirno|starting|/tmp|-|stage1'
  assert_eq "$(rec_get "$RES_DIR/f2fe56c9-466e-4333-bed0-4a89460dd0b8" name)" Sakuya

  t "help --json lists every public command with plain name first and its alias"
  out=$("$root/bin/gensokyo" help --json)
  assert_eq "$(printf '%s' "$out" | jq_ -r '.commands[] | select(.name=="new") | .alias')" summon
  assert_eq "$(printf '%s' "$out" | jq_ -r '.commands[] | select(.name=="close") | .alias')" banish
  assert_eq "$(printf '%s' "$out" | jq_ -r '.commands[] | select(.name=="list") | .alias')" who
  assert_eq "$(printf '%s' "$out" | jq_ -r '[.commands[].name] | index("focus") != null')" true
  assert_eq "$(printf '%s' "$out" | jq_ -r '.prefix')" C-Space
  for f in new close list focus stage help doctor; do
    assert_match "$("$root/bin/gensokyo" help)" "gensokyo $f"
  done

  t "every command in help --json is accepted by the dispatcher (no 'unknown command')"
  for f in $(printf '%s' "$out" | jq_ -r '.commands[] | .name, .alias | select(. != "" and . != "gensokyo")'); do
    case $("$root/bin/gensokyo" "$f" /nonexistent-dir-for-this-test 2>&1) in *"unknown command"*) bad "$f is listed but unknown" ;; *) ok ;; esac
  done

  t "the handbook skill and plugin manifest exist and the skill names only real commands"
  assert_ok test -f "$root/share/plugin/.claude-plugin/plugin.json"
  assert_ok jq_ -e '.name == "gensokyo"' "$root/share/plugin/.claude-plugin/plugin.json"
  assert_ok test -f "$root/share/plugin/skills/gensokyo/SKILL.md"
  assert_match "$(sed -n '2,/^---$/p' "$root/share/plugin/skills/gensokyo/SKILL.md")" 'description:'
  for f in $(grep -o 'gensokyo [a-z]*' "$root/share/plugin/skills/gensokyo/SKILL.md" | awk '{print $2}' | sort -u); do
    case $f in handbook|help|skill|is|CLI) continue ;; esac
    if printf '%s' "$out" | jq_ -e --arg c "$f" '[.commands[] | .name, .alias] | index($c) != null' >/dev/null; then ok
    elif grep -q "gensokyo $f\` is not available yet" "$root/share/plugin/skills/gensokyo/SKILL.md"; then ok
    else bad "SKILL.md mentions 'gensokyo $f', which help --json does not list"; fi
  done

  t "system_paragraph names the resident and the CLI"
  assert_match "$(system_paragraph Marisa)" 'resident name is Marisa'
  assert_match "$(system_paragraph Marisa)" 'GENSOKYO_BIN'

  t "real_path follows a chain of relative and absolute symlinks"
  mkdir -p "$scratch/rp/a" "$scratch/rp/b"; : > "$scratch/rp/b/target"
  ln -s ../b/target "$scratch/rp/a/rel"; ln -s "$scratch/rp/a/rel" "$scratch/rp/abs"
  assert_eq "$(real_path "$scratch/rp/abs")" "$scratch/rp/b/target"
  assert_eq "$(real_path "$scratch/rp/b/target")" "$scratch/rp/b/target"

  t "doctor reports the plugin and whether gensokyo is on PATH"
  out=$(PATH=/usr/bin:/bin cmd_doctor)
  assert_match "$out" "plugin     $root/share/plugin (handbook skill: gensokyo)"
  assert_match "$out" 'on PATH    no: run ./install.sh'
}

# install.sh: POSIX sh, run from the checkout into a scratch bin dir; --no-fetch keeps it offline.
install_tests() {
  local out bin=$scratch/ibin
  t "install.sh links bin/gensokyo into --bin-dir and the link resolves to this checkout"
  if [ ! -x "$root/vendor/$PLATFORM/tmux" ] && ! command -v tmux >/dev/null 2>&1; then skip "no tmux for install.sh --no-fetch"; return; fi
  out=$(cd "$scratch" && sh "$root/install.sh" --bin-dir "$bin" --no-fetch 2>&1)
  assert_match "$out" "linked $bin/gensokyo -> $root/bin/gensokyo"
  assert_match "$out" "tmux + jq: "
  assert_eq "$(readlink "$bin/gensokyo")" "$root/bin/gensokyo"
  assert_eq "$("$bin/gensokyo" version)" "gensokyo $VERSION"
  out=$(PATH=$bin:/usr/bin:/bin "$bin/gensokyo" doctor)
  assert_match "$out" "home       $root"
  assert_match "$out" "on PATH    $bin/gensokyo -> this copy"

  t "install.sh is idempotent, warns about PATH only when needed, and refuses to clobber a file"
  out=$(sh "$root/install.sh" --bin-dir "$bin" --no-fetch 2>&1)
  assert_match "$out" "linked $bin/gensokyo"
  assert_match "$out" "$bin is not on your PATH"
  out=$(PATH=$bin:$PATH sh "$root/install.sh" --bin-dir="$bin" --no-fetch 2>&1)
  assert_nomatch "$out" 'not on your PATH'
  mkdir -p "$scratch/ibin2"; : > "$scratch/ibin2/gensokyo"
  if out=$(sh "$root/install.sh" --bin-dir "$scratch/ibin2" --no-fetch 2>&1); then bad "should refuse to clobber a file"; else assert_match "$out" 'is not a symlink'; fi
  assert_fails sh "$root/install.sh" --bogus
  assert_match "$(sh "$root/install.sh" --help)" 'install.sh --no-fetch'
  cp "$root/install.sh" "$scratch/install.sh"
  assert_fails sh "$scratch/install.sh" --no-fetch   # not next to bin/gensokyo
}

# ---------------------------------------------------------------- smoke
# A real tmux server on its own socket, stub claude in the panes.
G=$root/bin/gensokyo
tm() { "$TMUX_BIN" -L "$GENSOKYO_SOCKET" "$@"; }
cleanup() { [ -n "${TMUX_BIN:-}" ] && tm kill-server 2>/dev/null; rm -rf "$scratch"; }
trap cleanup EXIT

smoke_tests() {
  local out pane1 pane2 id1 id2 args
  t "smoke: prerequisites"
  if [ -z "$TMUX_BIN" ] || [ -z "$JQ_BIN" ]; then skip "no tmux/jq (run scripts/vendor.sh)"; return; fi
  ok
  mkdir -p "$scratch/work/alpha" "$scratch/work/beta"
  fresh; rm -f "$REGISTRY"

  t "smoke: --detach starts the server with a shrine pane and an empty bar"
  out=$(cd "$scratch/work" && : > a && "$G" --detach 2>&1)   # a one-letter file: `?` must not glob
  assert_match "$out" 'running detached'
  assert_ok tm has-session -t =gensokyo
  assert_re "$(tm list-keys -T prefix)" '-T prefix +\? +display-popup .*_keys'
  assert_match "$("$G" _bar 1 %0)" 'no residents yet'
  assert_match "$("$G" list)" 'nobody is here yet'

  t "smoke: summon two residents; records, panes, launch flags"
  out=$("$G" new "$scratch/work/alpha" -n Alpha 2>&1)
  assert_match "$out" 'summoned Alpha (slot 1)'
  out=$("$G" new "$scratch/work/beta" -n Beta -m haiku -p plan 2>&1)
  assert_match "$out" 'summoned Beta (slot 2)'
  assert_fails "$G" new "$scratch/work/beta" -n alpha
  assert_fails "$G" new "$scratch/work/beta" -n 7
  pause 1.5
  id1=$(basename "$(find_resident Alpha)"); id2=$(basename "$(find_resident Beta)")
  pane1=$(rec_get "$RES_DIR/$id1" pane); pane2=$(rec_get "$RES_DIR/$id2" pane)
  assert_re "$pane1" '^%[0-9]+$'
  assert_eq "$(tm list-panes -t =gensokyo:stage1 -F '#{pane_id}' | wc -l | tr -d ' ')" 2
  args=$(cat "$STUB_STATE/$id2.args")
  assert_match "$args" "--session-id $id2 --name Beta"
  assert_match "$args" "--plugin-dir $root/share/plugin"
  assert_match "$args" '--append-system-prompt'
  assert_match "$args" '--model haiku --permission-mode plan'
  assert_nomatch "$(cat "$STUB_STATE/$id1.args")" '--model'
  assert_match "$(tm capture-pane -p -t "$pane1")" 'env CLAUDE*: 0'

  t "smoke: list and list --json see both, idle"
  rm -f "$REGISTRY"
  out=$("$G" list)
  assert_re "$out" '^  1   ○  Alpha .*alpha .*idle$'
  assert_re "$out" '^  2   ○  Beta .*beta .*idle$'
  out=$("$G" list --json)
  assert_eq "$(printf '%s' "$out" | jq_ -r 'map(.name) | join(",")')" 'Alpha,Beta'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[1] | "\(.slot) \(.status) \(.window) \(.outside)"')" '2 idle stage1 false'

  t "smoke: bar chips, bold for the active pane, waiting highlight and count"
  rm -f "$REGISTRY"
  out=$("$G" _bar 1 "$pane1")
  assert_match "$out" '#[bold] 1 ○ Alpha #[default]│#[default] 2 ○ Beta #[default]'
  echo waiting > "$STUB_STATE/$id2.status"; rm -f "$REGISTRY"
  out=$("$G" _bar 1 "$pane1")
  assert_match "$out" "#[bg=$CFG_COLOR_AWAIT,fg=black] 2 ✦ Beta "
  assert_match "$out" '#[align=right]#[bg='"$CFG_COLOR_AWAIT"',fg=black] ✦ 1 '
  rm -f "$STUB_STATE/$id2.status"
  assert_match "$("$G" _border "$pane1" '✳ Alpha')" ' 1 Alpha · alpha '

  t "smoke: focus by name and slot"
  assert_match "$("$G" focus beta)" 'focused Beta (slot 2)'
  assert_eq "$(tm display -p -t =gensokyo:stage1 '#{pane_id}')" "$pane2"
  assert_match "$("$G" focus 1)" 'focused Alpha'
  assert_eq "$(tm display -p -t =gensokyo:stage1 '#{pane_id}')" "$pane1"
  assert_fails "$G" focus 7

  t "smoke: /rename inside a resident reaches list and close"
  tm send-keys -t "$pane2" -l '/rename Gamma' \; send-keys -t "$pane2" Enter
  pause 0.5; rm -f "$REGISTRY"
  assert_match "$("$G" list)" 'Gamma'
  assert_eq "$(rec_get "$RES_DIR/$id2" name)" Gamma

  t "smoke: close asks for /exit; the pane shows the departed screen; the chip dims"
  out=$("$G" close gamma 2>&1)
  assert_match "$out" 'Gamma'
  pause 1.2
  assert_match "$(tm capture-pane -p -t "$pane2")" 'Gamma has left the shrine'
  assert_ok test -n "$(rec_get "$RES_DIR/$id2" departed)"
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane1")" '#[dim] 2 · Gamma '
  assert_match "$("$G" _border "$pane2" 'x')" 'departed'
  assert_re "$("$G" list)" '^  2   ·  Gamma .*departed$'

  t "smoke: closing the departed pane frees the slot; the next summon reuses it"
  "$G" close 2 >/dev/null 2>&1; pause 0.5
  assert_ok test ! -f "$RES_DIR/$id2"
  assert_eq "$(tm list-panes -t =gensokyo:stage1 -F '#{pane_id}' | wc -l | tr -d ' ')" 1
  assert_match "$("$G" new "$scratch/work/beta")" '(slot 2)'
  pause 0.5

  t "smoke: five residents overflow into stage2; stage applies a layout"
  "$G" new "$scratch/work/alpha" >/dev/null; "$G" new "$scratch/work/alpha" >/dev/null; "$G" new "$scratch/work/alpha" >/dev/null
  pause 0.5
  assert_eq "$(tm list-panes -t =gensokyo:stage1 -F x | wc -l | tr -d ' ')" 4
  assert_eq "$(tm list-panes -t =gensokyo:stage2 -F x | wc -l | tr -d ' ')" 1
  assert_ok "$G" stage main-vertical
  assert_eq "$(tm show -w -v -t =gensokyo:stage1 @layout)" main-vertical

  t "smoke: a pane killed behind gensokyo's back is pruned from the bar"
  tm kill-pane -t "$pane1"; pause 0.3
  "$G" _bar 1 %99 >/dev/null
  assert_ok test ! -f "$RES_DIR/$id1"

  t "smoke: the last resident leaving brings the shrine back and stale records are archived on restart"
  tm kill-server; pause 0.3
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 4
  "$G" --detach >/dev/null
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_eq "$(ls "$STATE_DIR/departed" | wc -l | tr -d ' ')" 4
  tm kill-server
}

case $what in
  unit) unit_tests; install_tests ;;
  smoke) smoke_tests ;;
  all) unit_tests; install_tests; smoke_tests ;;
esac
printf '\n%s passed, %s failed, %s skipped\n' "$pass" "$fail" "$skipped"
[ "$fail" -eq 0 ]
