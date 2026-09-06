# lib/hooks.sh - what gensokyo injects into every resident with `claude --settings` (hook
# commands), the status files those hooks write, and the "needs you" notifications they fire.
# Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

CLAUDE_DIR=${CLAUDE_CONFIG_DIR:-$HOME/.claude}   # Claude Code's settings.json and projects/

# ---------------------------------------------------------------- settings chain
# Claude Code reads settings from (highest precedence first) <cwd>/.claude/settings.local.json,
# <cwd>/.claude/settings.json and ~/.claude/settings.json. settings_value <cwd> <jq path>
# prints the first value defined along that chain, e.g. settings_value ~/dev/x .advisorModel
settings_value() {
  local cwd=$1 expr=$2 f files=()
  for f in "$cwd/.claude/settings.local.json" "$cwd/.claude/settings.json" "$CLAUDE_DIR/settings.json"; do
    [ -f "$f" ] && files[${#files[@]}]=$f
  done
  [ "${#files[@]}" -gt 0 ] || return 0
  jq_ -r -s "[.[] | $expr | select(. != null)][0] // empty" "${files[@]}" 2>/dev/null
}

# launch_settings <id> <cwd>: the JSON passed as `claude --settings` to every resident. Hook
# entries merge with the user's own hooks (Claude Code merges hooks across settings levels),
# so nothing of theirs is lost. Every event runs `gensokyo _hook` with the payload on stdin.
# Only the events the cockpit needs are wired: prompt typed, turn ended, permission and idle
# notifications, and a question dialog opening and closing. statusLine is a single key that
# this replaces for the session, so it points at `gensokyo _statusline <id>`, which records
# the JSON and then runs the user's own status line command (lib/telemetry.sh); their
# padding setting is carried over.
launch_settings() {
  local id=${1:-} cwd=${2:-} pad
  pad=$(settings_value "$cwd" '.statusLine.padding')
  # shellcheck disable=SC2016
  jq_ -n -c --arg self "$SELF" --arg id "$id" --arg pad "$pad" '
    {type: "command", command: (($self | @sh) + " _hook")} as $h
    | {hooks: {
        UserPromptSubmit: [{hooks: [$h]}],
        Stop:             [{hooks: [$h]}],
        Notification:     [{hooks: [$h]}],
        PreToolUse:       [{matcher: "AskUserQuestion", hooks: [$h]}],
        PostToolUse:      [{matcher: "AskUserQuestion", hooks: [$h]}]},
       statusLine: ({type: "command", command: (($self | @sh) + " _statusline " + ($id | @sh))}
                    + (if $pad == "" then {} else {padding: ($pad | tonumber)} end))}'
}

# ---------------------------------------------------------------- status files
# state/status/<session-id>, written by _hook: pending=awaits|question|stopped|'' is what the
# resident waits for (a permission, an answer to its question, your next prompt), detail one
# line about it, since the epoch of the last pending change, mode the permission mode from the
# last UserPromptSubmit/Stop payload (empty until the first prompt of the session).
status_load() {
  # shellcheck disable=SC2034  # loaded for every reader; not every reader uses all of them
  S_pending='' S_detail='' S_since='' S_mode=''
  load_kv "$STATE_DIR/status/$1" S pending detail since mode
}

# status_write <id> <pending> <detail> [mode]: mode is kept when not given; since moves only
# when pending changes.
status_write() {
  local id=$1 pending=$2 detail=$3 mode=${4:-} since f=$STATE_DIR/status/$1
  status_load "$id"
  [ -n "$mode" ] || mode=$S_mode
  since=$S_since
  if [ "$pending" != "$S_pending" ] || [ -z "$since" ]; then since=$(date +%s); fi
  mkdir -p "$STATE_DIR/status"
  printf 'pending=%s\ndetail=%s\nsince=%s\nmode=%s\n' "$pending" "$detail" "$since" "$mode" > "$f.tmp.$$" \
    && mv "$f.tmp.$$" "$f"
}

# ---------------------------------------------------------------- the hook
# `gensokyo _hook` runs for every hook event Claude Code fires inside a resident, with the
# JSON payload on stdin (Claude Code 2.1.260 payloads: hook_event_name, session_id, and per
# event: permission_mode in UserPromptSubmit and Stop only; notification_type + message in
# Notification; tool_name + tool_input in PreToolUse/PostToolUse; last_assistant_message in
# Stop). Nothing may reach stdout: UserPromptSubmit hook output is fed to the model. The exit
# code is always 0: a non-zero one would block the prompt or the tool call.
# GENSOKYO_HOOK_LOG=<file> appends every raw payload, for looking at what a Claude Code version sends.
cmd__hook() {
  local event id ntype mode tool question message last old payload
  exec >/dev/null 2>&1
  payload=$(cat)
  [ -n "${GENSOKYO_HOOK_LOG:-}" ] && printf '%s\n' "$payload" >> "$GENSOKYO_HOOK_LOG"
  IFS='|' read -r event id ntype mode tool question message last <<EOF
$(printf '%s' "$payload" | jq_ -r '[.hook_event_name, .session_id, (.notification_type // ""), (.permission_mode // ""),
           (.tool_name // ""), (.tool_input.questions[0].question // ""), (.message // ""),
           ((.last_assistant_message // "") | split("\n")[0])]
          | map(tostring | gsub("[\\t\\n\\r|]"; " ")) | join("|")')
EOF
  [ -n "${id:-}" ] && [ -f "$RES_DIR/$id" ] || return 0   # malformed, or not a resident of ours
  rec_load "$RES_DIR/$id"
  status_load "$id"; old=$S_pending
  case $event in
    UserPromptSubmit) status_write "$id" '' '' "$mode" ;;
    Stop) status_write "$id" stopped "${last:0:80}" "$mode" ;;
    Notification)
      case $ntype in
        permission_prompt) status_write "$id" awaits "${tool:+permission: $tool}" ;;
        agent_needs_input|elicitation*) status_write "$id" awaits "${message:0:80}" ;;
        idle_prompt) [ -n "$old" ] || status_write "$id" stopped '' ;;   # late backup for a missed Stop
      esac ;;
    PreToolUse) [ "$tool" = AskUserQuestion ] && status_write "$id" question "${question:0:80}" ;;
    PostToolUse) [ "$tool" = AskUserQuestion ] && [ "$old" = question ] && status_write "$id" '' '' ;;
  esac
  status_load "$id"
  if [ -n "$S_pending" ] && [ "$S_pending" != "$old" ]; then notify "$R_name" "$R_pane" "$S_pending" "$S_detail"; fi
  # A hook that raises a flag must put the glyph on screen at once, which needs the registry as
  # it is now: whether a chip is gold depends on the resident being idle there rather than busy.
  # UserPromptSubmit only clears a flag, and it runs in front of the prompt the user just typed,
  # so it pushes what the clock last fetched instead of paying for `claude agents --json` there.
  case $event in
    UserPromptSubmit) refresh_bar stale ;;
    *) refresh_bar ;;
  esac
  return 0
}

# ---------------------------------------------------------------- notifications
# notify <name> <pane> <kind> <detail>: a toast on every attached client, then a desktop alert
# and a terminal bell unless the owner is already watching that pane (pane_watched). One
# notification per waiting period: _hook calls this only when pending changes.
notify() {
  local name=$1 pane=$2 kind=$3 detail=$4 text glyph c tty
  case $kind in
    question) text="$name asks: $detail" ;;
    awaits)   text="$name needs your permission${detail:+ (${detail#permission: })}" ;;
    *)        text="$name is done${detail:+: $detail}" ;;
  esac
  glyph=$(glyph_for "$kind")
  if [ "$CFG_NOTIFY_TOAST" = on ]; then
    for c in $(tmux_ list-clients -F '#{client_name}' 2>/dev/null); do
      tmux_ display-message -c "$c" -d 4000 "$glyph ${text//\#/##}"   # a bare # would start a tmux format
    done
  fi
  pane_watched "$pane" && return 0
  [ "$CFG_NOTIFY_DESKTOP" = on ] && desktop_notify "$name" "$text"
  if [ "$CFG_NOTIFY_BELL" = on ] && [ -n "$pane" ]; then
    tty=$(tmux_ display -p -t "$pane" '#{pane_tty}' 2>/dev/null) && [ -w "$tty" ] && printf '\a' > "$tty"
  fi
  return 0
}

# pane_watched <pane>: is the owner looking at that pane right now? Then the desktop alert and
# the bell would interrupt the very thing they are reading, and the toast and the gold chip say
# it well enough. The two kinds of client have to be asked different questions:
#   control mode - three things at once, because no one of them is enough: iTerm2 is the
#     application in front, the pane is the one that client sits on, and the tab iTerm2 has in
#     front is that pane's. The third is the one that hurts to leave out: iTerm2 puts ordinary
#     tabs beside the cockpit's, and moving to one of those tells tmux nothing, so the client
#     stays on whichever resident was last selected and a resident nobody is looking at would be
#     silenced. `client_activity` is no help either: it never moves for a control client, since
#     iTerm2 speaks tmux's protocol instead of typing at it, and the `focused` flag stays set
#     even with another application in front.
#   plain client - it shows that pane and the user touched it within the last 10 s.
# Whatever cannot be answered counts as not watched: an alert nobody needed is a nuisance, one
# that never came leaves a resident waiting unseen.
pane_watched() {
  local pane=$1 now front='' tab='' flags act p
  [ -n "$pane" ] || return 1
  now=$(date +%s)
  while IFS='|' read -r flags act p; do
    [ "$p" = "$pane" ] || continue
    case ",${flags%,}," in
      *,control-mode,*)
        [ -n "$front" ] || front=$(iterm_frontmost)   # one ask, however many clients are on it
        [ "$front" = yes ] || continue                # behind another application: ask no further
        [ -n "$tab" ] || tab=$(iterm_front_tab)
        watched_cc "$front" "$p" "$pane" "$tab" "$(pane_title_of "$pane")" && return 0 ;;
      *)
        watched_tty "$act" "$now" && return 0 ;;
    esac
  done <<EOF
$(tmux_ list-clients -F '#{client_flags},|#{client_activity}|#{pane_id}' 2>/dev/null)
EOF
  return 1
}

# The two rules as they are decided, apart from the asking, so that both can be tried without a
# client attached. watched_cc <yes|no iTerm2 is in front> <the client's pane> <the resident's>
# <the front tab's name> <that pane's title>; watched_tty <that client's last activity> <now>.
# An empty answer never matches: iTerm2 declining to say which tab is in front, or a pane with
# no title, has to mean "tell them", not "keep quiet". Both epochs look alike, so watched_tty
# refuses a negative age as well as an old one: with the two arguments the wrong way round the
# answer would otherwise be "watched" every time, and nobody would ever be told anything.
watched_cc()  { [ "$1" = yes ] && [ -n "$3" ] && [ "$2" = "$3" ] && [ -n "$5" ] && [ "$4" = "$5" ]; }
watched_tty() { local age=$(( ${2:-0} - ${1:-0} )); [ "$age" -ge 0 ] && [ "$age" -le 10 ]; }

# iterm_front_tab: the name iTerm2 shows on the tab in front, which for one of the cockpit's
# tabs is that pane's `#{pane_title}` - the only signal that tracks the front tab, measured
# against every other candidate on a live cockpit. Nothing here belongs to gensokyo (the title
# is the resident's own), but both sides read the same string at the same moment, and it is
# never trusted alone. Empty when iTerm2 will not say, which counts as a mismatch.
iterm_front_tab() {
  command -v osascript >/dev/null 2>&1 || return 0
  osascript -e 'tell application "iTerm2" to tell current session of current window to get name' \
    2>/dev/null
}

pane_title_of() { tmux_ display -p -t "$1" '#{pane_title}' 2>/dev/null; }

# iterm_frontmost: `yes` when iTerm2 is the application in front, `no` when it is not and
# whenever the answer cannot be had. Asked once per waiting resident and never on a timer,
# since it costs a round trip to another process. `lsappinfo` reads the front application out
# of the window server: nothing to ask permission for, nothing that can block. AppleScript is
# the fallback and iTerm2 answers about itself, but a server started from another terminal is
# a stranger to it, and macOS would put a consent dialog in front of a hook that has to return.
# The fallback also takes iTerm2 for granted: `tell application` starts an application that is
# not running, and only a control-mode client - which is iTerm2, already running - asks this.
iterm_frontmost() {
  local out
  if command -v lsappinfo >/dev/null 2>&1; then
    out=$(lsappinfo info -only bundleid "$(lsappinfo front 2>/dev/null)" 2>/dev/null)
    case $out in
      *com.googlecode.iterm2*) printf 'yes\n'; return 0 ;;
      ?*)                      printf 'no\n';  return 0 ;;
    esac
  fi
  if command -v osascript >/dev/null 2>&1 \
     && [ "$(osascript -e 'tell application "iTerm2" to get frontmost' 2>/dev/null)" = true ]; then
    printf 'yes\n'; return 0
  fi
  printf 'no\n'
}

# desktop_notify <subtitle> <text>: macOS osascript (the text goes in as arguments, so no
# AppleScript quoting), else notify-send when present, else nothing. macOS may ask once to
# allow notifications from the terminal application.
desktop_notify() {
  case $(desktop_notifier) in
    osascript) osascript -e 'on run argv' \
      -e 'display notification (item 2 of argv) with title "Gensokyo" subtitle (item 1 of argv)' \
      -e 'end run' "$1" "$2" >/dev/null 2>&1 ;;
    notify-send) notify-send "Gensokyo · $1" "$2" >/dev/null 2>&1 ;;
  esac
  return 0
}
desktop_notifier() {
  if command -v osascript >/dev/null 2>&1; then printf osascript
  elif command -v notify-send >/dev/null 2>&1; then printf notify-send
  else printf none; fi
}

# refresh_bar: put the chips right now instead of waiting for the clock's next tick. The
# plain tmux client redraws its own bar on `refresh-client -S` (which re-runs the #()
# commands); iTerm2 reads the text gensokyo pushes, so that is rewritten too. With nobody
# attached there is no bar to correct: the first tick after the next attach draws it.
refresh_bar() {   # refresh_bar [stale registry ok]
  local c n=0
  for c in $(tmux_ list-clients -F '#{client_name}' 2>/dev/null); do
    n=$((n + 1))
    tmux_ refresh-client -S -t "$c"
  done
  [ "$n" -gt 0 ] && { push_bar "${1:-}"; shrine_signal; }
  return 0
}
