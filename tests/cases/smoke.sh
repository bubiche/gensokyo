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
# The shrine's pane, found the way gensokyo finds it: the loop marks it with @shrine. Its
# window is not the session's current one once a resident has been focused.
shrine_pane() { tm list-panes -s -t =gensokyo -F '#{pane_id} #{@shrine}' 2>/dev/null | awk '$2 == 1 { print $1 }'; }
# capture it by that pane id only: an empty -t would read whichever pane is current instead,
# and report a resident's screen as the shrine's.
shrine_capture() { local p; p=$(shrine_pane); [ -n "$p" ] && tm capture-pane -p -t "$p" 2>/dev/null; return 0; }
# pane_shows <pane> <text>: what the pane holds, once it holds that text. A pane is driven by
# keystrokes and by a stub that has to start, so a fixed pause is a race that a busy machine
# loses; this waits for the state the assertion is about and then reads it once.
pane_shows() {
  local out i
  for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
    out=$(tm capture-pane -p -t "$1" 2>/dev/null)
    case $out in *"$2"*) break ;; esac
    pause 0.3
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
  while [ "$i" -lt 20 ]; do
    [ "$(pane_current)" = "$1" ] && break
    i=$((i + 1)); pause 0.2
  done
  pane_current
}
# shrine_shows <text>: the shrine redraws on its own clock, so give it a tick to catch up.
shrine_shows() {
  local out i
  for i in 1 2 3 4 5 6 7 8 9; do
    out=$(shrine_capture)
    case $out in *"$1"*) break ;; esac
    pause 0.5
  done
  printf '%s' "$out"
}
cleanup() { [ -n "${TMUX_BIN:-}" ] && tm kill-server 2>/dev/null; rm -rf "$scratch"; }
trap cleanup EXIT

smoke_tests() {
  local out pane1 pane2 pane id1 id2 id3 id4 name3 args cirno row col
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
  assert_match "$args" '--append-system-prompt'
  assert_match "$args" '--settings {"hooks":{"UserPromptSubmit"'
  assert_match "$args" '--model haiku --permission-mode plan'
  assert_eq "$(rec_get "$RES_DIR/$id2" mode)" plan
  assert_nomatch "$(cat "$STUB_STATE/$id1.args")" '--model'
  assert_match "$(tm capture-pane -p -t "$pane1")" 'env CLAUDE*: 0'

  t "smoke: the shrine tab draws a block per resident and the buttons under them"
  assert_re "$(shrine_pane)" '^%[0-9]+$'
  out=$(shrine_shows '2 ○ Beta')
  assert_re "$out" '^  1 ○ Alpha +alpha'
  assert_re "$out" '^  2 ○ Beta +beta'
  assert_match "$out" '[ summon n ]  [ banish x ]  [ recall r ]  [ cast s ]  [ timetable t ]  [ ? ]'
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

  t "smoke: clicking a resident's line in the shrine brings its window to the front"
  assert_match "$(shrine_shows '2 ○ Beta')" '2 ○ Beta'
  shrine_click '2 ○ Beta' 3
  assert_eq "$(pane_fronts "$pane2")" "$pane2"
  shrine_click '1 ○ Alpha' 3
  assert_eq "$(pane_fronts "$pane1")" "$pane1"

  t "smoke: so does the slot number the line carries, and a click on a blank row does nothing"
  shrine_key 2
  assert_eq "$(pane_fronts "$pane2")" "$pane2"
  shrine_click '⏲ no rituals yet' 3          # a row with nothing on it to click
  pause 0.6
  assert_eq "$(pane_current)" "$pane2"
  "$G" _focus 1 "$(tm list-clients -F '#{client_name}' | head -n 1)" >/dev/null 2>&1   # the prefix-1 key
  assert_eq "$(pane_fronts "$pane1")" "$pane1"

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
  pause 1
  assert_ok test ! -f "$RES_DIR/$cirno"
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 3   # Alpha, Beta, the shrine

  t "smoke: /rename inside a resident reaches list and close"
  tm send-keys -t "$pane2" -l '/rename Gamma' \; send-keys -t "$pane2" Enter
  pause 0.5; rm -f "$REGISTRY"
  assert_match "$("$G" list)" 'Gamma'
  assert_eq "$(rec_get "$RES_DIR/$id2" name)" Gamma

  t "smoke: close asks for /exit; the pane shows the departed screen; the chip dims"
  out=$("$G" close gamma 2>&1)
  assert_match "$out" 'Gamma'
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
  "$G" close gamma >/dev/null 2>&1
  assert_match "$(pane_shows "$pane2" 'Gamma has left the shrine')" 'Gamma has left the shrine'

  t "smoke: closing the departed pane takes its window with it and frees the slot"
  "$G" close 2 >/dev/null 2>&1
  wait_for 5 '[ ! -f "$RES_DIR/$id2" ]'
  assert_ok test ! -f "$RES_DIR/$id2"
  wait_for 5 '[ "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d " ")" = 2 ]'
  assert_eq "$(tm list-windows -t =gensokyo -F x | wc -l | tr -d ' ')" 2   # Alpha and the shrine
  assert_match "$("$G" new "$scratch/work/beta")" '(slot 2)'
  pause 0.5

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
  "$G" close 1 >/dev/null 2>&1; pause 1.2
  "$G" close 1 >/dev/null 2>&1; pause 0.5
  assert_ok tm has-session -t =gensokyo
  assert_eq "$(tm list-windows -t =gensokyo -F '#{window_name}')" '⛩ gensokyo'
  out=$(shrine_shows 'Nobody is here yet')
  assert_match "$out" 'Nobody is here yet'
  assert_match "$out" '[ summon n ]'
  tm kill-server
}
