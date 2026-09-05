# lib/server.sh - gensokyo's own tmux server: start it, bind the keys, style the bar, place
# stage windows and move focus between panes. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# ---------------------------------------------------------------- tmux server
server_running() { tmux_ has-session -t "=$SESSION" 2>/dev/null; }
sq() { printf '%q' "$1"; }   # shell-quote for tmux command strings

# Inside our own server? $TMUX looks like /tmp/tmux-501/<socket>,<pid>,<idx>.
inside_own_server() {
  local path=${TMUX:-}
  [ -n "$path" ] && [ "$(basename "${path%%,*}")" = "$SOCKET" ]
}

prefix_label() {
  case $CFG_PREFIX in
    C-Space) printf 'Ctrl-Space' ;;
    C-*) printf 'Ctrl-%s' "${CFG_PREFIX#C-}" ;;
    M-*) printf 'Alt-%s' "${CFG_PREFIX#M-}" ;;
    *) printf '%s' "$CFG_PREFIX" ;;
  esac
}

# One table drives the bindings, the `g g` menu and the `?` list, so the legend cannot
# drift from what the keys do. Fields: table|key|label|tmux command.
# `#{client_name}` / `#{pane_id}` are expanded by tmux before run-shell runs the command.
key_table() {
  local s
  s=$(sq "$SELF")
  cat <<EOF
prefix|1-9|focus resident 1-9|
prefix|z|zoom / unzoom the focused resident|resize-pane -Z
prefix|n|next resident|run-shell "$s _cycle next '#{pane_id}'"
prefix|p|previous resident|run-shell "$s _cycle prev '#{pane_id}'"
prefix|g|gensokyo actions (then one key)|switch-client -T gensokyo
prefix|?|list every key|display-popup -E -w 66 -h 30 "$s _keys"
prefix|d|detach; the cockpit keeps running|detach-client
gensokyo|n|summon a new resident|run-shell "$s _menu-summon '#{client_name}'"
gensokyo|x|banish (close) the focused resident|run-shell "$s _menu-banish '#{client_name}' '#{pane_id}'"
gensokyo|r|recall a departed resident|run-shell "$s _menu-recall '#{client_name}'"
gensokyo|s|cast a spell card (broadcast a prompt)|run-shell "$s _menu-spell '#{client_name}'"
gensokyo|m|message a resident|run-shell "$s _menu-message '#{client_name}'"
gensokyo|w|who is around|display-popup -E -w 90% -h 70% "$s who --wait"
gensokyo|l|cycle the pane layout|run-shell "$s _layout '#{client_name}'"
gensokyo|t|timetable of rituals|run-shell "$s _menu-timetable '#{client_name}'"
gensokyo|g|menu of every action|run-shell "$s _menu-shrine '#{client_name}'"
gensokyo|?|list every key|display-popup -E -w 66 -h 30 "$s _keys"
EOF
}

apply_keys() {
  local table key label cmd i s
  s=$(sq "$SELF")
  tmux_ set -g prefix "$CFG_PREFIX" \; unbind C-b \; bind "$CFG_PREFIX" send-prefix
  for i in 1 2 3 4 5 6 7 8 9; do
    tmux_ bind "$i" run-shell "$s _focus $i '#{client_name}'"
  done
  while IFS='|' read -r table key label cmd; do
    [ -n "$cmd" ] || continue
    # $cmd carries its own quoting (see key_table), so it goes through eval; the key is
    # quoted because `?` would otherwise glob against files in the current directory.
    case $table in
      prefix)   eval "tmux_ bind $(sq "$key") $cmd" ;;
      gensokyo) eval "tmux_ bind -T gensokyo $(sq "$key") $cmd" ;;
    esac
  done <<EOF
$(key_table)
EOF
}

legend_text() {
  printf '%s then  1-9 focus · z zoom · n/p next/prev · g menu · ? keys' "$(prefix_label)"
}

apply_style() {
  local s
  s=$(sq "$SELF")
  tmux_ set -g status-style "bg=$CFG_COLOR_BAR,fg=$CFG_COLOR_BAR_FG" \
    \; set -g pane-border-style "fg=$CFG_COLOR_BORDER" \
    \; set -g pane-active-border-style "fg=$CFG_COLOR_BAR,bold" \
    \; set -g pane-border-format "#($s _border '#{pane_id}' '#{pane_title}')" \
    \; set -g message-style "bg=$CFG_COLOR_AWAIT,fg=black" \
    \; set -g mode-style "bg=$CFG_COLOR_AWAIT,fg=black" \
    \; set -g mouse "$CFG_MOUSE"
}

# The status line is where the two clients disagree, so it is set at attach time rather than
# once at startup. iTerm2 (control mode) draws its own bar from the `status-left` and
# `status-right` strings, reads them when the client attaches, and shows nothing at all when a
# `status-format` override is in place: one row, no override, and room for the text gensokyo
# pushes into it. The plain client gets the two rows gensokyo draws itself. One server, one
# set of options, so the last client to attach wins; `doctor` shows who is attached.
apply_status() {
  local s
  case $1 in
    cc)
      tmux_ set -gu status-format \
        \; set -g status on \
        \; set -g status-left-length 200 \; set -g status-right-length 200
      ;;
    *)
      s=$(sq "$SELF")
      tmux_ set -g status 2 \
        \; set -g "status-format[0]" "#($s _bar 1 '#{pane_id}')" \
        \; set -g "status-format[1]" "#[bg=$CFG_COLOR_ROW2,fg=$CFG_COLOR_ROW2_FG]#($s _bar 2)"
      ;;
  esac
}

# How many clients of one kind are attached. tmux marks a control-mode client (iTerm2's) in
# `client_flags`; everything else is a plain terminal client.
clients_in_mode() {   # clients_in_mode cc|tty
  local want=$1 flags n=0
  # A client with no flags at all still prints the trailing comma, so an empty line here
  # means "no clients", not "a client without flags".
  while IFS= read -r flags; do
    [ -n "$flags" ] || continue
    case ",${flags%,}," in
      *,control-mode,*) [ "$want" = cc ] && n=$((n + 1)) ;;
      *) [ "$want" = tty ] && n=$((n + 1)) ;;
    esac
  done <<EOF
$(tmux_ list-clients -F '#{client_flags},' 2>/dev/null)
EOF
  printf '%s\n' "$n"
}

client_summary() {
  local cc tty out=''
  cc=$(clients_in_mode cc); tty=$(clients_in_mode tty)
  [ "$cc" -gt 0 ] && out="$cc in iTerm2 (control mode)"
  [ "$tty" -gt 0 ] && out="${out:+$out, }$tty plain"
  printf '%s\n' "${out:-none attached}"
}

start_server() {
  server_running && return 0
  ensure_dirs
  archive_records
  scrub_env
  tmux_ -f "$SHARE/tmux.conf" new-session -d -s "$SESSION" -n stage1 -x 200 -y 50 \
    "$(sq "$SELF") _shrine" || die "could not start the tmux server"
  # Pin the resolved binaries and dirs in the server environment so run-shell commands,
  # hooks and residents resolve exactly what this launch resolved.
  tmux_ set-environment -g GENSOKYO_TMUX "$TMUX_BIN" \
    \; set-environment -g GENSOKYO_JQ "$JQ_BIN" \
    \; set-environment -g GENSOKYO_CLAUDE "$CLAUDE_BIN" \
    \; set-environment -g GENSOKYO_STATE_DIR "$STATE_DIR" \
    \; set-environment -g GENSOKYO_CONFIG_DIR "$CONFIG_DIR" \
    \; set-environment -g GENSOKYO_SOCKET "$SOCKET"
  apply_keys
  apply_style
  apply_status tty   # cmd_home switches it when the attach is iTerm2's
  apply_user_conf
  start_clock
  return 0
}

# ~/.config/gensokyo/tmux.conf overrides everything gensokyo sets, so it is sourced last -
# after the server starts and again after each attach restyles the status line.
apply_user_conf() {
  [ -f "$CONFIG_DIR/tmux.conf" ] && tmux_ source-file "$CONFIG_DIR/tmux.conf"
  return 0
}

# ---------------------------------------------------------------- the clock
# One loop per server, pushing the status bar (and, once rituals exist, firing them). It has
# to be invisible: iTerm2 draws every tmux window as a native tab, so a window of ours would
# be a tab in the user's face, whatever it was named. `run-shell -b` runs the loop as a child
# of the tmux server instead - no window, no pane, nothing for a client to show - and tmux
# kills it when the server exits, so the clock lives exactly as long as the cockpit. It has no
# window to see it by, so it leaves a heartbeat (`state/clock`) that `doctor` reports and that
# every attach checks.
CLOCK_TICK=3
start_clock() {
  : > "$STATE_DIR/clock"   # count as a tick at once, so a second attach does not start a second loop
  tmux_ run-shell -b "$(sq "$SELF") _tick >/dev/null 2>&1"
}

# clock_age: seconds since the last tick, or nothing when the clock has never run.
clock_age() {
  [ -f "$STATE_DIR/clock" ] || return 0
  printf '%s\n' $(( $(date +%s) - $(mtime_of "$STATE_DIR/clock") ))
}

# A server that is already running was started by whatever version was installed then, and a
# loop can die; without a clock the chips never move. Every attach checks the heartbeat.
ensure_clock() {
  local age
  age=$(clock_age)
  [ -n "$age" ] && [ "$age" -lt $((CLOCK_TICK * 3)) ] && return 0
  start_clock
  return 0
}

cmd__tick() {
  while :; do
    server_running || return 0
    : > "$STATE_DIR/clock"   # the heartbeat doctor reads; its mtime is the time of this tick
    # Nobody attached, nobody looking: skip the render, which would ask `claude agents --json`
    # for a fresh registry every few seconds for as long as the server runs.
    [ -n "$(tmux_ list-clients -F 1 2>/dev/null)" ] && push_bar
    sleep "$CLOCK_TICK"
  done
}

stage_of_slot() { printf 'stage%s\n' "$(( ($1 - 1) / ${CFG_STAGE_SIZE:-4} + 1 ))"; }
find_window() { tmux_ list-windows -t "=$SESSION" -F '#{window_id} #{window_name}' 2>/dev/null | awk -v n="$1" '$2 == n {print $1; exit}'; }

# focus_pane <pane> [client]: switch to the pane's window and pane; a zoom in the client's
# current window carries over so `prefix N` while zoomed shows resident N zoomed.
focus_pane() {
  local pane=$1 client=${2:-} zoomed=0
  [ -n "$client" ] && zoomed=$(tmux_ display -p -c "$client" '#{window_zoomed_flag}' 2>/dev/null)
  tmux_ select-window -t "$pane" \; select-pane -Z -t "$pane" 2>/dev/null || return 0
  if [ "$zoomed" = 1 ] && [ "$(tmux_ display -p -t "$pane" '#{window_zoomed_flag}')" != 1 ]; then
    tmux_ resize-pane -Z -t "$pane"
  fi
  return 0
}
