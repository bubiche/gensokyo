# tests/cases/core.sh - the pieces every command is built from: paths, records, the registry, config, doctor.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

core_tests() {
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

  t "glyph_for"
  assert_eq "$(glyph_for busy)$(glyph_for waiting)$(glyph_for idle)$(glyph_for departed)$(glyph_for starting)" '●✦○·○'

  t "resident_title: what a tab reads"
  assert_eq "$(resident_title 1 waiting Reimu)" '1 ✦ Reimu'
  assert_eq "$(resident_title 3 starting Sakuya)" '3 ○ Sakuya'

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
  printf '# gensokyo\nPREFIX=C-a\nMOUSE=on\nbad key=1\n\n' > "$CONFIG_DIR/config"
  out=$(load_config 2>&1; printf '%s|%s' "$CFG_PREFIX" "$CFG_MOUSE")
  assert_match "$out" 'ignoring line: bad key=1'
  assert_match "$out" 'C-a|on'
  rm -f "$CONFIG_DIR/config"
  CFG_PREFIX=C-Space; CFG_MOUSE=off

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
  fresh; rec ed82e343-81ce-4b9e-8fdb-9b32d8136a5c slot=1 name=Marisa cwd=/Users/me/dev/alpha pane=%1 window=@1
  assert_eq "$(outsider_count)" 2

  t "resident_rows: registry state and name win over the record; departed wins over both"
  rec f2fe56c9-466e-4333-bed0-4a89460dd0b8 slot=2 name=OldName cwd=/Users/me/dev/beta pane=%2 window=@1
  rec 33333333-cccc-4000-8000-000000000003 slot=3 name=Youmu cwd=/tmp pane=%3 window=@1 departed=1
  rec 44444444-dddd-4000-8000-000000000004 slot=4 name=Cirno cwd=/tmp window=@1
  assert_eq "$(resident_rows)" '1|ed82e343-81ce-4b9e-8fdb-9b32d8136a5c|Marisa|idle|/Users/me/dev/alpha|%1|@1|||||||||||||||
2|f2fe56c9-466e-4333-bed0-4a89460dd0b8|Sakuya|waiting|/Users/me/dev/beta|%2|@1|||||||||||||||
3|33333333-cccc-4000-8000-000000000003|Youmu|departed|/tmp|%3|@1|||||||||||||||
4|44444444-dddd-4000-8000-000000000004|Cirno|starting|/tmp|-|@1|||||||||||||||'
  assert_eq "$(rec_get "$RES_DIR/f2fe56c9-466e-4333-bed0-4a89460dd0b8" name)" Sakuya

  t "resident_rows: a pending question or finished turn from the hooks shows as needing you"
  fresh; rm -rf "$STATE_DIR/status"
  rec ed82e343-81ce-4b9e-8fdb-9b32d8136a5c slot=1 name=Marisa cwd=/a pane=%1 window=@1 mode=plan   # idle in the registry
  rec f2fe56c9-466e-4333-bed0-4a89460dd0b8 slot=2 name=Sakuya cwd=/b pane=%2 window=@1            # waiting
  rec 0b1c2d3e-0000-4000-8000-000000000003 slot=3 name=Youmu cwd=/c pane=%3 window=@1             # null -> idle
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
  rec aaaaaaaa-0000-4000-8000-000000000001 slot=1 name=Reimu cwd=/a pane=%1 window=@1
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
  # Focusing a resident is a click on its tab or its line in the shrine, so there is no command
  # for it any more and the handbook skill must not promise one.
  assert_eq "$(printf '%s' "$out" | jq_ -r '[.commands[].name] | index("focus") != null')" false
  assert_eq "$(printf '%s' "$out" | jq_ -r '[.commands[].name] | index("send") != null')" false
  assert_eq "$(printf '%s' "$out" | jq_ -r '[.commands[].name] | index("stage") != null')" false
  assert_eq "$(printf '%s' "$out" | jq_ -r '.prefix')" C-Space
  for f in new close list resume help doctor; do
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

  t "bar_escape doubles the two characters tmux would eat before iTerm2 sees the text"
  assert_eq "$(bar_escape '1 ✦ Reimu 42% | p#{x}')" '1 ✦ Reimu 42%% | p##{x}'
  assert_eq "$(bar_escape '')" ''

  t "the pushed bar is plain text; with nobody here it says so without naming a key"
  fresh; rm -f "$REGISTRY"
  assert_eq "$(cmd__bar 1 '' plain)" ' ⛩ gensokyo  no residents yet '
  assert_match "$(cmd__bar 1 '')" 'then g n to summon'


  t "clock_age is empty until the clock has ticked once"
  rm -f "$STATE_DIR/clock"
  assert_eq "$(clock_age)" ''
  : > "$STATE_DIR/clock"
  assert_re "$(clock_age)" '^[0-9]+$'
}
