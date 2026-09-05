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

# launch_settings: the JSON passed as `claude --settings` to every resident. Hook entries merge
# with the user's own hooks (Claude Code merges hooks across settings levels), so nothing of
# theirs is lost. Every event runs `gensokyo _hook` with the payload on stdin. Only the events
# the cockpit needs are wired: prompt typed, turn ended, permission and idle notifications,
# and a question dialog opening and closing.
launch_settings() {
  jq_ -n -c --arg self "$SELF" '
    {type: "command", command: (($self | @sh) + " _hook")} as $h
    | {hooks: {
        UserPromptSubmit: [{hooks: [$h]}],
        Stop:             [{hooks: [$h]}],
        Notification:     [{hooks: [$h]}],
        PreToolUse:       [{matcher: "AskUserQuestion", hooks: [$h]}],
        PostToolUse:      [{matcher: "AskUserQuestion", hooks: [$h]}]}}'
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
  refresh_bar
  return 0
}

# ---------------------------------------------------------------- notifications
# notify <name> <pane> <kind> <detail>: a toast on every attached client, then a desktop alert
# and a terminal bell unless the pane is on screen in a client the user touched in the last
# 10 s. One notification per waiting period: _hook calls this only when pending changes.
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

# pane_watched <pane>: an attached client shows that pane (the active pane of its current
# window) and had activity within the last 10 s.
pane_watched() {
  local now c act p
  [ -n "$1" ] || return 1
  now=$(date +%s)
  while read -r c act p; do
    [ "$p" = "$1" ] && [ $((now - ${act:-0})) -le 10 ] && return 0
  done <<EOF
$(tmux_ list-clients -F '#{client_name} #{client_activity} #{pane_id}' 2>/dev/null)
EOF
  return 1
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

# refresh_bar: redraw every client's status line now (refresh-client -S re-runs the #()
# commands) instead of waiting for status-interval.
refresh_bar() {
  local c
  for c in $(tmux_ list-clients -F '#{client_name}' 2>/dev/null); do tmux_ refresh-client -S -t "$c"; done
  return 0
}
