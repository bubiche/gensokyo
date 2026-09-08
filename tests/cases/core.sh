# tests/cases/core.sh - the pieces every command is built from: paths, records, the registry, config, doctor.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

core_tests() {
  local out f qw was_tmux was_ask mon was_now

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

  t "prune_records drops a record whose pane has gone - but never while a quit is going on"
  fresh; rec p1 slot=1 name=Reimu cwd=/tmp/a pane=%1; rec p2 slot=2 name=Youmu cwd=/tmp/a pane=%7
  was_tmux=$(declare -f tmux_)
  tmux_() { case $1 in list-panes) printf '%%1\n' ;; esac; return 0; }   # %7 is not there any more
  # `gensokyo quit` asks everyone to leave and then kills the server, so its panes go one after
  # another and this is what a tick in the middle of it sees. Dropping the record there loses the
  # resident for good: those records are what the next cockpit offers back under `resume`.
  : > "$STATE_DIR/quitting"
  prune_records
  assert_ok test -f "$RES_DIR/p2"
  # The marker is not forever - start_clock clears it - and afterwards the same record is pruned
  # exactly as it was before.
  rm -f "$STATE_DIR/quitting"
  prune_records
  assert_ok test ! -f "$RES_DIR/p2"
  # An interrupted quit does not switch pruning off for good: the marker has a life, because the
  # clock that would clear it is already running and will not start again.
  : > "$STATE_DIR/quitting"; touch -t "$(date -v-5M +%Y%m%d%H%M)" "$STATE_DIR/quitting"
  assert_fails quit_in_progress
  rm -f "$STATE_DIR/quitting"
  assert_ok test -f "$RES_DIR/p1"
  eval "$was_tmux"

  t "departed_within: the record, not the pane, is what says a /exit was read"
  fresh; rec d1 slot=1 name=Reimu cwd=/tmp/a pane=%1
  assert_fails departed_within d1 1
  rec_set "$RES_DIR/d1" departed 1700000000
  assert_ok departed_within d1 1

  t "quit_wait asks whoever has not answered a second time, and reports the ones that never do"
  fresh; rec q1 slot=1 name=Reimu cwd=/tmp/a pane=%1
  was_ask=$(declare -f ask_exit); rm -f "$STATE_DIR/asked"
  ask_exit() { printf '%s\n' "$1" >> "$STATE_DIR/asked"; }
  qw=$QUIT_WAIT; QUIT_WAIT=2
  assert_eq "$(quit_wait q1)" 1
  assert_eq "$(grep -c '^%1$' "$STATE_DIR/asked")" 1   # once, not once a tick
  # One that has left is not asked again: it is at its departed screen, where `x` closes its pane.
  rec_set "$RES_DIR/q1" departed 1700000000; rm -f "$STATE_DIR/asked"
  assert_eq "$(quit_wait q1)" 0
  assert_ok test ! -f "$STATE_DIR/asked"
  QUIT_WAIT=$qw; eval "$was_ask"
  t "a temp file left by a killed writer is swept, and one being written now is not"
  fresh
  : > "$STATE_DIR/registry.json.tmp.4242"          # yesterday's, from a process that was killed
  touch -t "$(date -v-2d +%Y%m%d%H%M)" "$STATE_DIR/registry.json.tmp.4242"
  : > "$STATE_DIR/registry.json.tmp.4243"          # a write happening right now
  : > "$STATE_DIR/registry.json"                   # not a temp file at all
  mkdir -p "$STATE_DIR/statusline"
  : > "$STATE_DIR/statusline/abc.json.tmp.99"      # the same shape, one directory down
  touch -t "$(date -v-2d +%Y%m%d%H%M)" "$STATE_DIR/statusline/abc.json.tmp.99"
  sweep_stale_temps
  assert_ok   test ! -e "$STATE_DIR/registry.json.tmp.4242"
  assert_ok   test ! -e "$STATE_DIR/statusline/abc.json.tmp.99"
  assert_ok   test -e "$STATE_DIR/registry.json.tmp.4243"
  assert_ok   test -e "$STATE_DIR/registry.json"
  rm -f "$STATE_DIR/registry.json.tmp.4243" "$STATE_DIR/registry.json"

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
  assert_eq "$(resident_rows)" '1|ed82e343-81ce-4b9e-8fdb-9b32d8136a5c|Marisa|idle|/Users/me/dev/alpha|%1|@1||||||||||||||||
2|f2fe56c9-466e-4333-bed0-4a89460dd0b8|Sakuya|waiting|/Users/me/dev/beta|%2|@1||||||||||||||||
3|33333333-cccc-4000-8000-000000000003|Youmu|departed|/tmp|%3|@1||||||||||||||||
4|44444444-dddd-4000-8000-000000000004|Cirno|starting|/tmp|-|@1||||||||||||||||'
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
  # for it any more, and nothing may promise one.
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

  t "quit: a command of its own, a key in the gensokyo table, and no arguments"
  assert_match "$("$root/bin/gensokyo" help)" 'gensokyo quit'
  assert_eq "$(key_table | grep -c '^gensokyo|q|')" 1
  assert_match "$(key_table | grep '^gensokyo|q|')" '_menu-quit'
  assert_match "$("$root/bin/gensokyo" quit please 2>&1)" 'quit takes no arguments'
  assert_match "$("$root/bin/gensokyo" quit 2>&1)" 'gensokyo is not running'

  t "quit waits for every resident to report departed, and gives up rather than hanging"
  fresh
  rec 33333333-dddd-4000-8000-000000000003 slot=1 name=Suika cwd=/tmp window=@1 pane=%1
  qw=$QUIT_WAIT; QUIT_WAIT=1
  assert_eq "$(quit_wait 33333333-dddd-4000-8000-000000000003)" 1
  rec_set "$RES_DIR/33333333-dddd-4000-8000-000000000003" departed "$(date +%s)"
  assert_eq "$(quit_wait 33333333-dddd-4000-8000-000000000003)" 0
  assert_eq "$(quit_wait 44444444-eeee-4000-8000-000000000004)" 0   # a record already gone counts as left
  QUIT_WAIT=$qw

  t "the plugin manifest is valid, and every skill in it has a name and a description"
  assert_ok test -f "$root/share/plugin/.claude-plugin/plugin.json"
  assert_ok jq_ -e '.name == "gensokyo"' "$root/share/plugin/.claude-plugin/plugin.json"
  # A skill's frontmatter description is in every resident's context whether the skill is ever
  # used, so it is the one part that must be there and must be short.
  for f in "$root"/share/plugin/skills/*/SKILL.md; do
    [ -f "$f" ] || continue
    assert_eq "$(sed -n '1p' "$f")" '---'
    assert_eq "$(sed -n '2,4p' "$f" | sed -n 's/^name: //p')" "$(basename "$(dirname "$f")")"
    assert_re "$(sed -n '2,6p' "$f")" '^description: .+'
  done

  t "plugin_skills names what the plugin carries, so doctor can say it"
  assert_eq "$(plugin_skills)" 'gensokyo-peers gensokyo-ritual'

  t "the peer skill leads with the reply channel, the thing that had to be learned twice"
  out=$(cat "$root/share/plugin/skills/gensokyo-peers/SKILL.md")
  assert_match "$out" 'the reply must come back as a `SendMessage` addressed to'
  assert_match "$out" 'ListAgents'
  # Saying one thing to everybody is a button; the skill has to point back at it rather than
  # teach a resident to fan out SendMessage calls by hand.
  assert_match "$out" 'gensokyo broadcast'

  t "the ritual skill sends the schedule here, confirms first, and passes the warning on"
  out=$(cat "$root/share/plugin/skills/gensokyo-ritual/SKILL.md")
  # The description is the routing surface, and it was measured losing: with the words missing,
  # "every weekday at 9..." went to Claude Code's own scheduling instead. What a user actually
  # says has to be in it.
  assert_match "$(sed -n '2,4p' "$root/share/plugin/skills/gensokyo-ritual/SKILL.md")" 'every weekday at 9:05'
  assert_match "$out" 'Do not use the built-in `schedule` skill, `CronCreate`, or scheduled tasks'
  # A ritual runs unattended, so the two things that cannot be left to the model's judgement:
  # ask before writing one, and hand on what `ritual add` warned about.
  assert_match "$out" 'Confirm before you create anything'
  assert_match "$out" 'Never leave it out of what you report'
  assert_match "$out" 'gensokyo ritual add --name slack-morning'
  assert_match "$out" 'gensokyo ritual disable slack-morning'
  # And what a ritual is not, so it is not promised: no clock while the cockpit is down.
  assert_match "$out" 'only fire while gensokyo is running'
  # Measured 2026-09-08: a session that knows the user's timezone is also told to convert a local
  # time to UTC before writing a cron line, and it does it to a ritual - five runs in eight - while
  # saying back the hour that was asked for, so the confirmation reads as agreeing with itself. The
  # rule belongs both in the flag and in what gets confirmed, which is where it gets said wrong.
  assert_match "$out" 'never convert it to UTC'
  assert_match "$out" "this machine's local time"

  t "system_paragraph says three things and teaches nothing a button already does"
  out=$(system_paragraph Marisa)
  assert_match "$out" 'resident name is Marisa'
  assert_match "$out" 'SendMessage'
  # Both skills are named here, and both are named as a prohibition rather than a suggestion.
  # Measured 2026-09-07: a resident told only that it *may* use gensokyo-peers does not - it
  # writes the message itself and leaves out the reply channel, which is the whole failure the
  # skill exists to prevent. "never compose ... yourself" is what made it fire.
  assert_match "$out" 'gensokyo-peers'
  assert_match "$out" 'never compose'
  assert_match "$out" 'gensokyo-ritual'
  assert_match "$out" 'never the built-in'
  # The clause the built-in's own procedure makes necessary: it converts a local time to UTC, and
  # a ritual is not in UTC. Named at the grant, for the same reason gensokyo-peers is.
  assert_match "$out" 'never convert a time you are given to UTC'
  # Reading is granted as explicitly as writing. Asked what it had scheduled, a resident answered
  # off CronList and said "nothing" over three rituals on disk: a sentence about setting a schedule
  # up does not reach a question about what is already there.
  assert_match "$out" 'question about what is scheduled'
  # Both skills ship, so the paragraph names them and stops: no command of gensokyo's own is in
  # here, because a resident that can read a skill does not need the CLI spelled out for it.
  for f in summon banish recall 'gensokyo new' 'gensokyo ritual' 'gensokyo close' 'gensokyo list' focus tmux; do
    assert_nomatch "$out" "$f"
  done
  out=$("$root/bin/gensokyo" help --json)   # the later tests read this again

  t "real_path follows a chain of relative and absolute symlinks"
  mkdir -p "$scratch/rp/a" "$scratch/rp/b"; : > "$scratch/rp/b/target"
  ln -s ../b/target "$scratch/rp/a/rel"; ln -s "$scratch/rp/a/rel" "$scratch/rp/abs"
  assert_eq "$(real_path "$scratch/rp/abs")" "$scratch/rp/b/target"
  assert_eq "$(real_path "$scratch/rp/b/target")" "$scratch/rp/b/target"

  t "doctor reports the plugin and whether gensokyo is on PATH"
  out=$(PATH=/usr/bin:/bin cmd_doctor)
  assert_match "$out" "plugin     $root/share/plugin (loaded into every resident; skills: gensokyo-peers gensokyo-ritual)"
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

  t "home_mode: iTerm2 gets the native tabs, --tty and --nested the plain tmux client"
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


  t "the bar's second row carries the next ritual, as a clock time and a name"
  mon=$(rt_at '2026-09-07 09:05:00'); was_now=${GENSOKYO_NOW:-}; GENSOKYO_NOW=$mon
  printf 'slack-morning|yes|%s|3 9 * * 1-5|check Slack\n' "$((mon + 3600))" > "$STATE_DIR/timetable"
  touch -t 203001010000 "$STATE_DIR/timetable"
  assert_eq "$(bar_ritual_text)" '⏲ 10:05 slack-morning'
  assert_match "$(cmd__bar 2 '' plain)" '⏲ 10:05 slack-morning'
  assert_match "$(cmd__bar 2 '')" '· ⏲ 10:05 slack-morning'
  # A fire days away is not a time of day: the day it falls on is the part worth the columns.
  printf 'weekly-sweep|yes|%s|@weekly|\n' "$((mon + 86400 * 3))" > "$STATE_DIR/timetable"
  touch -t 203001010000 "$STATE_DIR/timetable"
  assert_eq "$(bar_ritual_text)" '⏲ Thu 09:05 weekly-sweep'

  t "and nothing at all when nothing is going to fire: the bar is not where that is said"
  printf 'nightly|no||@daily|\n' > "$STATE_DIR/timetable"
  touch -t 203001010000 "$STATE_DIR/timetable"
  assert_eq "$(bar_ritual_text)" ''
  assert_eq "$(cmd__bar 2 '' plain)" "$(usage_text)"
  rm -f "$STATE_DIR/timetable"
  GENSOKYO_NOW=$was_now

  t "the tty ritual menus refuse what is not there rather than dying inside a run-shell"
  # Both run as children of the tmux server, where a `die` would take the job down without a
  # word to anybody; and with no server here every tmux_ call fails - so what is asserted is
  # that the refusal path still comes back.
  assert_ok cmd__menu-ritual someclient nothing-of-the-sort
  : > "$STATE_DIR/timetable"; touch -t 203001010000 "$STATE_DIR/timetable"
  assert_ok cmd__menu-timetable someclient
  rm -f "$STATE_DIR/timetable"

  t "clock_age is empty until the clock has ticked once"
  rm -f "$STATE_DIR/clock"
  assert_eq "$(clock_age)" ''
  : > "$STATE_DIR/clock"
  assert_re "$(clock_age)" '^[0-9]+$'
}
