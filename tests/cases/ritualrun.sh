# tests/cases/ritualrun.sh - firing a ritual (lib/ritualrun.sh): which minute has come round,
# whether the last run is still going, and the record the run is launched from. Sourced by
# tests/run.sh, which holds the harness, the scratch state dir and the sourced bin/gensokyo
# these tests call. No tmux: open_pane and the live pane list are stood in for, and the pane a
# fire really opens is the smoke test's business.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

# rr_load <slug> <frontmatter line>...: a ritual file with a prompt and a real directory,
# loaded, so the RIT_* the functions under test read are this ritual's.
rr_load() {
  local slug=$1
  shift
  { printf -- '---\n'; printf '%s\n' "$@"; printf -- 'cwd: %s\n---\ndo the thing\n' "$HOME"; } \
    > "$CONFIG_DIR/rituals/$slug.md"
  ritual_load "$CONFIG_DIR/rituals/$slug.md"
}

# The flag string as the pane will see it: `_run` eval's it back apart into arguments, so these
# ask how many there are and what the nth one is.
rr_argc() { eval "set -- $*"; printf '%s' "$#"; }
rr_argv() {
  local n=$1
  shift
  eval "set -- $*"
  while [ "$n" -gt 1 ]; do shift; n=$((n - 1)); done
  printf '%s' "${1:-}"
}

# rr_headless_wait <slug>: hold until that headless run is over. A headless run is a process of
# its own, so its log and its journal line land when they land - waited for, never paused for.
rr_headless_wait() {
  local n=0
  while ritual_headless_running "$1"; do
    nap 0.1; n=$((n + 1))
    [ "$n" -gt 100 ] && return 1
  done
  return 0
}

# rr_headless <slug> <why>: one headless run, in a subshell - `_headless` cd's into the ritual's
# directory and scrubs the environment, which is a process's business and not this one's.
rr_headless() { ( cmd__headless "$@" ); }

# rr_since <session-id> <pending> <epoch>: a status file saying that resident has been in that
# state since then. Forged rather than waited for: a tab's life is not a schedule, so `keep` is
# measured on the real clock and GENSOKYO_NOW does not move it - an hour of sitting there has to
# be written down instead of passing.
rr_since() {
  mkdir -p "$STATE_DIR/status"
  printf 'pending=%s\ndetail=\nsince=%s\nmode=\n' "$2" "$3" > "$STATE_DIR/status/$1"
}

# rr_tmux: what the last stub tmux_ was asked to do, one command per line.
rr_tmux() { cat "$scratch/rr-tmux" 2>/dev/null; }

# The stand-in for typing a prompt into somebody's input line: it keeps the pane and the text,
# and answers with whatever the test has put in RR_CAST_RC (0 typed and submitted, 1 tmux
# refused, 2 it never reached the input line - cast_type's own three answers).
RR_CAST_RC=0
rr_cast_type() {
  printf '%s' "$1" > "$scratch/cast-pane"
  printf '%s' "$2" > "$scratch/cast-text"
  return "$RR_CAST_RC"
}
rr_cast_pane() { cat "$scratch/cast-pane" 2>/dev/null; }
rr_cast_text() { cat "$scratch/cast-text" 2>/dev/null; }
rr_cast_none() { rm -f "$scratch/cast-pane" "$scratch/cast-text"; }

# rr_runlog <slug>: the newest run log of that ritual, whole.
rr_runlog() {
  local f last=''
  for f in "$STATE_DIR/rituals/$1"/runs/*.log; do [ -f "$f" ] && last=$f; done
  [ -n "$last" ] && cat "$last"
}

ritual_run_tests() {
  local mine=$CONFIG_DIR/rituals mon tue out rec was_pane was_live was_now was_mode id
  local was_keep i long was_tmux was_ask real_now was_cast was_ready was_resume was_blocked
  mkdir -p "$mine"
  rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"
  mon=$(rt_at '2026-09-07 09:05:00')   # a Monday, the Slack example's own minute
  tue=$(rt_at '2026-09-08 09:05:00')
  was_now=${GENSOKYO_NOW:-}; was_mode=$CFG_PERMISSION_MODE
  CFG_PERMISSION_MODE=''
  t "the stamp: the minute a ritual last ran for, and nothing at all when it never has"
  assert_eq "$(ritual_dir probe)" "$STATE_DIR/rituals/probe"
  assert_eq "$(ritual_stamp probe)" ''
  ritual_stamp_set probe "$mon"
  assert_eq "$(ritual_stamp probe)" "$mon"

  t "ritual_reason: the minute that has come round, and once however often the sweep asks"
  GENSOKYO_NOW=$mon
  rr_load probe 'schedule: "5 9 * * *"'
  rm -rf "$STATE_DIR/rituals/probe"
  assert_eq "$(ritual_reason '')" "due $mon"
  assert_eq "$(GENSOKYO_NOW=$((mon + 45)) ritual_reason '')" "due $mon"   # any second of that minute
  ritual_stamp_set probe "$mon"
  assert_fails ritual_reason ''
  assert_fails ritual_reason 1
  GENSOKYO_NOW=$((mon + 60))
  assert_fails ritual_reason ''                                          # the next minute is not its

  t "catch-up: one run for the most recent miss, and only when asked for it"
  rr_load probe 'schedule: "5 9 * * *"'
  GENSOKYO_NOW=$(rt_at '2026-09-07 11:00:00')                            # two hours late
  ritual_stamp_set probe "$(rt_at '2026-09-06 09:05:00')"                # yesterday's run
  assert_eq "$(ritual_reason 1)" "catch-up $mon"
  assert_fails ritual_reason ''                                          # a plain sweep looks forward only
  ritual_stamp_set probe "$mon"                                          # what the catch-up leaves behind
  assert_fails ritual_reason 1                                           # so it happens once
  # A ritual gensokyo has never seen does not run for a minute that passed before it existed:
  # writing the file at 11:00 must not fire this morning's 09:05.
  rm -rf "$STATE_DIR/rituals/probe"
  assert_fails ritual_reason 1
  # Weeks behind is still one run, for the most recent of the misses and not for every one.
  ritual_stamp_set probe "$(rt_at '2026-08-01 09:05:00')"
  assert_eq "$(ritual_reason 1)" "catch-up $mon"
  # And a fire that is further back than the window is not a miss any more.
  rr_load yearly 'schedule: "0 0 1 1 *"'
  ritual_stamp_set yearly "$(rt_at '2025-01-01 00:00:00')"
  assert_fails ritual_reason 1
  rr_load nocatch 'schedule: "5 9 * * *"' 'catch_up: false'
  ritual_stamp_set nocatch "$(rt_at '2026-09-06 09:05:00')"
  assert_fails ritual_reason 1

  t "first sight: the stamp a ritual gets before it has run, so that a miss is a miss from then on"
  rr_load probe 'schedule: "5 9 * * *"'
  rm -rf "$STATE_DIR/rituals/probe"
  GENSOKYO_NOW=$(rt_at '2026-09-07 11:00:30')
  ritual_seen
  assert_eq "$(ritual_stamp probe)" "$(rt_at '2026-09-07 11:00:00')"     # floored to the minute
  GENSOKYO_NOW=$(rt_at '2026-09-07 12:00:00')
  ritual_seen
  assert_eq "$(ritual_stamp probe)" "$(rt_at '2026-09-07 11:00:00')"     # and never moved again

  t "the prompt names the memory file, and the memory file is there for the run to read"
  rr_load probe 'schedule: "@daily"'
  rm -rf "$STATE_DIR/rituals/probe"
  out=$(ritual_prompt_text)
  assert_match "$out" 'do the thing'
  assert_match "$out" "Your notes from previous runs are at \`$STATE_DIR/rituals/probe/memory.md\`."
  assert_match "$out" 'Read them first; update them before you finish.'
  assert_match "$(cat "$STATE_DIR/rituals/probe/memory.md")" '# probe'

  t "the launch flags, with a permission pattern that has spaces in it kept as one argument"
  rr_load flags 'schedule: "@daily"' 'model: haiku' 'effort: low' 'mode: acceptEdits' \
    'mcp_config: ~/mcp.json' 'allowed_tools: ["Read", "Bash(npm run test:*)"]'
  out=$(ritual_args)
  assert_match "$out" '--model haiku'
  assert_match "$out" '--effort low'
  assert_match "$out" '--permission-mode acceptEdits'
  assert_match "$out" "--mcp-config $HOME/mcp.json"
  # What the pane does with the string: --allowedTools is variadic and comes last, and the
  # pattern has to arrive as one word (`_run` puts the prompt behind a `--`).
  assert_eq "$(rr_argc "$out")" 13
  assert_eq "$(rr_argv 13 "$out")" 'Bash(npm run test:*)'
  # The memory file is in there, so reading the notes every prompt names is not a permission
  # prompt (measured: without this even the read is asked for, the write still being the
  # ritual's own business).
  assert_match "$out" "--add-dir $STATE_DIR/rituals/flags"
  rr_load flags 'schedule: "@daily"'
  assert_eq "$(ritual_args)" "--add-dir $STATE_DIR/rituals/flags"
  # A home directory with a space in it: the flags go into the record as one line and are taken
  # apart again inside the pane, so the path has to arrive as one argument. Nothing else in
  # ritual_args has ever had a value that needed the quoting.
  was_state=$STATE_DIR; STATE_DIR="$scratch/two words"
  out=$(ritual_args)
  assert_eq "$(rr_argc "$out")" 2
  assert_eq "$(rr_argv 2 "$out")" "$scratch/two words/rituals/flags"
  STATE_DIR=$was_state
  CFG_PERMISSION_MODE=acceptEdits
  assert_eq "$(ritual_args)" "--permission-mode acceptEdits --add-dir $STATE_DIR/rituals/flags"
  CFG_PERMISSION_MODE=''

  t "overlap: a run still going is one that has not finished, not one still sitting in its tab"
  fresh; rm -rf "$STATE_DIR/status"
  was_live=$(declare -f live_panes)
  live_panes() { printf '%%1\n'; }
  assert_fails ritual_running slack-morning
  rec r-1 slot=1 name=slack-morning ritual=slack-morning pane=%1
  assert_ok ritual_running slack-morning       # no status yet: it has just started
  assert_fails ritual_running nightly-checks   # another ritual's run is not this one's
  status_write r-1 stopped 'triaged 4 threads'
  assert_fails ritual_running slack-morning    # finished, and left in its tab to be read
  status_write r-1 awaits 'permission: Bash'
  assert_ok ritual_running slack-morning       # stuck on a prompt nobody answered: still going
  status_write r-1 '' ''
  assert_ok ritual_running slack-morning       # mid-turn
  rec r-2 slot=2 ritual=departed-run pane=%1 departed=1757000000
  assert_fails ritual_running departed-run
  rec r-3 slot=3 ritual=killed-run pane=%9
  assert_fails ritual_running killed-run       # its pane went behind our back
  rec r-4 slot=4 ritual=starting-run
  assert_ok ritual_running starting-run        # no pane yet: a launch from a moment ago

  t "the sweep fires what has come round: a record for the run, the stamp, and a line in the log"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  was_pane=$(declare -f open_pane)
  open_pane() { printf '%s\n' "$2" > "$scratch/rit-title"; printf '%%7\n'; }
  rr_load slack-morning 'schedule: "5 9 * * 1-5"' 'model: haiku' 'allowed_tools: ["Read"]'
  GENSOKYO_NOW=$((mon + 37))   # a sweep lands where it lands inside the minute
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  id=$(ls "$RES_DIR"); rec=$RES_DIR/$id
  rec_load "$rec"
  assert_eq "$R_ritual|$R_name|$R_cwd" "slack-morning|slack-morning|$HOME"
  # The time the resident started, not the minute it is running for: a catch-up run stamped
  # with a minute from last week would be pruned before its pane came up.
  assert_eq "$R_launched" "$((mon + 37))"

  assert_eq "$R_args" "--model haiku --add-dir $STATE_DIR/rituals/slack-morning --allowedTools Read"
  assert_re "$R_prompt_file" "^$STATE_DIR/rituals/slack-morning/prompt\.[0-9a-f-]+$"
  assert_match "$(cat "$R_prompt_file")" 'do the thing'
  assert_eq "$(ritual_stamp slack-morning)" "$mon"
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" 'ran (due 2026-09-07 09:05)'
  assert_match "$(cat "$scratch/rit-title")" 'slack-morning'   # its own name on its own tab

  t "and once for that minute, however often the sweep comes round inside it"
  GENSOKYO_NOW=$((mon + 50))
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1

  t "a missed fire is caught up once, and the run is launched now rather than then"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"
  ritual_stamp_set slack-morning "$(rt_at '2026-09-04 09:05:00')"   # Friday's run, three days back
  GENSOKYO_NOW=$(rt_at '2026-09-07 11:00:00')
  ritual_sweep 1
  rec_load "$RES_DIR/$(ls "$RES_DIR")"
  assert_eq "$R_ritual" slack-morning
  assert_eq "$R_launched" "$(rt_at '2026-09-07 11:00:00')"
  assert_eq "$(ritual_stamp slack-morning)" "$mon"                 # the minute it ran for
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" 'ran (catch-up 2026-09-07 09:05)'
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1               # and only once

  t "a run that is still going is skipped with the reason, and the minute is spent"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"
  rec r-9 slot=1 name=slack-morning ritual=slack-morning pane=%1
  GENSOKYO_NOW=$tue
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1        # nobody new
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" 'skipped (due 2026-09-08 09:05): the last run is still going'
  assert_eq "$(ritual_stamp slack-morning)" "$tue"          # and not tried again this minute

  t "a ritual that is off is left alone, stamp and all"
  fresh; rm -rf "$STATE_DIR/rituals"; rm -f "$mine"/*.md
  rr_load resting 'schedule: "5 9 * * *"' 'enabled: false'
  GENSOKYO_NOW=$mon
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_ok test ! -d "$STATE_DIR/rituals/resting"          # nothing of it is gensokyo's to run yet

  t "a schedule gensokyo cannot read is said once, and the minute is not spent on it"
  rm -f "$mine"/*.md; : > "$scratch/notify.log"
  rr_load garbled 'schedule: "sometimes"'
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_match "$(notifications)" '⏲ garbled: schedule: sometimes is not one gensokyo can read'
  assert_eq "$(ritual_stamp garbled)" ''                    # it has not run, so nothing says it has
  ritual_sweep 1
  assert_eq "$(notifications)" ''                           # and it is not said again
  assert_eq "$(grep -c 'not run' "$STATE_DIR/rituals/garbled/log")" 1

  t "a ritual due this minute with a bad line says which line, and does not run"
  rm -f "$mine"/*.md; : > "$scratch/notify.log"
  printf -- '---\nschedule: "5 9 * * *"\ncwd: %s/nowhere\n---\nx\n' "$scratch" > "$mine/broken.md"
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_match "$(notifications)" 'is not a directory'
  assert_eq "$(ritual_stamp broken)" ''

  t "nothing fires into a quit: the panes are going, and the minute is not spent on it"
  rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"
  rr_load slack-morning 'schedule: "5 9 * * 1-5"'
  : > "$STATE_DIR/quitting"
  ritual_sweep 1
  rm -f "$STATE_DIR/quitting"
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_eq "$(ritual_stamp slack-morning)" ''
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1        # and it fires as soon as the quit is off

  t "the first sight of a ritual with nothing due stamps it rather than running it"
  fresh
  rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"
  rr_load later 'schedule: "0 3 * * *"'
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_eq "$(ritual_stamp later)" "$mon"
  eval "$was_pane"; eval "$was_live"

  t "the news about a run says which ritual finished, not which resident"
  if [ -z "$JQ_BIN" ]; then skip "no jq"; else
    fresh; rm -rf "$STATE_DIR/status"; : > "$scratch/notify.log"
    id=cafecafe-0000-4000-8000-0000000000ff
    rec "$id" slot=1 name=slack-morning cwd=/tmp pane=%9 window=@1 ritual=slack-morning
    hook "$id" Stop ',"permission_mode":"acceptEdits","last_assistant_message":"triaged 4 threads"'
    assert_eq "$(notifications)" 'slack-morning|⏲ slack-morning: done: triaged 4 threads'
    hook "$id" Notification ',"notification_type":"permission_prompt","message":"Claude needs your permission","tool_name":"Bash"'
    assert_eq "$(notifications)" 'slack-morning|⏲ slack-morning: needs your permission (Bash)'
  fi

  # ---------------------------------------------------------------- headless
  t "a headless run: no pane, no record, and everything it did written down three ways"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  : > "$scratch/notify.log"
  GENSOKYO_NOW=$mon
  rr_load quiet-one 'schedule: "5 9 * * *"' 'headless: true' 'model: haiku' 'allowed_tools: ["Read"]'
  rr_headless quiet-one 'due 2026-09-07 09:05'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0     # nobody was summoned for it
  out=$(rr_runlog quiet-one)
  assert_match "$out" 'ritual   quiet-one'
  assert_match "$out" 'started  2026-09-07 09:05  (due 2026-09-07 09:05)'
  assert_match "$out" "in       $HOME"
  assert_match "$out" "notes    $STATE_DIR/rituals/quiet-one/memory.md"
  assert_match "$out" 'stub -p ran in'                   # what the run itself said
  assert_match "$out" 'claude --resume 11111111-2222-3333-4444-555555555555'
  assert_match "$out" '2 turns'
  assert_re "$out" 'done in [0-9]+s, \$0\.01'
  # The journal keeps the first line of it, because the alert is gone in four seconds and this
  # is what somebody reads the next morning.
  assert_re "$(cat "$STATE_DIR/rituals/quiet-one/log")" 'done \(headless, [0-9]+s, \$0\.01\): stub -p ran in'
  assert_match "$(notifications)" 'quiet-one|⏲ quiet-one: done: stub -p ran in'
  # The prompt it was asked, and the flags: a headless run gets the ritual's own, plus the memory
  # file's directory, and none of the pane's hooks, plugin or system paragraph - it has no pane.
  assert_match "$(cat "$STUB_STATE/print.prompt")" 'do the thing'
  assert_match "$(cat "$STUB_STATE/print.prompt")" "$STATE_DIR/rituals/quiet-one/memory.md"
  assert_match "$(cat "$STATE_DIR/rituals/quiet-one/prompt")" 'do the thing'
  out=$(cat "$STUB_STATE/print.args")
  assert_match "$out" '-p --output-format json'
  assert_match "$out" '--disallowed-tools CronCreate CronList CronDelete'
  assert_match "$out" '--model haiku'
  assert_match "$out" "--add-dir $STATE_DIR/rituals/quiet-one"
  assert_match "$out" '--allowedTools Read --'
  assert_nomatch "$out" '--settings'
  assert_nomatch "$out" '--plugin-dir'
  assert_nomatch "$out" '--append-system-prompt'

  t "a run whose tools were refused: it says so, with the line to add, since nobody was asked"
  : > "$scratch/notify.log"
  export STUB_P_DENY=Write
  rr_headless quiet-one 'by hand'
  unset STUB_P_DENY
  assert_match "$(rr_runlog quiet-one)" "refused  Write - a run with nobody to ask needs it in the ritual's allowed_tools"
  assert_match "$(notifications)" 'it needed Write and had nobody to ask'

  t "a run that could not start at all: the log has what claude said, and the news says where"
  : > "$scratch/notify.log"
  export STUB_P_FAIL='no such model'
  assert_fails rr_headless quiet-one 'by hand'
  unset STUB_P_FAIL
  out=$(rr_runlog quiet-one)
  assert_match "$out" 'claude exited 1, and said:'
  assert_match "$out" 'stub-claude: no such model'
  assert_match "$(cat "$STATE_DIR/rituals/quiet-one/log")" 'failed (headless,'
  assert_match "$(notifications)" 'the headless run failed - gensokyo ritual log quiet-one'

  t "a run of a ritual that has gone, and one whose directory has: both say so and neither runs"
  : > "$scratch/notify.log"
  assert_fails rr_headless nothing-of-the-sort 'due 2026-09-07 09:05'
  assert_match "$(notifications)" 'the ritual was gone by the time its run started'
  mkdir -p "$scratch/going"
  { printf -- '---\nschedule: "@daily"\nheadless: true\ncwd: %s\n---\nwork\n' "$scratch/going"; } \
    > "$mine/going.md"
  rmdir "$scratch/going"
  : > "$scratch/notify.log"
  assert_fails rr_headless going 'by hand'
  assert_match "$(notifications)" 'is not there, so the run could not start'
  assert_match "$(rr_runlog going)" 'cwd is not there any more'
  # And a ritual with no cwd line at all, which `cd ""` would have run in whatever directory the
  # clock was started in.
  : > "$scratch/notify.log"
  printf -- '---\nschedule: "@daily"\nheadless: true\n---\nwork\n' > "$mine/nowhere.md"
  assert_fails rr_headless nowhere 'by hand'
  assert_match "$(notifications)" 'is not there, so the run could not start'
  assert_match "$(cat "$STATE_DIR/rituals/nowhere/log")" 'not run: it names no directory is not there'
  rm -f "$mine/going.md" "$mine/nowhere.md"

  t "a headless run in progress has a pid and no pane, and both run and remove refuse it for that"
  fresh; rm -rf "$STATE_DIR/rituals"
  rr_load slow-one 'schedule: "5 9 * * *"' 'headless: true'
  assert_fails ritual_headless_running slow-one
  export STUB_P_SLEEP=2
  assert_ok ritual_launch_headless 'by hand'
  unset STUB_P_SLEEP
  assert_ok ritual_headless_running slow-one     # by its pid: no server was started for this
  assert_ok ritual_busy slow-one
  assert_fails ritual_headless_running quiet-one # and it is that ritual's run, not any run
  out=$(ritual_cmd_run slow-one 2>&1)
  assert_match "$out" 'running headless right now'
  out=$(cmd_ritual remove slow-one 2>&1)
  assert_match "$out" 'running headless right now'
  assert_ok test -f "$mine/slow-one.md"          # and nothing of it was deleted
  assert_ok rr_headless_wait slow-one
  assert_match "$(cat "$STATE_DIR/rituals/slow-one/log")" 'done (headless,'
  # The pid file stays behind on purpose, and a dead pid is not a run.
  assert_ok test -s "$STATE_DIR/rituals/slow-one/headless.pid"
  assert_fails ritual_headless_running slow-one

  t "a long name is still recognised: the whole command line, not as much of it as a terminal fits"
  # `ps` sizes its output to a terminal unless told not to, and a clipped command line would read
  # as "no run in progress" while one was going - which is `overlap: skip` not skipping, and two
  # runs writing one memory file. The name here is at the long end of what a ritual may be called,
  # and the reason text is appended after it, which is what pushes the line past any such limit.
  long=a-very-long-ritual-name-of-the-sort-somebody-would-actually-write
  rr_load "$long" 'schedule: "5 9 * * *"' 'headless: true'
  export STUB_P_SLEEP=2
  assert_ok ritual_launch_headless 'due 2026-09-07 09:05'
  unset STUB_P_SLEEP
  assert_ok ritual_headless_running "$long"
  assert_ok rr_headless_wait "$long"
  rm -f "$mine/$long.md"

  t "a run of a name that is only part of another ritual's runs nothing at all"
  # `_headless` looks the ritual up again in its own process, and find_ritual takes a part of a
  # name when it picks out exactly one ritual. If this ritual's file went in the moment since the
  # fire, the ritual whose name contains it must not run under this name, into these notes.
  fresh; rm -rf "$STATE_DIR/rituals"; rm -f "$mine"/*.md
  : > "$scratch/notify.log"
  rr_load quiet-one-extended 'schedule: "@daily"' 'headless: true'
  assert_eq "$(find_ritual quiet-one)" "$mine/quiet-one-extended.md"   # a partial hit, as ever
  assert_fails rr_headless quiet-one 'due 2026-09-07 09:05'
  assert_match "$(notifications)" 'the ritual was gone by the time its run started'
  assert_fails test -e "$STATE_DIR/rituals/quiet-one-extended/runs"
  rm -f "$mine"/*.md

  t "the sweep fires a headless ritual the same way, and summons nobody to do it"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  rr_load quiet-one 'schedule: "5 9 * * 1-5"' 'headless: true'
  GENSOKYO_NOW=$mon
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_eq "$(ritual_stamp quiet-one)" "$mon"
  assert_match "$(cat "$STATE_DIR/rituals/quiet-one/log")" 'ran (due 2026-09-07 09:05)'
  assert_ok rr_headless_wait quiet-one
  assert_match "$(cat "$STATE_DIR/rituals/quiet-one/log")" 'done (headless,'
  # The minute in the header is the run's own process's clock and not the sweep's, which is why
  # this asks for the reason the sweep handed it rather than for the time beside it.
  assert_match "$(rr_runlog quiet-one)" '(due 2026-09-07 09:05)'

  t "and the run in progress is what the next fire skips: no pane to look for it in"
  rm -f "$mine"/*.md
  export STUB_P_SLEEP=2
  rr_load slow-two 'schedule: "5 9 * * *"' 'headless: true'
  GENSOKYO_NOW=$mon; ritual_sweep 1
  unset STUB_P_SLEEP
  assert_ok ritual_headless_running slow-two
  GENSOKYO_NOW=$((mon + 86400)); ritual_sweep 1
  assert_match "$(cat "$STATE_DIR/rituals/slow-two/log")" 'skipped (due 2026-09-08 09:05): the last run is still going'
  assert_ok rr_headless_wait slow-two

  t "the run logs are kept, but not for ever"
  was_keep=$RITUAL_RUNS_KEEP
  mkdir -p "$STATE_DIR/rituals/quiet-one/runs"
  for i in 1 2 3 4 5; do : > "$STATE_DIR/rituals/quiet-one/runs/2026010$i-000000.1.log"; done
  RITUAL_RUNS_KEEP=3
  rit_runs_trim quiet-one
  assert_eq "$(ls "$STATE_DIR/rituals/quiet-one/runs" | wc -l | tr -d ' ')" 3
  assert_ok test -f "$STATE_DIR/rituals/quiet-one/runs/20260105-000000.1.log"   # the newest kept
  assert_fails test -f "$STATE_DIR/rituals/quiet-one/runs/20260101-000000.1.log"
  RITUAL_RUNS_KEEP=$was_keep
  rit_runs_trim nothing-of-the-sort                                             # and no runs at all is fine

  # ---------------------------------------------------------------- overlap
  t "overlap: the three things to do about a fire that lands while the last run is going"
  rm -f "$mine"/*.md
  for i in skip queue parallel; do
    rr_load over "schedule: \"@daily\"" "overlap: $i"
    assert_eq "$RIT_overlap|$(ritual_problem)" "$i|"
  done
  rr_load over 'schedule: "@daily"' 'overlap: wait'
  assert_match "$(ritual_problem)" 'overlap: wait is not a thing to do about a run that is still going (skip, queue, parallel)'
  rr_load over 'schedule: "@daily"'
  assert_eq "$RIT_overlap" skip

  t "overlap: parallel starts the second run beside the first, and says which run this is"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  was_pane=$(declare -f open_pane); was_live=$(declare -f live_panes)
  open_pane() { printf '%%7\n'; }
  live_panes() { printf '%%1\n'; }
  rr_load slack-morning 'schedule: "5 9 * * 1-5"' 'overlap: parallel'
  rec r-1 slot=1 name=slack-morning ritual=slack-morning pane=%1     # a run still going
  GENSOKYO_NOW=$mon
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 2
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" \
    'ran (due 2026-09-07 09:05), alongside the run that was still going'
  # The second run has a name and a slot of its own: the first one is holding the ritual's name.
  rec_load "$RES_DIR/$(ls "$RES_DIR" | grep -v '^r-1$')"
  assert_eq "$R_slot" 2
  assert_ok test "$R_name" != slack-morning
  # And a prompt file of its own, because the pane reads it later and the first run's pane may
  # not have read its own yet.
  assert_eq "$R_prompt_file" "$STATE_DIR/rituals/slack-morning/prompt.${R_prompt_file##*prompt.}"
  assert_match "$(cat "$R_prompt_file")" 'do the thing'
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/prompt")" 'do the thing'

  t "and a run's prompt goes when its record does, so a run leaves no more behind than it did"
  out=$R_prompt_file
  drop_record "$RES_DIR/$(ls "$RES_DIR" | grep -v '^r-1$')"
  assert_fails test -f "$out"

  t "overlap: queue holds the fire, and it runs when the ritual is free again"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  rr_load slack-morning 'schedule: "5 9 * * 1-5"' 'overlap: queue'
  rec r-1 slot=1 name=slack-morning ritual=slack-morning pane=%1
  GENSOKYO_NOW=$mon
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1                 # nobody new yet
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" \
    'queued (due 2026-09-07 09:05): the last run is still going'
  assert_eq "$(sed -n 1p "$(rit_queue_file slack-morning)")" "$mon"
  assert_eq "$(ritual_stamp slack-morning)" "$mon"                   # the minute is spent either way
  # Still going a minute later: nothing is due, and the fire stays where it is with no second line.
  GENSOKYO_NOW=$((mon + 60))
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  assert_eq "$(grep -c queued "$STATE_DIR/rituals/slack-morning/log")" 1
  # The run finishes, and the next sweep is the queued fire's turn.
  status_write r-1 stopped 'done'
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 2
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" 'ran (queued 2026-09-07 09:05)'
  assert_fails test -f "$(rit_queue_file slack-morning)"
  # And the stamp was left exactly where it was: putting it back to the queued minute would make
  # catch-up count a run that has already happened as one that was missed.
  assert_eq "$(ritual_stamp slack-morning)" "$mon"
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 2                 # and once

  t "overlap: the queue is one fire deep, and the newest of them is the one that runs"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  rr_load every-minute 'schedule: "* * * * *"' 'overlap: queue'
  rec r-1 slot=1 name=every-minute ritual=every-minute pane=%1
  GENSOKYO_NOW=$mon;             ritual_sweep 1
  GENSOKYO_NOW=$((mon + 60));    ritual_sweep 1
  GENSOKYO_NOW=$((mon + 120));   ritual_sweep 1
  assert_eq "$(sed -n 1p "$(rit_queue_file every-minute)")" "$((mon + 120))"
  status_write r-1 stopped 'done'
  GENSOKYO_NOW=$((mon + 150))    # not a minute this schedule names, so nothing is due
  ritual_sweep 1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 2
  assert_match "$(cat "$STATE_DIR/rituals/every-minute/log")" 'ran (queued 2026-09-07 09:07)'

  t "overlap: a fire that waited too long is dropped rather than run hours after its minute"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  rr_load slack-morning 'schedule: "5 9 * * 1-5"' 'overlap: queue'
  rit_queue_set slack-morning "$mon"
  GENSOKYO_NOW=$((mon + RITUAL_QUEUE_LIFE + 60))
  ritual_sweep ''
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  assert_fails test -f "$(rit_queue_file slack-morning)"
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" \
    'dropped the fire queued for 2026-09-07 09:05: the run in its way took over 1h'

  t "overlap: a run by hand is the run a queued fire was waiting for, so it stops waiting"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  rr_load slack-morning 'schedule: "5 9 * * 1-5"' 'overlap: queue'
  rit_queue_set slack-morning "$mon"
  GENSOKYO_NOW=$((mon + 600))
  ritual_cmd_run slack-morning >/dev/null 2>&1
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  assert_fails test -f "$(rit_queue_file slack-morning)"

  t "overlap: a ritual that has gone back to skipping does not drain what it queued"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  rr_load slack-morning 'schedule: "5 9 * * 1-5"'
  rit_queue_set slack-morning "$mon"
  GENSOKYO_NOW=$((mon + 60))
  ritual_sweep ''
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0
  # And the next launch clears it, so nothing stays queued behind a ritual that is not queueing.
  GENSOKYO_NOW=$tue
  ritual_sweep ''
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  assert_fails test -f "$(rit_queue_file slack-morning)"
  eval "$was_pane"; eval "$was_live"

  # ---------------------------------------------------------------- target
  t "target: which way a fire goes is decided once, by the ritual"
  fresh; rm -rf "$STATE_DIR/rituals"; rm -f "$mine"/*.md; : > "$scratch/rr-tmux"
  was_pane=$(declare -f open_pane); was_tmux=$(declare -f tmux_)
  open_pane() { printf '%%7\n'; }
  tmux_() { printf '%s\n' "$*" >> "$scratch/rr-tmux"; return 0; }
  rr_load to-new 'schedule: "@daily"'
  assert_ok ritual_launch 'due 2026-09-07 09:05'
  assert_eq "$(rr_tmux)" ''                                 # a pane of its own, nothing sent
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  fresh; : > "$scratch/rr-tmux"
  rr_load to-reimu 'schedule: "@daily"' 'target: Reimu'
  assert_ok ritual_launch 'due 2026-09-07 09:05'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0        # nobody is summoned for a send
  assert_match "$(rr_tmux)" '_send to-reimu due\ 2026-09-07\ 09:05'
  : > "$scratch/rr-tmux"
  rr_load to-kept 'schedule: "@daily"' 'target: persistent'
  assert_ok ritual_launch 'due 2026-09-07 09:05'
  assert_match "$(rr_tmux)" '_send to-kept'

  t "target: a resident that is not ours gets the prompt and nothing else"
  # It has no --add-dir for the ritual's directory, so the sentence naming the memory file would
  # be a permission prompt every morning - and one ongoing conversation is why somebody named it.
  rr_load to-reimu 'schedule: "@daily"' 'target: Reimu'
  assert_eq "$(ritual_prompt_text)" 'do the thing'
  rr_load to-kept 'schedule: "@daily"' 'target: persistent'
  assert_match "$(ritual_prompt_text)" 'Your notes from previous runs are at'
  rr_load to-new 'schedule: "@daily"'
  assert_match "$(ritual_prompt_text)" 'Your notes from previous runs are at'

  t "target <name>: the prompt is typed into that resident, and the journal says where it went"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  : > "$scratch/notify.log"; rr_cast_none
  was_cast=$(declare -f cast_type); was_ready=$(declare -f rit_wait_ready)
  cast_type() { rr_cast_type "$@"; }
  rit_wait_ready() { return 0; }     # its own test is below; here the resident is ready
  RR_CAST_RC=0
  rr_load to-reimu 'schedule: "@daily"' 'target: Reimu'
  rec r-reimu slot=1 name=Reimu cwd=/tmp/alpha pane=%3
  assert_ok cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_eq "$(rr_cast_pane)" '%3'
  assert_eq "$(rr_cast_text)" 'do the thing'
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'sent to Reimu (due 2026-09-07 09:05)'
  # Nothing is notified: the prompt is sitting in a tab, and that resident's own chip says the rest.
  assert_eq "$(notifications)" ''

  t "target <name>: and every way it can fail to arrive says so, in the journal and out loud"
  rr_cast_none
  fresh
  assert_fails cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_eq "$(rr_cast_text)" ''
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'not sent: there is no resident called Reimu'
  assert_match "$(notifications)" 'Reimu is not here, so the fire was not delivered'
  # A departed screen has no input line, and recalling somebody else's resident is not the
  # ritual's business: the user chose to let it go.
  rec r-reimu slot=1 name=Reimu cwd=/tmp/alpha pane=%3 departed=1757000000
  assert_fails cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_eq "$(rr_cast_text)" ''
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'not sent: Reimu has left'
  assert_match "$(notifications)" 'gensokyo resume Reimu brings it back'
  # A dialog holding that resident open, which is cast_type's own refusal.
  rec r-reimu slot=1 name=Reimu cwd=/tmp/alpha pane=%3
  rit_wait_ready() { printf 'has a dialog waiting for you'; return 1; }
  assert_fails cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_eq "$(rr_cast_text)" ''
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'not sent: Reimu has a dialog waiting for you'
  assert_match "$(notifications)" 'Reimu has a dialog waiting for you, so the fire was not delivered'
  rit_wait_ready() { return 0; }
  # And a paste that went in but never showed up, which is the one cast_type will not press
  # Enter on: an unread prompt in an input line is recoverable and answering a dialog is not.
  RR_CAST_RC=2
  assert_fails cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" "not sent: the prompt never reached Reimu's input line"
  assert_match "$(notifications)" 'never reached'
  RR_CAST_RC=1
  assert_fails cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'not sent: tmux would not type into'
  RR_CAST_RC=0
  # The ritual gone in the moment since the fire, and a name that is only part of another's.
  rm -f "$mine"/*.md
  rr_load to-reimu-extended 'schedule: "@daily"' 'target: Reimu'
  assert_fails cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_match "$(notifications)" 'the ritual was gone by the time its fire was sent'
  # Nothing goes anywhere during a quit: those panes are being told to leave.
  rm -f "$mine"/*.md; rr_load to-reimu 'schedule: "@daily"' 'target: Reimu'
  rr_cast_none; : > "$STATE_DIR/quitting"
  assert_ok cmd__send to-reimu 'due 2026-09-07 09:05'
  assert_eq "$(rr_cast_text)" ''
  rm -f "$STATE_DIR/quitting"

  t "rit_wait_ready waits out a session that is only starting, and reports anything else at once"
  eval "$was_ready"                  # the real one: everything above stands in for it
  was_blocked=$(declare -f cast_blocked)
  cast_blocked() { cat "$scratch/blocked" 2>/dev/null; return 0; }
  was_keep=$RITUAL_SEND_WAIT; RITUAL_SEND_WAIT=0
  : > "$scratch/blocked"
  assert_ok rit_wait_ready some-id
  printf 'has a dialog waiting for you' > "$scratch/blocked"
  assert_eq "$(rit_wait_ready some-id)" 'has a dialog waiting for you'
  assert_fails rit_wait_ready some-id
  printf 'is still starting up' > "$scratch/blocked"
  assert_eq "$(rit_wait_ready some-id)" 'is still starting up'   # waited out, and it never came
  RITUAL_SEND_WAIT=$was_keep
  eval "$was_blocked"
  rit_wait_ready() { return 0; }     # and back to the stand-in for the tests below

  t "target persistent: the first fire starts the session, and that session is the ritual's"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status" "$STATE_DIR/departed"; rm -f "$mine"/*.md
  rr_cast_none; : > "$scratch/notify.log"
  was_live=$(declare -f live_panes)
  live_panes() { printf '%%5\n'; }
  rr_load kept-one 'schedule: "@daily"' 'target: persistent' 'keep: 30m'
  assert_ok cmd__send kept-one 'due 2026-09-07 09:05'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  id=$(ls "$RES_DIR")
  assert_eq "$(sed -n 1p "$(ritual_session_file kept-one)")" "$id"
  # Nothing was typed anywhere: a fresh session is launched with the prompt as its argument.
  assert_eq "$(rr_cast_text)" ''
  rec_load "$RES_DIR/$id"
  assert_match "$(cat "$R_prompt_file")" 'do the thing'
  # And no keep: a persistent ritual's session is the ritual, not one run of it, and a reaper
  # closing its tab two hours after the last answer would undo the one thing it is for.
  assert_eq "$R_keep" ''

  t "target persistent: the next fire is typed into the session it kept"
  rec_set "$RES_DIR/$id" pane %5
  assert_ok cmd__send kept-one 'due 2026-09-08 09:05'
  assert_eq "$(rr_cast_pane)" '%5'
  assert_match "$(rr_cast_text)" 'Your notes from previous runs are at'
  assert_match "$(cat "$STATE_DIR/rituals/kept-one/log")" 'sent to kept-one (due 2026-09-08 09:05)'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1          # and nobody new

  t "target persistent: a session that has left is recalled first, and the fire follows it in"
  rr_cast_none
  was_resume=$(declare -f cmd_resume)
  cmd_resume() { printf 'resumed %s\n' "$1" >> "$scratch/rr-tmux"; rec_del "$RES_DIR/$1" departed; }
  : > "$scratch/rr-tmux"
  rec_set "$RES_DIR/$id" departed 1757000000
  assert_ok cmd__send kept-one 'due 2026-09-09 09:05'
  assert_match "$(rr_tmux)" "resumed $id"
  assert_eq "$(rr_cast_pane)" '%5'
  # Its own cockpit having stopped is the same thing from here: the record is in the departed
  # pile rather than in a pane, and `resume` is the whole of bringing either back.
  rr_cast_none; : > "$scratch/rr-tmux"
  mkdir -p "$DEPARTED_DIR"; cp "$RES_DIR/$id" "$DEPARTED_DIR/$id"; rm -f "$RES_DIR/$id"
  cmd_resume() { printf 'resumed %s\n' "$1" >> "$scratch/rr-tmux"; mv "$DEPARTED_DIR/$1" "$RES_DIR/$1"; }
  assert_ok cmd__send kept-one 'due 2026-09-10 09:05'
  assert_match "$(rr_tmux)" "resumed $id"
  assert_eq "$(rr_cast_pane)" '%5'

  t "target persistent: a recall that cannot happen is said rather than swallowed"
  rr_cast_none; : > "$scratch/notify.log"
  cmd_resume() { die 'no transcript for that session'; }     # `die` from a job with no terminal
  rec_set "$RES_DIR/$id" departed 1757000000
  assert_fails cmd__send kept-one 'due 2026-09-11 09:05'
  assert_eq "$(rr_cast_text)" ''
  assert_match "$(cat "$STATE_DIR/rituals/kept-one/log")" 'not sent: could not recall the session this ritual keeps'
  assert_match "$(notifications)" 'could not recall the session it keeps'
  # And a session nothing is left of at all is a first fire again: the ritual starts a new one.
  rm -f "$RES_DIR/$id"
  assert_ok cmd__send kept-one 'due 2026-09-11 09:05'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  assert_ok test "$(sed -n 1p "$(ritual_session_file kept-one)")" != "$id"
  eval "$was_resume"

  t "the sweep sends a fire like any other, and does not skip one behind a busy session"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"; rm -f "$mine"/*.md
  : > "$scratch/rr-tmux"
  rr_load to-reimu 'schedule: "5 9 * * 1-5"' 'target: Reimu'
  # A record of this ritual's, mid-turn: for a target of `new` this is what `overlap: skip`
  # skips, and here it is somebody else's session and Claude Code's to queue a prompt into.
  rec r-old slot=1 name=Reimu ritual=to-reimu pane=%5
  GENSOKYO_NOW=$mon
  ritual_sweep 1
  assert_match "$(rr_tmux)" '_send to-reimu'
  assert_match "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'ran (due 2026-09-07 09:05)'
  assert_nomatch "$(cat "$STATE_DIR/rituals/to-reimu/log")" 'skipped'
  eval "$was_cast"; eval "$was_ready"; eval "$was_pane"; eval "$was_live"; eval "$was_tmux"
  rm -rf "$STATE_DIR/departed"

  # ---------------------------------------------------------------- keep
  # ---------------------------------------------------------------- keep
  t "keep: the length a finished run's tab stays, and the two words that mean it stays"
  assert_eq "$(rit_keep_secs 45s)" 45
  assert_eq "$(rit_keep_secs 30m)" 1800
  assert_eq "$(rit_keep_secs 2h)" 7200
  assert_eq "$(rit_keep_secs 1d)" 86400
  assert_fails rit_keep_secs forever            # not 0: for ever is not a length, and is not none
  assert_fails rit_keep_secs until_banished
  assert_fails rit_keep_secs 2                  # a number with no unit
  assert_fails rit_keep_secs h
  assert_fails rit_keep_secs 2w                 # a week is not one of them
  assert_fails rit_keep_secs ''
  assert_fails rit_keep_secs '2 h'
  assert_eq "$(rit_keep_secs 999999999d)" 86399999913600   # nine digits still answers
  assert_fails rit_keep_secs 9999999999d                   # ten does not: it would wrap round
  # A length of nothing is a length, and it means what it says: the tab goes as soon as the run
  # has finished. Worth pinning, because it is the one value that is written into a record and
  # still says "now" - an empty keep is the opposite, a tab that stays.
  assert_eq "$(rit_keep_secs 0s)" 0
  assert_eq "$(rit_keep_secs 0m)" 0
  assert_fails rit_keep_secs 0            # still not a length: no unit

  t "the tab's life has a default, it is not for ever, and a keep nobody can read is a complaint"
  rm -f "$mine"/*.md
  rr_load kept 'schedule: "@daily"'
  assert_eq "$RIT_keep" 2h
  assert_eq "$(ritual_problem)" ''
  rr_load kept 'schedule: "@daily"' 'keep: forever'
  assert_eq "$RIT_keep|$(ritual_problem)" 'forever|'
  rr_load kept 'schedule: "@daily"' 'keep: until_banished'
  assert_eq "$(ritual_problem)" ''
  rr_load kept 'schedule: "@daily"' 'keep: 2 hours'
  assert_match "$(ritual_problem)" 'keep: 2 hours is not a length (30m, 2h, 1d, forever, until_banished)'

  t "the launch writes the tab's life into the record, in seconds, and leaves it out for forever"
  fresh; rm -rf "$STATE_DIR/rituals"
  was_pane=$(declare -f open_pane)
  open_pane() { printf '%%1\n'; }
  rr_load kept 'schedule: "@daily"' 'keep: 30m'
  assert_ok ritual_launch_pane
  rec_load "$RES_DIR/$(ls "$RES_DIR")"
  assert_eq "$R_keep" 1800
  fresh
  rr_load kept 'schedule: "@daily"' 'keep: forever'
  assert_ok ritual_launch_pane
  rec_load "$RES_DIR/$(ls "$RES_DIR")"
  assert_eq "$R_keep" ''
  eval "$was_pane"

  t "keep: a run that has finished and sat there long enough is handed to a job of the server's"
  fresh; rm -rf "$STATE_DIR/status" "$STATE_DIR/departed" "$STATE_DIR/rituals"
  was_live=$(declare -f live_panes); was_tmux=$(declare -f tmux_)
  live_panes() { printf '%%1\n%%2\n%%3\n'; }
  tmux_() { printf '%s\n' "$*" >> "$scratch/rr-tmux"; return 0; }
  : > "$scratch/rr-tmux"
  real_now=$(date +%s)
  rec r-keep slot=1 name=Cirno ritual=slack-morning pane=%1 keep=1800
  rr_since r-keep stopped "$real_now"
  ritual_reap
  assert_eq "$(rr_tmux)" ''                          # it has only just finished
  rr_since r-keep stopped "$((real_now - 1801))"
  ritual_reap
  assert_match "$(rr_tmux)" 'run-shell -b'
  assert_match "$(rr_tmux)" '_reap r-keep'
  assert_ok test -f "$RES_DIR/r-keep"                # the job does the taking, not the clock

  t "and what is not a finished ritual run is left where it is"
  : > "$scratch/rr-tmux"
  fresh
  # Nobody's to reap: a resident somebody summoned by hand, whatever the rituals say. The keep
  # in this record is what proves which check does the work - nothing writes one into a hand
  # summon, and if that ever changed the `ritual` line would still be what says whose run a tab is.
  rec r-hand slot=1 name=Marisa pane=%1 keep=60
  rr_since r-hand stopped "$((real_now - 99999))"
  # keep: forever, which is a record with no keep in it at all.
  rec r-ever slot=2 name=Reimu ritual=nightly-checks pane=%2
  rr_since r-ever stopped "$((real_now - 99999))"
  ritual_reap
  assert_eq "$(rr_tmux)" ''
  # Still going, and stuck waiting for somebody: neither is a run that has finished, and the
  # second is the one thing keep cannot bound - a tab nobody answered stays for good.
  fresh
  rec r-turn slot=1 name=Cirno ritual=slack-morning pane=%1 keep=60
  rr_since r-turn '' "$((real_now - 99999))"
  ritual_reap
  assert_eq "$(rr_tmux)" ''
  rr_since r-turn awaits "$((real_now - 99999))"
  ritual_reap
  assert_eq "$(rr_tmux)" ''
  # And a run with no status file at all: its hooks have never fired, which is what a session
  # stalled on Claude Code's workspace-trust dialog looks like. Another tab keep cannot bound.
  rm -f "$STATE_DIR/status/r-turn"
  ritual_reap
  assert_eq "$(rr_tmux)" ''
  # A pane that went behind our back, and a launch that has not got one yet.
  rec r-lost slot=2 name=Sakuya ritual=slack-morning pane=%9 keep=60
  rr_since r-lost stopped "$((real_now - 99999))"
  rec r-new slot=3 name=Youmu ritual=slack-morning keep=60
  ritual_reap
  assert_eq "$(rr_tmux)" ''
  # And the other end of the scale: keep: 0s is a tab that goes as soon as the run is done.
  rec r-now slot=4 name=Youmu ritual=slack-morning pane=%2 keep=0
  rr_since r-now stopped "$real_now"
  ritual_reap
  assert_match "$(rr_tmux)" '_reap r-now'
  : > "$scratch/rr-tmux"
  # And nothing at all during a quit: those panes are going, and `quit` is watching these very
  # records for the departure it asked for.
  fresh
  rec r-keep slot=1 name=Cirno ritual=slack-morning pane=%1 keep=60
  rr_since r-keep stopped "$((real_now - 99999))"
  : > "$STATE_DIR/quitting"
  ritual_reap
  assert_eq "$(rr_tmux)" ''
  rm -f "$STATE_DIR/quitting"

  t "keep: a run that has already left has only its tab left, and that goes with no job at all"
  fresh; rm -rf "$STATE_DIR/departed" "$STATE_DIR/rituals"; : > "$scratch/rr-tmux"
  rec r-gone slot=1 name=Cirno ritual=slack-morning pane=%1 keep=60 departed="$((real_now - 200))"
  ritual_reap
  assert_match "$(rr_tmux)" 'kill-pane -t %1'
  assert_fails test -f "$RES_DIR/r-gone"
  # Archived and not deleted: nobody was there to decide the transcript was finished with, so it
  # is still in the recall list afterwards.
  assert_ok test -f "$STATE_DIR/departed/r-gone"
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" "closed Cirno's tab, 1m after it left (keep)"
  # Not yet, though, for one that left a moment ago.
  fresh; : > "$scratch/rr-tmux"
  rec r-gone2 slot=1 name=Cirno ritual=slack-morning pane=%1 keep=600 departed="$((real_now - 10))"
  ritual_reap
  assert_eq "$(rr_tmux)" ''

  t "the job asks the resident to leave, and takes the tab once it has"
  fresh; rm -rf "$STATE_DIR/departed" "$STATE_DIR/rituals" "$STATE_DIR/status"; : > "$scratch/rr-tmux"
  was_ask=$(declare -f ask_leave)
  ask_leave() { printf 'asked %s\n' "$1" >> "$scratch/rr-tmux"; rec_set "$RES_DIR/$2" departed "$(date +%s)"; }
  rec r-keep slot=1 name=Cirno ritual=slack-morning pane=%1 keep=1800
  rr_since r-keep stopped "$((real_now - 1801))"
  assert_ok cmd__reap r-keep
  assert_match "$(rr_tmux)" 'asked %1'
  assert_match "$(rr_tmux)" 'kill-pane -t %1'
  assert_ok test -f "$STATE_DIR/departed/r-keep"
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" "closed Cirno's tab, idle 30m since the run finished (keep)"

  t "a resident that will not answer /exit keeps its tab, and the journal says so"
  fresh; rm -rf "$STATE_DIR/departed" "$STATE_DIR/rituals"; : > "$scratch/rr-tmux"
  ask_leave() { printf 'asked %s\n' "$1" >> "$scratch/rr-tmux"; return 1; }
  rec r-keep slot=1 name=Cirno ritual=slack-morning pane=%1 keep=1800
  rr_since r-keep stopped "$((real_now - 1801))"
  assert_fails cmd__reap r-keep
  assert_eq "$(grep -c 'asked %1' "$scratch/rr-tmux")" 2            # asked twice, as close does
  assert_nomatch "$(rr_tmux)" 'kill-pane'
  assert_ok test -f "$RES_DIR/r-keep"
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" "Cirno did not answer /exit, so its tab stays (keep 30m)"
  # And the clock does not come back in twenty seconds with another /exit for a resident sitting
  # right there: giving up takes the keep out of the record, which is what the journal promised.
  assert_eq "$(rec_get "$RES_DIR/r-keep" keep)" ''
  : > "$scratch/rr-tmux"
  ritual_reap
  assert_eq "$(rr_tmux)" ''

  t "and a resident given something to do since the clock looked is not a finished run any more"
  : > "$scratch/rr-tmux"
  rr_since r-keep '' "$((real_now - 1801))"
  assert_ok cmd__reap r-keep
  assert_eq "$(rr_tmux)" ''                                          # nothing asked, nothing taken
  rr_since r-keep stopped "$real_now"                                # finished, but only just
  assert_ok cmd__reap r-keep
  assert_eq "$(rr_tmux)" ''
  rr_since r-keep stopped "$((real_now - 1801))"
  : > "$STATE_DIR/quitting"
  assert_ok cmd__reap r-keep
  assert_eq "$(rr_tmux)" ''
  rm -f "$STATE_DIR/quitting"
  assert_ok cmd__reap nobody-of-that-name                             # a record that has gone
  eval "$was_ask"; eval "$was_live"; eval "$was_tmux"
  rm -rf "$STATE_DIR/departed" "$STATE_DIR/status"

  rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"
  CFG_PERMISSION_MODE=$was_mode
  GENSOKYO_NOW=$was_now
  fresh
}
