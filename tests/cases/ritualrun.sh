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

ritual_run_tests() {
  local mine=$CONFIG_DIR/rituals mon tue out rec was_pane was_live was_now was_mode id
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
  assert_eq "$(rr_argc "$out")" 11
  assert_eq "$(rr_argv 11 "$out")" 'Bash(npm run test:*)'
  rr_load flags 'schedule: "@daily"'
  assert_eq "$(ritual_args)" ''
  CFG_PERMISSION_MODE=acceptEdits
  assert_eq "$(ritual_args)" '--permission-mode acceptEdits'   # a run is a resident: same default
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

  assert_eq "$R_args" '--model haiku --allowedTools Read'
  assert_eq "$R_prompt_file" "$STATE_DIR/rituals/slack-morning/prompt"
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

  rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"
  CFG_PERMISSION_MODE=$was_mode
  GENSOKYO_NOW=$was_now
  fresh
}
