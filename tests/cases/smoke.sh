# tests/cases/smoke.sh - a real tmux server on its own socket, stub claude in the panes.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files
# A real tmux server on its own socket, stub claude in the panes.
G=$root/bin/gensokyo
tm() { "$TMUX_BIN" -L "$GENSOKYO_SOCKET" "$@"; }
# attach_headless: an attached client without a terminal, through script(1)'s pty (BSD and
# util-linux spellings); it lives until detach-client or kill-server. Its stdin is a pipe that
# stays open for a minute: script forwards an EOF on stdin as Ctrl-D into the pty, and the
# client would type that into the active pane (the stub exits on it, Claude Code asks twice).
# The whole subshell is redirected, or that minute of `sleep` would hold this script's own
# stdout open and anything piping the run (`tests/run.sh | tail`) would wait for it.
attach_headless() {
  if script --version >/dev/null 2>&1; then
    (sleep 60 | script -q -c "$TMUX_BIN -L $GENSOKYO_SOCKET attach -t =gensokyo" /dev/null >/dev/null 2>&1 &) >/dev/null 2>&1
  else
    (sleep 60 | script -q /dev/null "$TMUX_BIN" -L "$GENSOKYO_SOCKET" attach -t =gensokyo >/dev/null 2>&1 &) >/dev/null 2>&1
  fi
  pause 1
}
# attach_cc: the same pty trick in control mode - iTerm2's kind of client, which -CC will not
# give to a process with no terminal at all. Its bar is tmux's own status-left/right rather than
# the rows gensokyo draws, which is the half of a reload that the plain client cannot exercise.
attach_cc() {
  if script --version >/dev/null 2>&1; then
    (sleep 60 | script -q -c "$TMUX_BIN -L $GENSOKYO_SOCKET -CC attach -t =gensokyo" /dev/null >/dev/null 2>&1 &) >/dev/null 2>&1
  else
    (sleep 60 | script -q /dev/null "$TMUX_BIN" -L "$GENSOKYO_SOCKET" -CC attach -t =gensokyo >/dev/null 2>&1 &) >/dev/null 2>&1
  fi
  pause 1.5
}
cc_client() { tm list-clients -F '#{client_name} #{client_flags}' 2>/dev/null | awk '/control-mode/ { print $1; exit }'; }
# The shrine's pane, found the way gensokyo finds it: the loop marks it with @shrine. Its
# window is not the session's current one once a resident has been focused.
shrine_pane() { tm list-panes -s -t =gensokyo -F '#{pane_id} #{@shrine}' 2>/dev/null | awk '$2 == 1 { print $1 }'; }
# capture it by that pane id only: an empty -t would read whichever pane is current instead,
# and report a resident's screen as the shrine's.
shrine_capture() { local p; p=$(shrine_pane); [ -n "$p" ] && tm capture-pane -p -t "$p" 2>/dev/null; return 0; }
# pane_shows <pane> <text>: what the pane holds, once it holds that text. A pane is driven by
# keystrokes and by a stub that has to start, so a fixed pause is a race that a busy machine
# loses; this waits for the state the assertion is about and then reads it once. The budget is
# generous because it costs nothing when the pane is ready - and a budget that is merely enough
# is the shape of every flake this suite has had.
pane_shows() {
  local out n=0
  while [ "$n" -lt 40 ]; do
    out=$(tm capture-pane -p -t "$1" 2>/dev/null)
    case $out in *"$2"*) break ;; esac
    pause 0.3; n=$((n + 1))
  done
  printf '%s' "$out"
}
# shrine_click <text> [column]: click the shrine line that holds that text, the way a mouse does -
# the SGR release report the shrine acts on, at that row and column. Column 3 is inside every
# list line, whose whole width is one target; a button wants the column its text starts at.
shrine_click() {
  local p out row line col
  p=$(shrine_pane); [ -n "$p" ] || { warn "no shrine pane"; return 1; }
  out=$(shrine_capture)
  row=$(printf '%s\n' "$out" | grep -n -F -- "$1" | head -n 1 | cut -d: -f1)
  [ -n "$row" ] || { warn "the shrine has no line holding: $1"; return 1; }
  line=$(printf '%s\n' "$out" | sed -n "${row}p")
  col=${2:-$(awk -v l="$line" -v s="$1" 'BEGIN { print index(l, s) + 1 }')}
  tm send-keys -t "$p" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$col" "$row" "$col" "$row")"
}
# cast_landed <pane> <text>: what a resident's pane holds after a card has been typed into it,
# once <text> - the stub's echo of the card's last line - has arrived. A card is many lines
# long and the pane shows both what was typed and what the resident echoed back, so twice its
# height: by the time the end of it is there the beginning has scrolled off the visible screen.
# The assertion is therefore about the scrollback (-S -), which is readable because the stub is
# an ordinary program rather than a full-screen one, with -J putting each wrapped line back
# together so a path that wraps is still one string.
cast_landed() {
  pane_shows "$1" "$2" >/dev/null
  tm capture-pane -p -J -S - -t "$1" 2>/dev/null
}
# shrine_key <character>: the letter or digit that does the same as clicking.
shrine_key() { local p; p=$(shrine_pane); [ -n "$p" ] && tm send-keys -t "$p" -l "$1"; }
# shrine_type <text>: an answer to the question the shrine is holding.
shrine_type() { local p; p=$(shrine_pane); tm send-keys -t "$p" -l "$1" \; send-keys -t "$p" Enter; }
# Which tab is at the front: with one pane per window, the pane the session is showing.
pane_current() { tm display -p -t '=gensokyo:' '#{pane_id}' 2>/dev/null; }
# pane_fronts <pane>: that pane, once the session is showing it. A click travels through the
# terminal and a loop that is not necessarily reading at that moment, so this waits for it.
pane_fronts() {
  local i=0
  while [ "$i" -lt 50 ]; do
    [ "$(pane_current)" = "$1" ] && break
    i=$((i + 1)); pause 0.2
  done
  pane_current
}
# shrine_shows <text>: the shrine redraws on its own clock, so give it a tick to catch up.
shrine_shows() {
  local out n=0
  while [ "$n" -lt 24 ]; do
    out=$(shrine_capture)
    case $out in *"$1"*) break ;; esac
    pause 0.5; n=$((n + 1))
  done
  printf '%s' "$out"
}
# cleanup: the server goes, then the socket file it left behind, then the scratch directory.
# tmux unlinks its socket neither on kill-server nor when its last session exits (measured), so a
# suite that does not remove its own leaves one file per run in /tmp/tmux-<uid> for ever - a
# couple of hundred of them by the time anyone looked. The server is killed before the file goes
# and not after: a server whose socket has been removed under it is alive and unreachable, with
# every stub in its panes still running.
#
# Its own answer for where that socket is while there is a server to ask, and tmux's own rule for
# it when there is not (TMUX_TMPDIR, else /tmp, then tmux-<uid>/<name>) - a guessed path that is
# wrong would leak in silence, which is the thing being fixed.
cleanup() {
  local sock=''
  if [ -n "${TMUX_BIN:-}" ]; then
    sock=$(tm display -p '#{socket_path}' 2>/dev/null)
    tm kill-server 2>/dev/null
  fi
  [ -n "$sock" ] || [ -z "${GENSOKYO_SOCKET:-}" ] ||
    sock=${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$GENSOKYO_SOCKET
  [ -n "$sock" ] && rm -f "$sock"
  rm -rf "$scratch"
}
trap cleanup EXIT

smoke_tests() {
  local out pane1 pane2 pane id1 id2 id3 id4 id5 name3 args cirno row col gen spane spid
  t "smoke: prerequisites"
  if [ -z "$TMUX_BIN" ] || [ -z "$JQ_BIN" ]; then skip "no tmux/jq (run scripts/vendor.sh)"; return; fi
  ok
  mkdir -p "$scratch/work/alpha" "$scratch/work/beta"
  fresh; rm -f "$REGISTRY"

  t "smoke: --detach starts the server with the shrine in its own window and an empty bar"
  out=$(cd "$scratch/work" && : > a && "$G" --detach 2>&1)   # a one-letter file: `?` must not glob
  assert_match "$out" 'running detached'
  assert_ok tm has-session -t =gensokyo
  assert_eq "$(tm list-windows -t =gensokyo -F '#{window_name}')" '⛩ gensokyo'
  assert_eq "$(tm show -gv set-titles-string)" '#W'   # else every tab reads the same
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

  t "smoke: summon two residents; a window each, records, launch flags"
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
  assert_re "$(rec_get "$RES_DIR/$id1" window)" '^@[0-9]+$'
  # One window per resident, one pane in each, plus the shrine's; the tab title is the
  # resident's short form, which is what iTerm2 puts in the tab bar.
  assert_eq "$(tm list-windows -t =gensokyo -F '#{window_id}' | wc -l | tr -d ' ')" 3
  assert_eq "$(tm list-panes -s -t =gensokyo -F x | wc -l | tr -d ' ')" 3
  assert_eq "$(tm display -p -t "$pane1" '#{window_panes}')" 1
  # The titles, not their order: the user reorders tabs, and iTerm2 reorders tmux's window
  # indexes to match, so the order in this list is theirs and not ours to assert.
  out=$(tm list-windows -t =gensokyo -F '#{window_name}')
  assert_match "$out" '⛩ gensokyo'
  assert_match "$out" '1 ○ Alpha'
  assert_match "$out" '2 ○ Beta'
  args=$(cat "$STUB_STATE/$id2.args")
  assert_match "$args" "--session-id $id2 --name Beta"
  assert_match "$args" "--plugin-dir $root/share/plugin"
  # All three cron tools share one per-session in-memory store: a schedule built on CronCreate dies
  # with the pane without ever firing, and with CronCreate refused CronList can only ever answer
  # "no scheduled jobs" however many rituals are on disk - which one resident duly told its owner.
  # The `schedule` skill itself is not refused: its cloud routines are the only schedule here that
  # runs while the cockpit is down.
  assert_match "$args" '--disallowed-tools CronCreate CronList CronDelete'
  assert_match "$args" '--append-system-prompt'
  assert_match "$args" '--settings {"hooks":{"UserPromptSubmit"'
  assert_match "$args" '--model haiku --permission-mode plan'
  assert_eq "$(rec_get "$RES_DIR/$id2" mode)" plan
  assert_nomatch "$(cat "$STUB_STATE/$id1.args")" '--model'
  assert_match "$(tm capture-pane -p -t "$pane1")" 'env CLAUDE*: 0'
  # The skills print `gensokyo ritual add` and `gensokyo ritual run` for the user to read, so bare
  # `gensokyo` has to be the copy that launched the resident - not missing, which sends a resident
  # hunting the filesystem for it, and not somebody else's copy either.
  assert_match "$(tm capture-pane -p -t "$pane1")" "gensokyo on PATH: $root/bin/gensokyo"

  t "smoke: the shrine tab draws a block per resident and the buttons under them"
  assert_re "$(shrine_pane)" '^%[0-9]+$'
  out=$(shrine_shows '2 ○ Beta')
  assert_re "$out" '^  1 ○ Alpha +alpha'
  assert_re "$out" '^  2 ○ Beta +beta'
  assert_match "$out" '[ summon n ]  [ banish x ]  [ recall r ]  [ cast s ]  [ timetable t ]  [ reload l ]  [ quit q ]  [ ? ]'
  assert_match "$out" 'click a resident above to open its tab'
  assert_nomatch "$out" 'Nobody is here yet'

  t "smoke: list and list --json see both, idle"
  rm -f "$REGISTRY"
  out=$("$G" list)
  assert_re "$out" '^  1   ○  Alpha .*alpha .*idle$'
  assert_re "$out" '^  2   ○  Beta .*beta .*idle$'
  out=$("$G" list --json)
  assert_eq "$(printf '%s' "$out" | jq_ -r 'map(.name) | join(",")')" 'Alpha,Beta'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[1] | "\(.slot) \(.status) \(.window) \(.outside)"')" "2 idle $(rec_get "$RES_DIR/$id2" window) false"

  t "smoke: bar chips, bold for the active pane, waiting highlight and count"
  rm -f "$REGISTRY"
  out=$("$G" _bar 1 "$pane1")
  assert_match "$out" '#[bold] 1 ○ Alpha #[default]│#[default] 2 ○ Beta #[default]'
  echo waiting > "$STUB_STATE/$id2.status"; rm -f "$REGISTRY"
  out=$("$G" _bar 1 "$pane1")
  assert_match "$out" "#[bg=$CFG_COLOR_AWAIT,fg=black] 2 ✦ Beta "
  assert_match "$out" '#[align=right]#[bg='"$CFG_COLOR_AWAIT"',fg=black] ✦ 1 '

  t "smoke: the same chips in plain text, where the one who needs you comes first instead"
  assert_eq "$("$G" _bar 1 "$pane1" plain)" ' 2 ✦ Beta │ 1 ○ Alpha   ✦ 1 '
  rm -f "$STUB_STATE/$id2.status"; rm -f "$REGISTRY"
  assert_eq "$("$G" _bar 1 "$pane1" plain)" ' 1 ○ Alpha │ 2 ○ Beta '
  assert_match "$("$G" _border "$pane1" '✳ Alpha')" ' 1 Alpha · alpha '

  t "smoke: a Stop hook turns the chip gold, rings the bell and alerts the desktop; typing clears it"
  : > "$scratch/notify.log"
  payload "$id1" Stop ',"permission_mode":"acceptEdits","last_assistant_message":"done here"' | TMUX_PANE=$pane1 "$G" _hook
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane2")" "#[bg=$CFG_COLOR_AWAIT,fg=black] 1 ✦ Alpha "
  assert_eq "$(tm display -p -t "$pane1" '#{window_bell_flag}')" 1
  assert_eq "$(notifications)" 'Alpha|Alpha is done: done here'
  assert_re "$("$G" list)" '^  1   ✦  Alpha .*waiting \(done here\)$'
  assert_eq "$("$G" list --json | jq_ -r '.[0] | "\(.status) \(.permission_mode) \(.detail)"')" 'waiting acceptEdits done here'
  assert_eq "$("$G" list --json | jq_ -r '.[1] | "\(.status) \(.permission_mode) \(.detail)"')" 'idle plan null'
  payload "$id1" UserPromptSubmit ',"permission_mode":"acceptEdits"' | "$G" _hook
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane2")" '#[default] 1 ○ Alpha '

  t "smoke: no desktop alert for the pane on screen in a client that was just used"
  tm select-window -t "$pane1"
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

  t "smoke: the clock pushes the chips into status-left, where iTerm2 reads them"
  attach_headless
  if [ "$(tm list-clients 2>/dev/null | wc -l | tr -d ' ')" != 1 ]; then skip "could not attach a headless client (script(1))"; else
    out=''
    for _ in 1 2 3 4 5 6 7 8; do
      out=$(tm show -gv status-left)
      case $out in *'1 ○ Alpha'*) break ;; esac
      pause 0.5
    done
    assert_match "$out" ' 1 ○ Alpha Sonnet 5%% '   # the percent doubled, so tmux hands iTerm2 one
    assert_nomatch "$out" '#['                     # one style would blank the whole bar
    assert_match "$(tm show -gv status-right)" '36%% ↻'
    assert_match "$("$G" doctor)" 'clock      ticking'

    t "smoke: a hook moves the chip within a second, without waiting for the next tick"
    payload "$id1" Stop ',"last_assistant_message":"ping"' | "$G" _hook
    for _ in 1 2 3 4; do
      out=$(tm show -gv status-left)
      case $out in *'✦ Alpha'*) break ;; esac
      pause 0.25
    done
    assert_match "$out" ' 1 ✦ Alpha'
    assert_match "$out" '  ✦ 1 '

    t "smoke: and the tab title with it, so the tab bar says who needs you"
    assert_eq "$(tm display -p -t "$pane1" '#{window_name}')" '1 ✦ Alpha'
    payload "$id1" UserPromptSubmit '' | "$G" _hook
    for _ in 1 2 3 4; do
      out=$(tm display -p -t "$pane1" '#{window_name}')
      [ "$out" = '1 ○ Alpha' ] && break
      pause 0.25
    done
    assert_eq "$out" '1 ○ Alpha'

    t "smoke: a hook wakes the shrine too, so its blocks do not wait for the next tick"
    payload "$id1" Stop ',"last_assistant_message":"ping"' | "$G" _hook
    pause 0.4
    assert_match "$(shrine_capture)" '  1 ✦ Alpha'
    payload "$id1" UserPromptSubmit '' | "$G" _hook
    tm detach-client; pause 0.3
  fi

  t "smoke: reload runs the code on disk again - the keys, the clock and the shrine loop, not the residents"
  gen=$(cat "$STATE_DIR/clock.gen")
  spane=$(shrine_pane); spid=$(tm display -p -t "$spane" '#{pane_pid}')
  tm unbind -T gensokyo n              # a key lost by hand: reload builds the table again
  assert_nomatch "$(tm list-keys -T gensokyo)" '_menu-summon'
  out=$("$G" reload)
  assert_match "$out" 'reloaded'
  assert_re "$(tm list-keys -T gensokyo)" '-T gensokyo +n +run-shell .*_menu-summon'
  assert_re "$(tm list-keys -T prefix)" '-T prefix +l +run-shell .*_reload'
  # The shrine keeps its pane and its window - that tab must never move - and runs a new process
  # in it, which draws the same frame again.
  assert_eq "$(shrine_pane)" "$spane"
  assert_ok test "$(tm display -p -t "$spane" '#{pane_pid}')" != "$spid"
  assert_match "$(shrine_shows '1 ○ Alpha')" '[ summon n ]'
  # A clock of the new code took over, and the loop of the old one stopped: the beat carries the
  # generation of whoever wrote it, so a beat that stays behind the current generation is a loop
  # that outlived its reload.
  assert_ok test "$(cat "$STATE_DIR/clock.gen")" != "$gen"
  wait_for 8 '[ "$(cat "$STATE_DIR/clock")" = "$(cat "$STATE_DIR/clock.gen")" ]'
  assert_eq "$(cat "$STATE_DIR/clock")" "$(cat "$STATE_DIR/clock.gen")"
  assert_match "$("$G" doctor)" 'clock      ticking'
  # A beat from anywhere else is a second loop writing it, which is what a cockpit older than the
  # generations leaves behind: nothing can wave that one off, so it is said out loud instead.
  printf 'older\n' > "$STATE_DIR/clock"
  wait_for 10 '[ -f "$STATE_DIR/clock.stray" ]'
  assert_match "$("$G" doctor)" 'a second clock is ticking as well'
  rm -f "$STATE_DIR/clock.stray"
  # share/tmux.conf blanks status-left on its way through a reload, so push_bar has to come after
  # it: otherwise the bar stays empty until somebody attaches again.
  assert_ok test -n "$(tm show -gv status-left)"
  assert_eq "$(tm show -gv status)" 2   # the plain client's two rows

  t "smoke: and with iTerm2's kind of client attached it sets that bar up instead, and fills it"
  attach_cc
  if [ -z "$(cc_client)" ]; then skip "could not attach a control-mode client (script(1))"; else
    out=$("$G" reload); assert_match "$out" 'reloaded'
    assert_eq "$(tm show -gv status)" on
    assert_nomatch "$(tm show -g status-format)" '_bar 1'   # iTerm2 shows nothing under an override
    assert_ok test -n "$(tm show -gv status-left)"
    tm detach-client -t "$(cc_client)"
    wait_for 5 '[ -z "$(cc_client)" ]'
    out=$("$G" reload)                  # back to the two rows the tests after this one read
    assert_eq "$(tm show -gv status)" 2
  fi

  t "smoke: reload leaves the residents and the shrine's own pane where they were"
  # Nobody was disturbed: the same residents, the same panes, the same windows.
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 3
  assert_match "$(tm capture-pane -p -t "$pane1")" 'env CLAUDE*: 0'
  assert_eq "$(rec_get "$RES_DIR/$id1" pane)" "$pane1"

  t "smoke: and the shrine's own reload button, which has to survive respawning the pane it is in"
  spid=$(tm display -p -t "$spane" '#{pane_pid}')
  assert_ok shrine_click '[ reload l ]'
  wait_for 10 '[ "$(tm display -p -t "$spane" "#{pane_pid}")" != "$spid" ]'
  assert_ok test "$(tm display -p -t "$spane" '#{pane_pid}')" != "$spid"
  assert_match "$(shrine_shows '1 ○ Alpha')" '[ reload l ]'

  t "smoke: clicking a resident's line in the shrine brings its window to the front"
  assert_match "$(shrine_shows '2 ○ Beta')" '2 ○ Beta'
  shrine_click '2 ○ Beta' 3
  assert_eq "$(pane_fronts "$pane2")" "$pane2"
  shrine_click '1 ○ Alpha' 3
  assert_eq "$(pane_fronts "$pane1")" "$pane1"

  t "smoke: so does the slot number the line carries, and a click on a blank row does nothing"
  shrine_key 2
  assert_eq "$(pane_fronts "$pane2")" "$pane2"
  shrine_click 'click a resident above' 3    # a row with nothing on it to click
  pause 0.6
  assert_eq "$(pane_current)" "$pane2"
  "$G" _focus 1 "$(tm list-clients -F '#{client_name}' | head -n 1)" >/dev/null 2>&1   # the prefix-1 key
  assert_eq "$(pane_fronts "$pane1")" "$pane1"

  t "smoke: casting a card types it into every resident and submits it, and takes the gold off"
  payload "$id1" Stop ',"permission_mode":"acceptEdits","last_assistant_message":"idle again"' | TMUX_PANE=$pane1 "$G" _hook
  rm -f "$REGISTRY"
  assert_re "$("$G" list)" '^  1   ✦  Alpha'
  out=$("$G" broadcast status-report all 2>&1)
  assert_match "$out" 'cast Spirit Sign "Status Report" on 2 residents'
  assert_match "$(cast_landed "$pane1" '> were mid-task')" '> Stop and tell me where you are'
  assert_match "$(cast_landed "$pane2" '> were mid-task')" '> Stop and tell me where you are'
  rm -f "$REGISTRY"
  assert_re "$("$G" list)" '^  1   ○  Alpha'   # gensokyo typed for the user, so nothing is pending

  t "smoke: a resident holding a dialog is reported, never typed into - the card would answer it"
  echo waiting > "$STUB_STATE/$id2.status"; rm -f "$REGISTRY"
  out=$("$G" broadcast status-report all 2>&1)
  assert_match "$out" 'Beta has a dialog waiting for you; left out'
  assert_match "$out" 'cast Spirit Sign "Status Report" on Alpha'
  out=$("$G" broadcast status-report Beta 2>&1)
  assert_match "$out" 'Beta has a dialog waiting for you; not cast at'
  assert_fails "$G" broadcast status-report Beta
  rm -f "$STUB_STATE/$id2.status"; rm -f "$REGISTRY"

  t "smoke: a pair card goes to one resident, with {peer}, {self} and {cwd} filled in"
  out=$("$G" broadcast second-opinion Alpha --with Beta 2>&1)
  assert_match "$out" 'cast Review Sign "Second Opinion" on Alpha, with Beta as peer'
  out=$(cast_landed "$pane1" '> that instead of messaging it again')
  assert_match "$out" '> Ask Beta for a review of the uncommitted changes in'
  assert_match "$out" "$scratch/work/alpha"
  assert_match "$out" 'addressed to Alpha'
  assert_nomatch "$out" '{peer}'
  assert_nomatch "$(tm capture-pane -p -J -S - -t "$pane2" 2>/dev/null)" 'Ask Beta for a review'

  t "smoke: {residents} is everyone else, per resident, and idle picks out who is resting"
  out=$("$G" broadcast sync-up idle 2>&1)
  assert_match "$out" 'cast Border Sign "Sync Up" on 2 residents'
  assert_match "$(cast_landed "$pane1" '> anybody')" '> The other residents on this machine are: Beta.'
  assert_match "$(cast_landed "$pane2" '> anybody')" '> The other residents on this machine are: Alpha.'

  t "smoke: and the cast button does the same three steps by mouse"
  shrine_key s
  assert_match "$(shrine_shows 'cast which spell card?')" 'Spirit Sign "Status Report"'
  assert_ok shrine_click 'Time Sign "Wrap Up"' 3
  assert_match "$(shrine_shows 'on?')" 'everyone ('
  assert_ok shrine_click '1 ○ Alpha' 3
  assert_match "$(shrine_shows 'on Alpha')" 'cast Time Sign "Wrap Up" on Alpha'
  assert_match "$(cast_landed "$pane1" '> things, stop and wait for me')" '> We are finishing here'

  t "smoke: the summon button opens the picker; a click there and a name bring a resident in"
  shrine_key n
  assert_match "$(shrine_shows 'summon a resident into')" 'summon a resident into'
  assert_match "$(shrine_capture)" '[ cancel q ]'
  shrine_click 'work/alpha' 3
  assert_match "$(shrine_shows 'name (Enter for a random one')" 'name (Enter for a random one'
  shrine_type Cirno
  assert_match "$(shrine_shows 'summoned Cirno')" 'summoned Cirno'
  assert_ok test -n "$(find_resident Cirno)"
  assert_re "$("$G" list)" 'Cirno'

  t "smoke: the banish button asks before it does anything, and yes sends that resident away"
  shrine_key x
  assert_match "$(shrine_shows 'banish which resident?')" 'banish which resident?'
  shrine_click 'Cirno' 3
  assert_match "$(shrine_shows 'banish Cirno?')" 'banish Cirno?'
  shrine_click '[ no n ]'
  assert_match "$(shrine_shows 'Nobody is here yet|click a resident above')" 'click a resident above'
  assert_nomatch "$(shrine_capture)" 'banish Cirno?'
  shrine_key x; shrine_shows 'banish which resident?' >/dev/null
  shrine_key 3                                # the number the line carries, not the slot
  assert_match "$(shrine_shows 'banish Cirno?')" 'banish Cirno?'
  shrine_key y
  cirno=$(find_resident Cirno); cirno=${cirno##*/}
  assert_match "$(pane_shows "$(rec_get "$RES_DIR/$cirno" pane)" 'Cirno has left the shrine')" 'Cirno has left the shrine'

  t "smoke: the departed screen is clicked the same way, and closing it takes its tab"
  assert_match "$(pane_shows "$(rec_get "$RES_DIR/$cirno" pane)" '[ close x ]')" '[ recall r ]'
  out=$(rec_get "$RES_DIR/$cirno" pane)
  row=$(tm capture-pane -p -t "$out" | grep -n -F '[ close x ]' | head -n 1 | cut -d: -f1)
  col=$(tm capture-pane -p -t "$out" | sed -n "${row}p" | awk '{ print index($0, "[ close x ]") + 1 }')
  tm send-keys -t "$out" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$col" "$row" "$col" "$row")"
  wait_for 10 '[ ! -f "$RES_DIR/$cirno" ]'
  assert_ok test ! -f "$RES_DIR/$cirno"
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 3   # Alpha, Beta, the shrine

  t "smoke: /rename inside a resident reaches list and close"
  tm send-keys -t "$pane2" -l '/rename Gamma' \; send-keys -t "$pane2" Enter
  wait_for 10 '[ "$(rec_get "$RES_DIR/$id2" name)" = Gamma ]'; rm -f "$REGISTRY"
  assert_match "$("$G" list)" 'Gamma'
  assert_eq "$(rec_get "$RES_DIR/$id2" name)" Gamma

  t "smoke: close asks for /exit; the pane shows the departed screen; the chip dims"
  out=$("$G" close gamma 2>&1)
  assert_match "$out" 'Gamma has left (/exit)'
  assert_match "$(pane_shows "$pane2" 'Gamma has left the shrine')" 'Gamma has left the shrine'
  assert_ok test -n "$(rec_get "$RES_DIR/$id2" departed)"
  rm -f "$REGISTRY"
  assert_match "$("$G" _bar 1 "$pane1")" '#[dim] 2 · Gamma '
  assert_match "$("$G" _border "$pane2" 'x')" 'departed'
  assert_re "$("$G" list)" '^  2   ·  Gamma .*departed$'
  assert_re "$("$G" resume)" '^  Gamma +.*beta +'"${id2:0:8}"' +[0-9]+[a-z]+ ago · still in its pane \(slot 2\) · no transcript$'

  t "smoke: resume recalls a departed pane in place with --resume and the summon flags; the name stays"
  out=$("$G" resume gamma 2>&1)
  assert_match "$out" 'recalled Gamma into its pane (slot 2)'
  assert_match "$(pane_shows "$pane2" "stub-claude Gamma ($id2) resumed")" "stub-claude Gamma ($id2) resumed"
  args=$(cat "$STUB_STATE/$id2.args")
  assert_match "$args" "--resume $id2"
  assert_nomatch "$args" '--session-id'
  assert_match "$args" '--model haiku --permission-mode plan'
  assert_match "$args" '--settings {"hooks":{"UserPromptSubmit"'
  assert_eq "$(rec_get "$RES_DIR/$id2" departed)|$(rec_get "$RES_DIR/$id2" pane)" "|$pane2"
  assert_match "$("$G" resume gamma 2>&1)" 'Gamma is still here (slot 2)'
  rm -f "$REGISTRY"
  assert_re "$("$G" list)" '^  2   ○  Gamma .*idle$'
  out=$("$G" close gamma 2>&1)
  # The record, not the pane: this pane still has the first departed screen in its scrollback, so
  # matching that text would pass while Gamma sat there with claude still in it - which is how a
  # lost /exit used to slip past this line and fail three tests further down instead.
  assert_match "$out" 'Gamma has left (/exit)'
  assert_ok test -n "$(rec_get "$RES_DIR/$id2" departed)"

  t "smoke: closing the departed pane takes its window with it and frees the slot"
  "$G" close 2 >/dev/null 2>&1
  # No wait: `close` removes the record itself before it returns. If this ever needs one again,
  # the command has gone back to asking the departed screen to do its work for it.
  assert_ok test ! -f "$RES_DIR/$id2"
  wait_for 5 '[ "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d " ")" = 2 ]'
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 2   # Alpha and the shrine
  assert_match "$("$G" new "$scratch/work/beta")" '(slot 2)'
  pause 0.5

  t "smoke: a /exit the resident never hears is asked again, and close reports what happened"
  # Every name typed with -n here is one share/names.txt does not hold. A placeholder is drawn at
  # random from that list, so a hard-coded name that is also in it collides with whatever an
  # earlier unnamed summon happened to draw - `new: X is already here`, and a dozen record
  # counts wrong after it. Measured 2026-09-07, after two full runs failed that way.
  # The lost keystroke that cost this suite 22 tests, on demand: the stub drops the first /exit
  # one character short, exactly as the flake did. `close` used to send it, say it had asked and
  # return, leaving a resident nothing could shift and every later test failing instead.
  out=$("$G" new "$scratch/work/alpha" -n Kasen 2>&1)
  assert_match "$out" 'summoned Kasen'
  id5=$(basename "$(find_resident Kasen)")
  wait_for 10 '[ -n "$(rec_get "$RES_DIR/$id5" pane)" ]'; pane=$(rec_get "$RES_DIR/$id5" pane)
  assert_match "$(pane_shows "$pane" "stub-claude Kasen")" 'stub-claude Kasen'
  printf '1\n' > "$STUB_STATE/$id5.eat-exit"
  out=$("$G" close Kasen 2>&1)
  assert_match "$out" 'Kasen has left (/exit)'
  assert_nomatch "$out" 'did not answer'
  assert_ok test -n "$(rec_get "$RES_DIR/$id5" departed)"
  assert_eq "$(cat "$STUB_STATE/$id5.eat-exit")" 0     # the first one really was eaten
  # And one that is never heard at all is said so, not reported as a departure.
  out=$("$G" close Kasen 2>&1); assert_match "$out" "closed Kasen's pane"   # the departed pane
  out=$("$G" new "$scratch/work/alpha" -n Hatate 2>&1)
  id5=$(basename "$(find_resident Hatate)")
  wait_for 10 '[ -n "$(rec_get "$RES_DIR/$id5" pane)" ]'; pane=$(rec_get "$RES_DIR/$id5" pane)
  assert_match "$(pane_shows "$pane" "stub-claude Hatate")" 'stub-claude Hatate'
  printf '9\n' > "$STUB_STATE/$id5.eat-exit"
  out=$("$G" close Hatate 2>&1)
  assert_match "$out" 'Hatate did not answer /exit'
  assert_eq "$(rec_get "$RES_DIR/$id5" departed)" ''
  rm -f "$STUB_STATE/$id5.eat-exit"
  out=$("$G" close Hatate 2>&1); assert_match "$out" 'Hatate has left (/exit)'
  "$G" close Hatate >/dev/null 2>&1                    # its pane, so the counts below still hold
  wait_for 5 '[ ! -f "$RES_DIR/$id5" ]'
  assert_ok test ! -f "$RES_DIR/$id5"


  t "smoke: /exit typed at a departed tab is ignored, not read as its close key"
  # The `x` in `/exit` used to close the pane and delete the record - the resident gone from
  # `resume` altogether. `quit` asks a second time when a /exit goes unheard, so a resident that
  # leaves in that instant reads the retry at this screen; typing /exit twice does it by hand.
  out=$("$G" new "$scratch/work/alpha" -n Yamame 2>&1)
  id5=$(basename "$(find_resident Yamame)")
  wait_for 10 '[ -n "$(rec_get "$RES_DIR/$id5" pane)" ]'; pane=$(rec_get "$RES_DIR/$id5" pane)
  assert_match "$(pane_shows "$pane" "stub-claude Yamame")" 'stub-claude Yamame'
  "$G" close Yamame >/dev/null 2>&1
  assert_match "$(pane_shows "$pane" 'Yamame has left the shrine')" 'Yamame has left the shrine'
  tm send-keys -t "$pane" -l '/exit' \; send-keys -t "$pane" Enter
  pause 1
  assert_ok test -f "$RES_DIR/$id5"                    # still recallable
  assert_ok tm has-session -t '=gensokyo'
  assert_eq "$(tm display -p -t "$pane" '#{pane_id}' 2>/dev/null)" "$pane"
  "$G" close Yamame >/dev/null 2>&1                    # its pane, so the counts below still hold
  wait_for 5 '[ ! -f "$RES_DIR/$id5" ]'
  assert_ok test ! -f "$RES_DIR/$id5"

  t "smoke: every resident gets a window of its own, one pane in each, the shrine's aside"
  "$G" new "$scratch/work/alpha" >/dev/null; "$G" new "$scratch/work/alpha" >/dev/null; "$G" new "$scratch/work/alpha" >/dev/null
  pause 0.5
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 6
  assert_eq "$(tm list-windows -t =gensokyo -F '#{window_panes}' | sort -u | tr '\n' ' ')" '1 '
  assert_eq "$(tm list-windows -t =gensokyo -F '#{window_name}' | grep -c '^[1-9] ')" 5

  t "smoke: a pane killed behind gensokyo's back is pruned from the bar"
  tm kill-pane -t "$pane1"; pause 0.3
  "$G" _bar 1 %99 >/dev/null
  assert_ok test ! -f "$RES_DIR/$id1"

  t "smoke: stale records are archived when the server is restarted"
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
  assert_eq "$(rec_get "$RES_DIR/$id3" slot)|$(rec_get "$RES_DIR/$id3" window)|$(rec_get "$RES_DIR/$id3" resume)|$(rec_get "$RES_DIR/$id3" launched)" "1|$(rec_get "$RES_DIR/$id3" window)|1|$(rec_get "$RES_DIR/$id3" launched)"
  pane=$(rec_get "$RES_DIR/$id3" pane)
  assert_match "$(pane_shows "$pane" "($id3) resumed")" "($id3) resumed"
  assert_match "$(cat "$STUB_STATE/$id3.args")" "--resume $id3"
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 2   # its own window, next to the shrine's
  assert_eq "$(tm display -p -t "$pane" '#{window_name}')" "1 ○ $name3"
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

  t "smoke: the last resident leaving leaves the shrine window, and the server, alone"
  # Two closes, each waited out rather than timed: the first asks for /exit and is done when the
  # record says departed, the second is the departed screen's x and is done when the record is gone.
  "$G" close 1 >/dev/null 2>&1
  wait_for 10 '[ -n "$(grep -l "^departed=" "$RES_DIR"/* 2>/dev/null)" ]'
  "$G" close 1 >/dev/null 2>&1
  wait_for 10 '[ -z "$(ls "$RES_DIR" 2>/dev/null)" ]'
  assert_ok tm has-session -t =gensokyo
  assert_eq "$(tm list-windows -t =gensokyo -F '#{window_name}')" '⛩ gensokyo'
  out=$(shrine_shows 'Nobody is here yet')
  assert_match "$out" 'Nobody is here yet'
  assert_match "$out" '[ summon n ]'

  t "smoke: the quit button asks first, and yes takes the whole cockpit down"
  # The button is the surface that matters in iTerm2, where no key of gensokyo's is bound at all.
  shrine_key q
  assert_match "$(shrine_shows 'close gensokyo?')" 'close gensokyo?'
  shrine_click '[ no n ]'
  assert_match "$(shrine_shows '[ summon n ]')" '[ summon n ]'
  assert_nomatch "$(shrine_capture)" 'close gensokyo?'
  shrine_key q; shrine_shows 'close gensokyo?' >/dev/null
  shrine_key y
  assert_ok wait_for 15 '! tm has-session -t "=$SESSION"'

  t "smoke: quit asks everyone to leave, waits for them, and takes the server with it"
  "$G" new "$scratch/work/alpha" -n Kisume >/dev/null
  id5=$(grep -l '^name=Kisume$' "$RES_DIR"/* | head -n 1); id5=${id5##*/}
  wait_for 10 '[ -n "$(rec_get "$RES_DIR/$id5" pane)" ]'
  out=$("$G" quit 2>&1)
  assert_match "$out" 'asking 1 resident(s) to leave (/exit)'
  assert_nomatch "$out" 'did not answer'
  assert_match "$out" 'closing gensokyo'
  assert_ok test -n "$(rec_get "$RES_DIR/$id5" departed)"
  assert_fails tm has-session -t '=gensokyo'
  # The record is still there and unarchived, which is what makes the next cockpit offer it back.
  assert_ok test -f "$RES_DIR/$id5"
  # It still names the pane it departed in, and that pane went down with the server, so what
  # `resume` offers has to say earlier run - not "still in its pane", which invites a recall
  # into nothing.
  out=$("$G" resume 2>&1)
  assert_match "$out" Kisume
  assert_nomatch "$out" 'still in its pane'
  assert_match "$("$G" quit 2>&1)" 'gensokyo is not running'

  t "smoke: a resident asking for the quit hands it to the server, so its own Ctrl-C cannot stop it"
  "$G" --detach >/dev/null
  out=$(GENSOKYO_RESIDENT=$id5 "$G" quit 2>&1)
  assert_match "$out" 'you are one of them'
  assert_nomatch "$out" 'asking'
  assert_ok wait_for 10 '! tm has-session -t "=$SESSION"'

  t "smoke: the clock fires a ritual into a resident of its own, with its flags and its prompt"
  local ritid ritpane front
  fresh; rm -rf "$STATE_DIR/rituals"
  mkdir -p "$CONFIG_DIR/rituals" "$scratch/work/ritual"
  # Every minute, and the clock left to say which one: GENSOKYO_NOW is not injected here. A
  # record is written with the time its run started, and prune_records drops a paneless record
  # thirty seconds later - so a fire told the minute is 09:05 hands the shrine's next sweep a
  # record that is hours old and loses the race to it. What a fixed minute is for is the unit
  # tests; what this is for is a real launch on the real clock.
  {
    printf -- '---\nschedule: "* * * * *"\ncwd: %s/work/ritual\nmodel: haiku\nkeep: 5m\n' "$scratch"
    printf 'allowed_tools: ["Read", "Bash(npm run test:*)"]\n---\ncheck the thing\n'
  } > "$CONFIG_DIR/rituals/nightly-checks.md"
  "$G" --detach >/dev/null
  front=$(pane_current)
  # The sweep the clock runs every 20 s, asked once rather than waited for: the clock is this
  # same call on a timer.
  ritual_sweep 1
  # By the field, not by being the only record: the run is found the way the cockpit finds it.
  ritid=$(grep -l '^ritual=nightly-checks$' "$RES_DIR"/* 2>/dev/null | head -n 1); ritid=${ritid##*/}
  assert_eq "$(rec_get "$RES_DIR/$ritid" ritual)" nightly-checks
  assert_ok wait_for 10 '[ -n "$(rec_get "$RES_DIR/$ritid" pane)" ]'
  ritpane=$(rec_get "$RES_DIR/$ritid" pane)
  # The prompt is the file's body with the memory file named after it, and it arrives as one
  # prompt however many lines it has.
  out=$(pane_shows "$ritpane" 'memory.md')
  assert_match "$out" '> check the thing'
  assert_match "$out" "Your notes from previous runs are at \`$STATE_DIR/rituals/nightly-checks/memory.md\`."
  # The flags the file asked for reached claude, and the tool pattern arrived as one argument -
  # --allowedTools is variadic, so the prompt is behind a `--` and the pattern is not three tools.
  assert_match "$(cat "$STUB_STATE/$ritid.args")" '--model haiku'
  # The whole way through: the record is written by the sweep, taken apart again by `_run` inside
  # the pane, and this is what claude was actually called with - so the memory file's directory
  # reached the command line rather than only the record.
  assert_match "$(cat "$STUB_STATE/$ritid.args")" "--add-dir $STATE_DIR/rituals/nightly-checks"
  assert_match "$(cat "$STUB_STATE/$ritid.args")" '--allowedTools Read Bash(npm run test:*) --'
  # Its own name on its own tab, and the front tab is left where the owner had it: a ritual
  # fires into whatever they were doing. (What iTerm2 does with the new tab only a screenshot
  # shows; the tmux half is this.)
  assert_match "$(tm list-windows -t =gensokyo -F '#{window_name}')" 'nightly-checks'
  assert_eq "$(pane_current)" "$front"
  assert_match "$(cat "$STATE_DIR/rituals/nightly-checks/log")" \
    "ran (due $(ritual_when "$(ritual_stamp nightly-checks)"))"
  # The tab's life went into the record at launch, in seconds, so that a ritual edited or deleted
  # this afternoon cannot change what happens to a tab that is already open.
  assert_eq "$(rec_get "$RES_DIR/$ritid" keep)" 300

  t "smoke: and the tab goes when its keep runs out - the /exit, the window and the record"
  # `keep: 5m` of nothing happening, written down rather than waited for: a tab's life is
  # measured on the real clock, so five minutes of idling is a status file and not five minutes.
  # The Stop hook first, through the binary, because "the run has finished" is what starts it.
  payload "$ritid" Stop ',"last_assistant_message":"checked the thing"' | "$G" _hook
  assert_eq "$(sed -n 's/^pending=//p' "$STATE_DIR/status/$ritid")" stopped
  printf 'pending=stopped\ndetail=checked the thing\nsince=%s\nmode=\n' \
    "$(( $(date +%s) - 301 ))" > "$STATE_DIR/status/$ritid"
  # The job the clock starts, run here in the foreground: this is the whole gesture against a
  # real pane - the interrupt, the /exit, and the tab only once the resident has answered it.
  "$G" _reap "$ritid"
  assert_ok wait_for 10 '! tm list-windows -t "=$SESSION" -F "#{window_name}" | grep -q nightly-checks'
  assert_fails test -f "$RES_DIR/$ritid"
  # Archived and not deleted: nobody was there to say the transcript was finished with.
  assert_ok test -f "$STATE_DIR/departed/$ritid"
  assert_eq "$("$G" resume --json | jq_ -r --arg id "$ritid" '.[] | select(.session_id == $id) | .in_pane')" false
  assert_match "$(cat "$STATE_DIR/rituals/nightly-checks/log")" 'idle 5m since the run finished (keep)'
  # And the owner's own tab is where it was: a tab closing itself does not move anybody.
  assert_eq "$(pane_current)" "$front"
  rm -f "$CONFIG_DIR/rituals/nightly-checks.md"

  t "smoke: ritual add then ritual run - a ritual written by a command, fired by hand"
  local handid handpane stamp
  # The whole way a ritual is meant to arrive: a command writes the file (which is what the skill
  # calls once the user has said yes), and another fires it now so its prompts can be approved
  # once. Both through the binary, in a cockpit that is already running.
  rm -rf "$STATE_DIR/rituals"; mkdir -p "$scratch/work/by-hand"
  out=$("$G" ritual add --name by-hand --schedule '0 4 * * *' --cwd "$scratch/work/by-hand" \
    --description 'run when it is asked to' --model haiku --prompt 'look at the thing' 2>&1)
  assert_match "$out" 'next fire'
  assert_match "$("$G" ritual list)" 'by-hand'
  # The clock finds it without being restarted, and both always-visible surfaces say so: the
  # shrine's own line, and the bar iTerm2 draws (status-right, which push_bar writes).
  assert_match "$(shrine_shows '⏲ next  by-hand')" '⏲ next  by-hand'
  # The bar the clock pushes into the two options iTerm2 draws. Pushed here rather than waited
  # for: the clock skips its render while nothing is attached, and nothing is attached to this
  # detached cockpit.
  push_bar
  assert_match "$(tm show -gv status-right)" '⏲ 04:00 by-hand'
  # The stamp the sweep would have written for a minute that has not come round yet: a hand run
  # must not spend it, or the 04:00 fire would skip itself.
  ritual_stamp_set by-hand 1788000000; stamp=$(ritual_stamp by-hand)
  out=$("$G" ritual run by-hand 2>&1)
  assert_match "$out" "by-hand is running in $(tilde "$scratch/work/by-hand")"
  handid=$(grep -l '^ritual=by-hand$' "$RES_DIR"/* 2>/dev/null | head -n 1); handid=${handid##*/}
  assert_ok wait_for 10 '[ -n "$(rec_get "$RES_DIR/$handid" pane)" ]'
  handpane=$(rec_get "$RES_DIR/$handid" pane)
  assert_match "$(pane_shows "$handpane" 'memory.md')" '> look at the thing'
  assert_match "$(cat "$STUB_STATE/$handid.args")" '--model haiku'
  assert_match "$(cat "$STATE_DIR/rituals/by-hand/log")" 'ran (by hand)'
  assert_eq "$(ritual_stamp by-hand)" "$stamp"
  assert_match "$(tm list-windows -t =gensokyo -F '#{window_name}')" 'by-hand'
  # Whose run this resident is, wherever a resident is listed: nobody summoned it, and the
  # ritual's name is the answer to what it is doing there.
  assert_match "$(shrine_shows '⏲ by-hand ·')" '⏲ by-hand ·'
  assert_match "$("$G" list)" '· ⏲ by-hand'
  assert_eq "$("$G" list --json | jq_ -r --arg id "$handid" '.[] | select(.session_id == $id) | .ritual')" by-hand
  # And its own listing now knows it has run, from the log rather than from the stamp.
  assert_match "$("$G" ritual log by-hand)" 'ran (by hand)'
  assert_match "$("$G" ritual list)" 'last ran'
  rm -f "$CONFIG_DIR/rituals/by-hand.md"

  t "smoke: a headless fire opens no window, and the run outlives the cockpit that started it"
  local hpid wins
  # The one thing about a headless run that needs a real server: who its parent is. The clock is
  # a `run-shell -b` child of the tmux server, and `quit` takes both down - so the sweep is left
  # to the clock here rather than called from this process, which would make the run this test's
  # child instead of the clock's and prove nothing about the promise below.
  tm kill-server 2>/dev/null
  fresh; rm -rf "$STATE_DIR/rituals"; rm -f "$CONFIG_DIR/rituals"/*.md
  mkdir -p "$CONFIG_DIR/rituals" "$scratch/work/quiet"
  printf -- '---\nschedule: "* * * * *"\nheadless: true\ncwd: %s/work/quiet\n---\ntake your time\n' \
    "$scratch" > "$CONFIG_DIR/rituals/quiet-one.md"
  # Slow enough to still be going when the cockpit is taken down under it, and exported before
  # the server starts: the clock has the environment the server was started with, and nothing
  # else reaches it.
  export STUB_P_SLEEP=6
  "$G" --detach >/dev/null
  unset STUB_P_SLEEP
  assert_ok wait_for 30 '[ -s "$STATE_DIR/rituals/quiet-one/headless.pid" ]'
  hpid=$(cat "$STATE_DIR/rituals/quiet-one/headless.pid")
  assert_ok ritual_headless_running quiet-one
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 0            # nobody was summoned for it
  wins=$(tm list-windows -t "=$SESSION" -F 1 | wc -l | tr -d ' ')
  assert_eq "$wins" 1                                           # and no tab either: the shrine's
  # Cutting a turn off halfway would leave the ritual's notes half written for nothing gained,
  # so the run is meant to survive the quit - which is what its log header tells whoever reads it.
  "$G" quit >/dev/null 2>&1
  assert_fails tm has-session -t "=$SESSION"
  assert_ok kill -0 "$hpid"
  assert_ok wait_for 25 '! kill -0 "$hpid" 2>/dev/null'
  assert_match "$(cat "$STATE_DIR/rituals/quiet-one/log")" 'done (headless,'
  assert_match "$(cat "$STATE_DIR"/rituals/quiet-one/runs/*.log)" 'stub -p ran in'
  rm -f "$CONFIG_DIR/rituals/quiet-one.md"
  tm kill-server 2>/dev/null

  t "smoke: a persistent ritual fires twice into one session, and both prompts are in it"
  local keptid keptpane
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/departed"; rm -f "$CONFIG_DIR/rituals"/*.md
  mkdir -p "$CONFIG_DIR/rituals" "$scratch/work/kept"
  printf -- '---\nschedule: "0 4 * * *"\ntarget: persistent\ncwd: %s/work/kept\n---\nmind the shop\n' \
    "$scratch" > "$CONFIG_DIR/rituals/kept-one.md"
  "$G" --detach >/dev/null
  # The first fire has no session to join, so it starts one - with the prompt as its argument,
  # the way a `new` run gets it - and that session is the ritual's from then on.
  assert_match "$("$G" ritual run kept-one 2>&1)" "on its way to the session it keeps"
  assert_ok wait_for 20 '[ -s "$STATE_DIR/rituals/kept-one/session-id" ]'
  keptid=$(cat "$STATE_DIR/rituals/kept-one/session-id")
  assert_ok test -f "$RES_DIR/$keptid"
  assert_ok wait_for 15 '[ -n "$(rec_get "$RES_DIR/$keptid" pane)" ]'
  keptpane=$(rec_get "$RES_DIR/$keptid" pane)
  assert_match "$(pane_shows "$keptpane" 'mind the shop')" '> mind the shop'
  # Its tab is not on a clock: `keep` is about one run's tab, and this session is the ritual.
  assert_eq "$(rec_get "$RES_DIR/$keptid" keep)" ''
  # The second fire has one to join, so the prompt is typed into it - and the first prompt is
  # still there above it, which is the whole point of a session that is kept.
  assert_match "$("$G" ritual run kept-one 2>&1)" "on its way to the session it keeps"
  assert_ok wait_for 25 'grep -q "sent to" "$STATE_DIR/rituals/kept-one/log"'
  out=$(cast_landed "$keptpane" 'Read them first')
  # Two prompts submitted in one session, which is what a kept session is for. The stub echoes
  # every prompt it is given behind a `> `, so counting those lines counts the fires - and it is
  # a count rather than a pattern because the pane also holds the paste going in.
  assert_eq "$(printf '%s\n' "$out" | grep -c '^> mind the shop$')" 2
  assert_eq "$("$G" list --json | jq_ -r 'length')" 1            # and one resident, not two
  assert_match "$(cat "$STATE_DIR/rituals/kept-one/log")" "sent to kept-one (by hand)"

  t "smoke: and when that session has left, the fire recalls it and lands in it"
  # `/exit` in the pane rather than `gensokyo close`, because what is being tested is the state
  # the ritual finds - a departed screen where the input line used to be.
  tm send-keys -t "$keptpane" -l '/exit' \; send-keys -t "$keptpane" Enter
  assert_ok wait_for 15 '[ -n "$(rec_get "$RES_DIR/$keptid" departed)" ]'
  : > "$STATE_DIR/rituals/kept-one/log"
  assert_match "$("$G" ritual run kept-one 2>&1)" "on its way to the session it keeps"
  assert_ok wait_for 40 'grep -q "sent to" "$STATE_DIR/rituals/kept-one/log"'
  assert_eq "$(rec_get "$RES_DIR/$keptid" departed)" ''          # recalled, and in a pane again
  assert_match "$(cast_landed "$(rec_get "$RES_DIR/$keptid" pane)" 'Read them first')" 'mind the shop'
  assert_eq "$(cat "$STATE_DIR/rituals/kept-one/session-id")" "$keptid"   # the same session
  rm -f "$CONFIG_DIR/rituals/kept-one.md"
  tm kill-server 2>/dev/null

  t "smoke: a ritual aimed at a resident by name types its prompt into that resident"
  local namedid
  fresh; rm -rf "$STATE_DIR/rituals"; rm -f "$CONFIG_DIR/rituals"/*.md
  mkdir -p "$CONFIG_DIR/rituals"
  "$G" --detach >/dev/null
  "$G" new "$scratch/work/alpha" --name Suika >/dev/null 2>&1
  assert_ok wait_for 20 'find_resident Suika >/dev/null'
  namedid=$(find_resident Suika); namedid=${namedid##*/}
  assert_ok wait_for 15 '[ -n "$(rec_get "$RES_DIR/$namedid" pane)" ]'
  printf -- '---\nschedule: "0 4 * * *"\ntarget: Suika\n---\nlook at the cellar\n' \
    > "$CONFIG_DIR/rituals/cellar.md"
  assert_match "$("$G" ritual run cellar 2>&1)" "cellar's prompt is on its way to Suika"
  assert_ok wait_for 30 'grep -q "sent to Suika" "$STATE_DIR/rituals/cellar/log"'
  # The prompt and nothing else: a resident summoned by hand has no --add-dir for the ritual's
  # directory, so the sentence naming the memory file would be a permission prompt every morning.
  out=$(cast_landed "$(rec_get "$RES_DIR/$namedid" pane)" 'look at the cellar')
  assert_match "$out" '> look at the cellar'
  assert_nomatch "$out" 'Your notes from previous runs'
  # Nobody was summoned for it, and Suika is still Suika: no record of the ritual's own.
  assert_eq "$("$G" list --json | jq_ -r 'length')" 1
  assert_eq "$("$G" list --json | jq_ -r '.[0].ritual')" null
  # And a target that is not here is a fire that says so rather than one that goes missing.
  printf -- '---\nschedule: "0 4 * * *"\ntarget: Nobody\n---\nwho?\n' \
    > "$CONFIG_DIR/rituals/nowhere.md"
  "$G" ritual run nowhere >/dev/null 2>&1
  assert_ok wait_for 20 'grep -q "not sent" "$STATE_DIR/rituals/nowhere/log"'
  assert_match "$(cat "$STATE_DIR/rituals/nowhere/log")" 'not sent: there is no resident called Nobody'
  rm -f "$CONFIG_DIR/rituals/cellar.md" "$CONFIG_DIR/rituals/nowhere.md"
  tm kill-server 2>/dev/null
}
