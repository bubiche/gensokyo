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

  t "load_config: KEY=value lines, comments, bad keys ignored with a warning, STAGE_SIZE sanity"
  printf '# gensokyo\nPREFIX=C-a\nSTAGE_SIZE=6\nbad key=1\n\n' > "$CONFIG_DIR/config"
  out=$(load_config 2>&1; printf '%s|%s' "$CFG_PREFIX" "$CFG_STAGE_SIZE")
  assert_match "$out" 'ignoring line: bad key=1'
  assert_match "$out" 'C-a|6'
  printf 'STAGE_SIZE=0\n' > "$CONFIG_DIR/config"
  out=$(load_config 2>&1; printf '%s' "$CFG_STAGE_SIZE")
  assert_match "$out" 'STAGE_SIZE must be a positive number'
  assert_re "$out" '4$'
  rm -f "$CONFIG_DIR/config"
  CFG_PREFIX=C-Space; CFG_STAGE_SIZE=4

  t "scrub_env drops CLAUDE* session markers but keeps CLAUDE_CONFIG_DIR"
  out=$(CLAUDECODE=1 CLAUDE_CODE_CHILD_SESSION=x CLAUDE_CONFIG_DIR=/tmp/cc bash -c '. "$1"; scrub_env; env | grep "^CLAUDE" | sort | tr "\n" " "' _ "$root/bin/gensokyo")
  assert_eq "$out" 'CLAUDE_CONFIG_DIR=/tmp/cc '
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
  assert_eq "$(resident_rows)" '1|ed82e343-81ce-4b9e-8fdb-9b32d8136a5c|Marisa|idle|/Users/me/dev/alpha|%1|stage1|||||||||||||||
2|f2fe56c9-466e-4333-bed0-4a89460dd0b8|Sakuya|waiting|/Users/me/dev/beta|%2|stage1|||||||||||||||
3|33333333-cccc-4000-8000-000000000003|Youmu|departed|/tmp|%3|stage1|||||||||||||||
4|44444444-dddd-4000-8000-000000000004|Cirno|starting|/tmp|-|stage1|||||||||||||||'
  assert_eq "$(rec_get "$RES_DIR/f2fe56c9-466e-4333-bed0-4a89460dd0b8" name)" Sakuya

  t "resident_rows: a pending question or finished turn from the hooks shows as needing you"
  fresh; rm -rf "$STATE_DIR/status"
  rec ed82e343-81ce-4b9e-8fdb-9b32d8136a5c slot=1 name=Marisa cwd=/a pane=%1 window=stage1 mode=plan   # idle in the registry
  rec f2fe56c9-466e-4333-bed0-4a89460dd0b8 slot=2 name=Sakuya cwd=/b pane=%2 window=stage1            # waiting
  rec 0b1c2d3e-0000-4000-8000-000000000003 slot=3 name=Youmu cwd=/c pane=%3 window=stage1             # null -> idle
  status_write ed82e343-81ce-4b9e-8fdb-9b32d8136a5c stopped 'all tests pass'
  status_write f2fe56c9-466e-4333-bed0-4a89460dd0b8 question 'Delete the branch?'
  status_write 0b1c2d3e-0000-4000-8000-000000000003 '' '' acceptEdits
  assert_eq "$(resident_rows | cut -d'|' -f1,3,4,8,9)" '1|Marisa|waiting|plan|all tests pass
2|Sakuya|question||Delete the branch?
3|Youmu|idle|acceptEdits|'
  status_write ed82e343-81ce-4b9e-8fdb-9b32d8136a5c '' '' default   # the hook mode beats the launch mode
  assert_eq "$(resident_rows | head -n 1 | cut -d'|' -f4,8)" 'idle|default'

  t "resident_rows: busy clears a stale awaits/stopped flag, but not one newer than the registry snapshot"
  fresh; rm -rf "$STATE_DIR/status"
  printf '[{"pid": 1, "cwd": "/a", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000001", "name": "Reimu", "status": "busy"}]\n' > "$REGISTRY"
  REG=$'\n'$(registry_filter)
  rec aaaaaaaa-0000-4000-8000-000000000001 slot=1 name=Reimu cwd=/a pane=%1 window=stage1
  status_write aaaaaaaa-0000-4000-8000-000000000001 stopped 'done'      # since = now > registry mtime? no: same second
  printf 'pending=stopped\ndetail=done\nsince=%s\nmode=\n' "$(( $(date +%s) - 30 ))" > "$STATE_DIR/status/aaaaaaaa-0000-4000-8000-000000000001"
  assert_eq "$(resident_rows | cut -d'|' -f4)" busy
  assert_eq "$(rec_get "$STATE_DIR/status/aaaaaaaa-0000-4000-8000-000000000001" pending)" ''   # cleared: older than the snapshot
  printf 'pending=stopped\ndetail=done\nsince=%s\nmode=\n' "$(( $(date +%s) + 30 ))" > "$STATE_DIR/status/aaaaaaaa-0000-4000-8000-000000000001"
  assert_eq "$(resident_rows | cut -d'|' -f4)" busy
  assert_eq "$(rec_get "$STATE_DIR/status/aaaaaaaa-0000-4000-8000-000000000001" pending)" stopped   # kept: a Stop that raced the snapshot
  cp "$here/fixtures/agents.json" "$REGISTRY"; REG=$'\n'$(registry_filter)

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

  t "doctor reports iTerm2 and its profile from the environment, without launching anything"
  out=$(TERM_PROGRAM=iTerm.app TERM_PROGRAM_VERSION=3.6.11 cmd_doctor)
  assert_match "$out" 'iTerm2     3.6.11  this shell came from iTerm2'
  assert_nomatch "$out" 'too old'
  out=$(TERM_PROGRAM=iTerm.app TERM_PROGRAM_VERSION=3.4.9 cmd_doctor)
  assert_match "$out" 'too old: need >= 3.5'
  out=$(TERM_PROGRAM=Apple_Terminal cmd_doctor)
  assert_match "$out" 'iTerm2     no (TERM_PROGRAM=Apple_Terminal)'
  assert_match "$out" 'profile    '

  t "home_mode: iTerm2 gets the native panes, --tty and --nested the plain tmux client"
  assert_eq "$(TERM_PROGRAM=iTerm.app home_mode '' '')" cc
  assert_eq "$(TERM_PROGRAM=iTerm.app home_mode 1 '')" tty
  assert_eq "$(TERM_PROGRAM=iTerm.app home_mode '' 1)" tty
  assert_eq "$(TERM_PROGRAM=Apple_Terminal home_mode '' '')" tty
  assert_eq "$(TERM_PROGRAM='' home_mode '' '')" tty

  t "the attach options are --tty, --detach and --nested; anything else is refused"
  assert_match "$("$root/bin/gensokyo" help)" '--tty: plain tmux client'
  assert_fails "$root/bin/gensokyo" --cc
  assert_match "$("$root/bin/gensokyo" --cc 2>&1)" 'unknown option: --cc'
}

# The hooks Claude Code runs inside a resident: payloads piped to `gensokyo _hook`, no tmux
# server (the toast and bell need one; the fake osascript logs the desktop alerts).
payload() { printf '{"session_id":"%s","hook_event_name":"%s"%s}\n' "$1" "$2" "${3:-}"; }
hook() { payload "$@" | "$root/bin/gensokyo" _hook; }
notifications() { cat "$scratch/notify.log" 2>/dev/null; : > "$scratch/notify.log"; }
hook_tests() {
  local id=cafecafe-0000-4000-8000-000000000001 st=$STATE_DIR/status/cafecafe-0000-4000-8000-000000000001 out
  if [ -z "$JQ_BIN" ]; then t "hook tests"; skip "no jq"; return; fi

  t "status_write/status_load: since moves only when pending changes; mode is kept when not given"
  fresh; rm -rf "$STATE_DIR/status"
  status_write "$id" awaits 'permission: Bash' plan
  status_load "$id"; out=$S_since
  assert_eq "$S_pending|$S_detail|$S_mode" 'awaits|permission: Bash|plan'
  assert_re "$out" '^[0-9]+$'
  status_write "$id" awaits 'permission: Edit'
  status_load "$id"
  assert_eq "$S_since|$S_mode|$S_detail" "$out|plan|permission: Edit"
  status_write "$id" '' '' ; status_load "$id"
  assert_eq "$S_pending|$S_mode" '|plan'

  t "launch_settings: the hooks Claude Code merges into the resident, all calling _hook; the status line wrapper with the user's padding"
  out=$(launch_settings "$id" /tmp)
  assert_eq "$(printf '%s' "$out" | jq_ -r '.hooks | keys | join(",")')" 'Notification,PostToolUse,PreToolUse,Stop,UserPromptSubmit'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.hooks.PreToolUse[0].matcher, .hooks.PostToolUse[0].matcher')" 'AskUserQuestion
AskUserQuestion'
  assert_eq "$(printf '%s' "$out" | jq_ -r '[.hooks[][].hooks[].command] | unique | .[]')" "'$SELF' _hook"
  assert_eq "$(printf '%s' "$out" | jq_ -r '.statusLine | "\(.type) \(.command) \(.padding)"')" "command '$SELF' _statusline '$id' 1"
  out=$(CLAUDE_DIR=$scratch/cc-empty launch_settings "$id" /tmp)
  assert_eq "$(printf '%s' "$out" | jq_ -c '.statusLine | keys')" '["command","type"]'

  t "_hook: Stop marks the turn as awaiting you and notifies once; UserPromptSubmit clears and records the mode"
  fresh; rm -rf "$STATE_DIR/status"; : > "$scratch/notify.log"
  rec "$id" slot=1 name=Reimu cwd=/tmp pane=%9 window=stage1
  hook "$id" Stop ',"permission_mode":"default","last_assistant_message":"pong\nsecond line"'
  assert_eq "$(cut -d= -f1,2 "$st" | grep -v since | tr '\n' ' ')" 'pending=stopped detail=pong mode=default '
  assert_eq "$(notifications)" 'Reimu|Reimu is done: pong'
  hook "$id" Stop ',"permission_mode":"default","last_assistant_message":"pong"'
  assert_eq "$(notifications)" ''             # same waiting period: no second alert
  hook "$id" UserPromptSubmit ',"permission_mode":"plan","prompt":"go on"'
  assert_eq "$(cut -d= -f1,2 "$st" | grep -v since | tr '\n' ' ')" 'pending= detail= mode=plan '
  assert_eq "$(notifications)" ''

  t "_hook: permission prompt, question dialog open/close, idle_prompt as a late backup"
  hook "$id" Notification ',"notification_type":"permission_prompt","message":"Claude needs your permission","tool_name":"Bash"'
  assert_eq "$(rec_get "$st" pending)|$(rec_get "$st" detail)|$(rec_get "$st" mode)" 'awaits|permission: Bash|plan'
  assert_eq "$(notifications)" 'Reimu|Reimu needs your permission (Bash)'
  hook "$id" Notification ',"notification_type":"permission_prompt","message":"Claude needs your permission"'
  assert_eq "$(rec_get "$st" detail)" ''
  assert_eq "$(notifications)" ''
  hook "$id" PreToolUse ',"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Delete the branch | yes/no?\nreally","header":"Branch"}]}'
  assert_eq "$(rec_get "$st" pending)|$(rec_get "$st" detail)" 'question|Delete the branch   yes/no? really'
  assert_eq "$(notifications)" 'Reimu|Reimu asks: Delete the branch   yes/no? really'
  hook "$id" PostToolUse ',"tool_name":"AskUserQuestion","tool_response":{}'
  assert_eq "$(rec_get "$st" pending)" ''
  hook "$id" Notification ',"notification_type":"idle_prompt","message":"Claude is waiting for your input"'
  assert_eq "$(rec_get "$st" pending)" stopped
  assert_eq "$(notifications)" 'Reimu|Reimu is done'
  hook "$id" Notification ',"notification_type":"idle_prompt","message":"Claude is waiting for your input"'
  assert_eq "$(notifications)" ''
  hook "$id" PreToolUse ',"tool_name":"Bash","tool_input":{"command":"ls"}'
  assert_eq "$(rec_get "$st" pending)" stopped   # only AskUserQuestion matters

  t "_hook: unknown sessions and garbage are ignored, stdout stays empty, exit 0"
  out=$(hook dead-beef Stop ',"last_assistant_message":"x"' 2>&1; echo "rc=$?")
  assert_eq "$out" 'rc=0'
  assert_ok test ! -f "$STATE_DIR/status/dead-beef"
  out=$(printf 'not json' | "$root/bin/gensokyo" _hook 2>&1; echo "rc=$?")
  assert_eq "$out" 'rc=0'

  t "_hook: NOTIFY_DESKTOP=off silences the desktop alert; the state still changes"
  printf 'NOTIFY_DESKTOP=off\n' > "$CONFIG_DIR/config"
  hook "$id" UserPromptSubmit ',"permission_mode":"default"'
  hook "$id" Stop ',"permission_mode":"default","last_assistant_message":"quiet"'
  assert_eq "$(rec_get "$st" pending)" stopped
  assert_eq "$(notifications)" ''
  rm -f "$CONFIG_DIR/config"
  fresh; rm -rf "$STATE_DIR/status"
}

# The status line wrapper and what the bar, border and list make of its digest.
telemetry_tests() {
  local id=cafecafe-0000-4000-8000-000000000002 id2=cafecafe-0000-4000-8000-000000000003 out now
  if [ -z "$JQ_BIN" ]; then t "telemetry tests"; skip "no jq"; return; fi

  t "git_branch reads .git/HEAD walking up, follows worktree files, shows a detached head short; nothing outside a repo"
  mkdir -p "$scratch/repo/.git/worktrees/wt" "$scratch/repo/sub/dir" "$scratch/wt" "$scratch/norepo"
  echo 'ref: refs/heads/feature/x' > "$scratch/repo/.git/HEAD"
  assert_eq "$(git_branch "$scratch/repo/sub/dir")" feature/x
  echo 'abcdef1234567890' > "$scratch/repo/.git/HEAD"
  assert_eq "$(git_branch "$scratch/repo")" abcdef1
  printf 'gitdir: %s/repo/.git/worktrees/wt\n' "$scratch" > "$scratch/wt/.git"
  echo 'ref: refs/heads/wt-branch' > "$scratch/repo/.git/worktrees/wt/HEAD"
  assert_eq "$(git_branch "$scratch/wt")" wt-branch
  assert_eq "$(git_branch "$scratch/norepo")" ''

  t "formatting: pct_bar, fmt_eta, fmt_age, fmt_cost, mode_label, model_short, tele_fields leaves unknowns out"
  assert_eq "$(pct_bar 0)|$(pct_bar 36)|$(pct_bar 100)|$(pct_bar 250)" '░░░░░░░░░░|▓▓▓░░░░░░░|▓▓▓▓▓▓▓▓▓▓|▓▓▓▓▓▓▓▓▓▓'
  assert_eq "$(fmt_eta 7860) $(fmt_eta 275000) $(fmt_eta 840) $(fmt_eta 0)" '2h11m 3d4h 14m now'
  assert_eq "$(fmt_age 12) $(fmt_age 200) $(fmt_age 7200) $(fmt_age 90000)" '12s 3m 2h 1d'
  assert_eq "$(fmt_cost 0.18978) $(fmt_cost 12)" '$0.19 $12.00'
  assert_eq "$(mode_label acceptEdits)/$(mode_label bypassPermissions)/$(mode_label dontAsk)/$(mode_label plan)" 'accept-edits/bypass/dont-ask/plan'
  assert_eq "$(model_short 'Sonnet 5')|$(model_short Fable)" 'Sonnet|Fable'
  assert_eq "$(tele_fields 'Sonnet 5' 42 high 91 88 0.42 main opus acceptEdits)" 'Sonnet 5→⚖ Opus · high · accept-edits · ⚡91% · $0.42'
  assert_eq "$(fmt_tokens 1000000) $(fmt_tokens 200000) $(fmt_tokens 999)" '1M 200k 999'
  assert_eq "$(tele_fields 'Sonnet 5' 42 high 91 88 0.42 main opus acceptEdits verbose)" 'Sonnet 5→⚖ Opus · ctx 42% · high · accept-edits · ⚡91% (turn 88%) · $0.42 · ⎇ main'
  assert_eq "$(tele_fields Haiku '' '' '' '' '' '' '' '')" 'Haiku'
  assert_eq "$(tele_fields '' '' '' '' '' '' main '' plan)" 'plan'

  t "_statusline: keeps the JSON and a digest, then prints gensokyo's own status line"
  fresh; rm -rf "$TELE_DIR"
  rec "$id" slot=1 name=Reimu cwd=/tmp pane=%9 window=stage1
  out=$("$root/bin/gensokyo" _statusline "$id" < "$here/fixtures/statusline.json")
  assert_eq "$out" 'Sonnet 5→⚖ Opus · medium · ░░░░░░░░░░ 5% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m'
  assert_eq "$(jq_ -r .model.display_name "$TELE_DIR/$id.json")" 'Sonnet 5'
  tele_load "$id"
  assert_eq "$T_model|$T_ctx|$T_effort|$T_cache|$T_tcache|$T_cost|$T_five|$T_five_reset|$T_week|$T_week_reset|$T_advisor|$T_added|$T_removed|$T_dur" \
    'Sonnet 5|5|medium|93|99|0.18978259999999997|36|1788543000|49|1788595200|opus|8|0|325'
  assert_re "$T_at" '^[0-9]+$'

  t "_statusline: a sparse report (haiku, API key: no effort, no rate limits, no usage yet) leaves fields empty"
  out=$(jq_ 'del(.effort, .rate_limits, .prompt_cache, .cost) | .context_window.current_usage = null' "$here/fixtures/statusline.json" \
    | "$root/bin/gensokyo" _statusline "$id")
  assert_eq "$out" 'Sonnet 5→⚖ Opus · ░░░░░░░░░░ 5% of 1M'
  tele_load "$id"
  assert_eq "$T_model|$T_ctx|$T_effort|$T_cache|$T_tcache|$T_five|$T_week" 'Sonnet 5|5|||||'
  assert_eq "$(usage_text)" ''

  t "_statusline: STATUSLINE=user runs the user's own status line command with the same JSON; own line when they have none"
  printf 'STATUSLINE=user\n' > "$CONFIG_DIR/config"
  out=$("$root/bin/gensokyo" _statusline "$id" < "$here/fixtures/statusline.json")
  assert_eq "$out" 'USER LINE'
  assert_eq "$(jq_ -r .session_id "$scratch/sl.in")" f2fe56c9-466e-4333-bed0-4a89460dd0b8
  out=$(CLAUDE_CONFIG_DIR=$scratch/cc-empty "$root/bin/gensokyo" _statusline "$id" < "$here/fixtures/statusline.json")
  assert_match "$out" 'Sonnet 5 · medium · ░░░░░░░░░░ 5% of 1M'
  assert_eq "$(rec_get "$TELE_DIR/$id.kv" advisor)" ''
  rm -f "$CONFIG_DIR/config"

  t "usage_text: newest digest with rate limits wins; countdowns only for resets still ahead"
  now=$(date +%s)
  printf 'five=36\nfive_reset=%s\nweek=49\nweek_reset=%s\nat=%s\n' "$((now + 7900))" "$((now + 275000))" "$now" > "$TELE_DIR/$id.kv"
  printf 'five=80\nfive_reset=%s\nweek=90\nweek_reset=%s\nat=%s\n' "$((now + 100))" "$((now + 100))" "$((now - 60))" > "$TELE_DIR/$id2.kv"
  assert_eq "$(usage_text)" '5h ▓▓▓░░░░░░░ 36% ↻2h11m   wk ▓▓▓▓░░░░░░ 49% ↻3d4h'
  printf 'five=36\nfive_reset=%s\nweek=49\nweek_reset=\nat=%s\n' "$((now - 5))" "$now" > "$TELE_DIR/$id.kv"
  assert_eq "$(usage_text)" '5h ▓▓▓░░░░░░░ 36%   wk ▓▓▓▓░░░░░░ 49%'
  rm -f "$TELE_DIR/$id.kv"
  assert_eq "$(usage_text)" '5h ▓▓▓▓▓▓▓▓░░ 80% ↻1m   wk ▓▓▓▓▓▓▓▓▓░ 90% ↻1m'
  fresh; rm -rf "$TELE_DIR"
}

recall_tests() {
  local out id1=aaaaaaaa-1111-4000-8000-000000000001 id2=bbbbbbbb-2222-4000-8000-000000000002 id3=cccccccc-3333-4000-8000-000000000003

  t "recall_rows: departed residents in panes and archived records, newest first, transcript found by id"
  fresh; rm -rf "$DEPARTED_DIR"; mkdir -p "$DEPARTED_DIR" "$scratch/cc/projects/-tmp-a" "$scratch/cc/projects/-tmp-b"
  rec $id1 slot=1 name=Reimu cwd=/tmp/a pane=%1 window=stage1                      # here: not listed
  rec $id2 slot=2 name=Youmu cwd=/tmp/a pane=%2 window=stage1 departed=1700000300   # departed screen in its pane
  printf 'slot=3\nname=Sakuya\ncwd=/tmp/b\nwindow=stage1\nlaunched=1700000000\ndeparted=1700000200\nexit=0\n' > "$DEPARTED_DIR/$id3"
  : > "$scratch/cc/projects/-tmp-b/$id3.jsonl"; touch -t 202001010000 "$scratch/cc/projects/-tmp-b/$id3.jsonl"   # older than Youmu
  assert_eq "$(recall_rows | cut -d'|' -f2-5)" "$id2|Youmu|/tmp/a|2
$id3|Sakuya|/tmp/b|"
  assert_eq "$(recall_rows | sed -n 1p | cut -d'|' -f1,6)" '1700000300|'
  assert_eq "$(recall_rows | sed -n 2p | cut -d'|' -f6)" "$scratch/cc/projects/-tmp-b/$id3.jsonl"

  t "find_departed: name in any case, else an id prefix of 4+ characters; only archived records"
  assert_eq "$(find_departed sakuya)" "$DEPARTED_DIR/$id3"
  assert_eq "$(find_departed CCCC)" "$DEPARTED_DIR/$id3"
  assert_fails find_departed youmu   # departed, but still in its pane: find_resident's business
  assert_fails find_departed ccc
  assert_fails find_departed Nobody

  t "resume alone lists who can be recalled, --json the same for tools; wrong names and options fail"
  out=$(cmd_resume)
  assert_re "$out" '^  Youmu +/tmp/a +bbbbbbbb +[0-9]+[a-z]+ ago · still in its pane \(slot 2\) · no transcript$'
  assert_re "$out" '^  Sakuya +/tmp/b +cccccccc +[0-9]+[a-z]+ ago$'
  assert_match "$out" 'gensokyo resume <name|session id>'
  assert_eq "$(cmd_resume --json | jq_ -r '.[] | "\(.name) \(.slot) \(.in_pane) \(.transcript) \(.departed_at)"')" "Youmu 2 true false 1700000300
Sakuya null false true $(mtime_of "$scratch/cc/projects/-tmp-b/$id3.jsonl")"
  assert_match "$("$root/bin/gensokyo" resume Nobody 2>&1)" "nobody called 'Nobody' has departed"
  assert_match "$("$root/bin/gensokyo" resume reimu 2>&1)" 'Reimu is still here (slot 1)'
  assert_match "$("$root/bin/gensokyo" recall --bogus 2>&1)" 'unknown option --bogus'
  assert_match "$("$root/bin/gensokyo" resume a b 2>&1)" 'one resident at a time'
  fresh; rm -rf "$DEPARTED_DIR" "$scratch/cc/projects"
  assert_match "$(cmd_resume)" 'nobody has departed'
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
# attach_headless: an attached client without a terminal, through script(1)'s pty (BSD and
# util-linux spellings); it lives until detach-client or kill-server. Its stdin is a pipe that
# stays open for a minute: script forwards an EOF on stdin as Ctrl-D into the pty, and the
# client would type that into the active pane (the stub exits on it, Claude Code asks twice).
attach_headless() {
  if script --version >/dev/null 2>&1; then
    (sleep 60 | script -q -c "$TMUX_BIN -L $GENSOKYO_SOCKET attach -t =gensokyo" /dev/null >/dev/null 2>&1 &)
  else
    (sleep 60 | script -q /dev/null "$TMUX_BIN" -L "$GENSOKYO_SOCKET" attach -t =gensokyo >/dev/null 2>&1 &)
  fi
  pause 1
}
cleanup() { [ -n "${TMUX_BIN:-}" ] && tm kill-server 2>/dev/null; rm -rf "$scratch"; }
trap cleanup EXIT

smoke_tests() {
  local out pane1 pane2 pane id1 id2 id3 id4 name3 args
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

  t "smoke: the status line is configured per client kind, at attach time"
  apply_status cc                     # iTerm2 draws its own bar from status-left/status-right
  assert_eq "$(tm show -gv status)" on
  assert_nomatch "$(tm show -g status-format)" '_bar 1'
  assert_match "$(tm show -g status-format)" 'status-left'   # tmux's own format, which draws what we push
  assert_eq "$(tm show -gv status-left-length)" 200
  apply_status tty                    # the plain client gets the two rows gensokyo draws
  assert_eq "$(tm show -gv status)" 2
  assert_match "$(tm show -g status-format)" '_bar 1'

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
  assert_match "$args" '--settings {"hooks":{"UserPromptSubmit"'
  assert_match "$args" '--model haiku --permission-mode plan'
  assert_eq "$(rec_get "$RES_DIR/$id2" mode)" plan
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

  t "smoke: a Stop hook turns the chip gold, rings the bell and alerts the desktop; typing clears it"
  : > "$scratch/notify.log"
  payload "$id1" Stop ',"permission_mode":"acceptEdits","last_assistant_message":"done here"' | TMUX_PANE=$pane1 "$G" _hook
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane2")" "#[bg=$CFG_COLOR_AWAIT,fg=black] 1 ✦ Alpha "
  assert_eq "$(tm display -p -t =gensokyo:stage1 '#{window_bell_flag}')" 1
  assert_eq "$(notifications)" 'Alpha|Alpha is done: done here'
  assert_re "$("$G" list)" '^  1   ✦  Alpha .*waiting \(done here\)$'
  assert_eq "$("$G" list --json | jq_ -r '.[0] | "\(.status) \(.permission_mode) \(.detail)"')" 'waiting acceptEdits done here'
  assert_eq "$("$G" list --json | jq_ -r '.[1] | "\(.status) \(.permission_mode) \(.detail)"')" 'idle plan null'
  payload "$id1" UserPromptSubmit ',"permission_mode":"acceptEdits"' | "$G" _hook
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane2")" '#[default] 1 ○ Alpha '

  t "smoke: no desktop alert for the pane on screen in a client that was just used"
  tm select-window -t =gensokyo:stage1 \; select-pane -t "$pane1"
  attach_headless
  if [ "$(tm list-clients 2>/dev/null | wc -l | tr -d ' ')" != 1 ]; then skip "could not attach a headless client (script(1))"; else
    t "smoke: an attached client is counted by its kind (a plain one here, not control mode)"
    assert_eq "$(clients_in_mode tty)|$(clients_in_mode cc)" '1|0'
    assert_match "$(client_summary)" '1 plain'
    t "smoke: no desktop alert for the pane on screen in a client that was just used"
    : > "$scratch/notify.log"
    payload "$id1" Stop ',"permission_mode":"default","last_assistant_message":"seen"' | "$G" _hook
    assert_eq "$(notifications)" ''
    payload "$id2" Stop ',"permission_mode":"default","last_assistant_message":"unseen"' | "$G" _hook
    assert_eq "$(notifications)" 'Beta|Beta is done: unseen'
    tm detach-client; pause 0.3
    assert_eq "$(tm list-clients 2>/dev/null | wc -l | tr -d ' ')" 0
  fi
  payload "$id1" UserPromptSubmit '' | "$G" _hook; payload "$id2" UserPromptSubmit '' | "$G" _hook

  t "smoke: a status line report puts model and context in the chip, telemetry in the border, usage in row 2 and everything in list"
  mkdir -p "$scratch/work/alpha/.git"; echo 'ref: refs/heads/main' > "$scratch/work/alpha/.git/HEAD"
  jq_ --arg id "$id1" --argjson now "$(date +%s)" \
    '.session_id = $id | .rate_limits.five_hour.resets_at = $now + 7900 | .rate_limits.seven_day.resets_at = $now + 275000' \
    "$here/fixtures/statusline.json" > "$scratch/sl.json"
  assert_eq "$("$G" _statusline "$id1" < "$scratch/sl.json")" 'Sonnet 5→⚖ Opus · medium · ░░░░░░░░░░ 5% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m'
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane2")" '#[default] 1 ○ Alpha Sonnet 5% #[default]│#[bold] 2 ○ Beta #[default]'
  assert_match "$("$G" _bar 2)" '#[align=right]5h ▓▓▓░░░░░░░ 36% ↻2h11m   wk ▓▓▓▓░░░░░░ 49% ↻3d4h '
  payload "$id1" UserPromptSubmit ',"permission_mode":"plan"' | "$G" _hook
  assert_eq "$("$G" _border "$pane1" '✳ Alpha')" ' 1 Alpha · alpha ⎇ main · Sonnet 5→⚖ Opus · medium · plan · ⚡93% · $0.19 '
  assert_eq "$("$G" _border "$pane2" '✳ Beta')" ' 2 Beta · beta · default '   # the Stop above reported default
  out=$("$G" list --json)
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[0] | .telemetry | "\(.model) \(.context_pct) \(.effort) \(.cache_pct) \(.turn_cache_pct) \(.advisor) \(.five_hour.used_pct) \(.seven_day.used_pct) \(.cost_usd)"')" \
    'Sonnet 5 5 medium 93 99 opus 36 49 0.18978259999999997'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[0].branch, .[1].branch, .[1].telemetry')" 'main
null
null'
  out=$("$G" list)
  assert_match "$out" 'Sonnet 5→⚖ Opus · ctx 5% · medium · plan · ⚡93% (turn 99%) · $0.19 · ⎇ main · '
  assert_match "$out" '  usage  5h ▓▓▓░░░░░░░ 36% ↻2h11m   wk ▓▓▓▓░░░░░░ 49% ↻3d4h   ('

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
  assert_re "$("$G" resume)" '^  Gamma +.*beta +'"${id2:0:8}"' +[0-9]+[a-z]+ ago · still in its pane \(slot 2\) · no transcript$'

  t "smoke: resume recalls a departed pane in place with --resume and the summon flags; the name stays"
  out=$("$G" resume gamma 2>&1)
  assert_match "$out" 'recalled Gamma into its pane (slot 2)'
  pause 1.2
  assert_match "$(tm capture-pane -p -t "$pane2")" "stub-claude Gamma ($id2) resumed"
  args=$(cat "$STUB_STATE/$id2.args")
  assert_match "$args" "--resume $id2"
  assert_nomatch "$args" '--session-id'
  assert_match "$args" '--model haiku --permission-mode plan'
  assert_match "$args" '--settings {"hooks":{"UserPromptSubmit"'
  assert_eq "$(rec_get "$RES_DIR/$id2" departed)|$(rec_get "$RES_DIR/$id2" pane)" "|$pane2"
  assert_match "$("$G" resume gamma 2>&1)" 'Gamma is still here (slot 2)'
  rm -f "$REGISTRY"
  assert_re "$("$G" list)" '^  2   ○  Gamma .*idle$'
  "$G" close gamma >/dev/null 2>&1; pause 1.2
  assert_match "$(tm capture-pane -p -t "$pane2")" 'Gamma has left the shrine'

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
  rm -f "$REGISTRY"
  assert_match "$("$G" list)" '4 from earlier runs can be recalled: gensokyo resume'

  t "smoke: a resident from an earlier run is recalled into a fresh slot; its record moves back"
  id3=$(grep -l '^slot=3$' "$STATE_DIR/departed"/* | head -n 1); id3=${id3##*/}; name3=$(rec_get "$STATE_DIR/departed/$id3" name)
  mkdir -p "$scratch/cc/projects/-work-alpha"; : > "$scratch/cc/projects/-work-alpha/$id3.jsonl"   # a fresh transcript: newest
  assert_eq "$("$G" resume | sed -n 2p | awk '{print $1}')" "$name3"
  assert_eq "$("$G" resume --json | jq_ -r --arg id "$id3" 'map(select(.session_id == $id)) | .[0] | "\(.transcript) \(.in_pane) \(.slot)"')" 'true false null'
  out=$("$G" resume "$name3" 2>&1)
  assert_match "$out" "recalled $name3 (slot 1) in $scratch/work/alpha"
  assert_nomatch "$out" 'no transcript'
  pause 1.5
  assert_ok test -f "$RES_DIR/$id3"
  assert_ok test ! -f "$STATE_DIR/departed/$id3"
  assert_eq "$(rec_get "$RES_DIR/$id3" slot)|$(rec_get "$RES_DIR/$id3" window)|$(rec_get "$RES_DIR/$id3" resume)|$(rec_get "$RES_DIR/$id3" launched)" "1|stage1|1|$(rec_get "$RES_DIR/$id3" launched)"
  pane=$(rec_get "$RES_DIR/$id3" pane)
  assert_match "$(tm capture-pane -p -t "$pane")" "($id3) resumed"
  assert_match "$(cat "$STUB_STATE/$id3.args")" "--resume $id3"
  assert_eq "$(tm list-panes -t =gensokyo:stage1 -F x | wc -l | tr -d ' ')" 1   # took the shrine's place
  assert_eq "$(ls "$STATE_DIR/departed" | wc -l | tr -d ' ')" 3
  assert_match "$("$G" resume "$name3" 2>&1)" "$name3 is still here (slot 1)"
  rm -f "$REGISTRY"
  assert_re "$("$G" list)" "^  1   ○  $name3 .*idle\$"
  assert_match "$("$G" list)" '3 from earlier runs can be recalled'

  t "smoke: recall refuses a name that is here already and a directory that is gone; the record stays archived"
  id4=$(ls "$STATE_DIR/departed" | head -n 1)
  rec_set "$STATE_DIR/departed/$id4" name "$name3"
  assert_match "$("$G" resume "${id4:0:8}" 2>&1)" "another $name3 is here already"
  rec_set "$STATE_DIR/departed/$id4" name Gone; rec_set "$STATE_DIR/departed/$id4" cwd "$scratch/work/vanished"
  assert_match "$("$G" resume gone 2>&1)" "Gone's directory is gone: $scratch/work/vanished"
  assert_ok test -f "$STATE_DIR/departed/$id4"
  out=$("$G" resume 2>&1)
  assert_match "$out" 'no transcript'
  tm kill-server
}

case $what in
  unit) unit_tests; hook_tests; telemetry_tests; recall_tests; install_tests ;;
  smoke) smoke_tests ;;
  all) unit_tests; hook_tests; telemetry_tests; recall_tests; install_tests; smoke_tests ;;
esac
printf '\n%s passed, %s failed, %s skipped\n' "$pass" "$fail" "$skipped"
[ "$fail" -eq 0 ]
