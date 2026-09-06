# lib/cockpit.sh - what the user sees and presses inside tmux: the attach command, the shrine
# pane, the key legend, the status bar and border formats, and the run-shell actions behind
# the keys and menus. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# ---------------------------------------------------------------- commands: cockpit
# Which client attaches. In iTerm2 tmux runs in control mode (`-CC`): iTerm2 draws the
# session as native tabs and panes, with its own status bar and no keys of ours. Anywhere
# else, and with --tty or --nested, the plain tmux client draws the cockpit itself.
home_mode() {   # home_mode <--tty given> <--nested given>
  if [ -n "$1" ] || [ -n "$2" ]; then printf 'tty\n'
  elif [ "${TERM_PROGRAM:-}" = iTerm.app ]; then printf 'cc\n'
  else printf 'tty\n'
  fi
}

cmd_home() {
  local nested='' detach='' tty='' mode other n
  while [ $# -gt 0 ]; do
    case $1 in
      --nested) nested=1 ;;
      --detach) detach=1 ;;
      --tty) tty=1 ;;
      *) die "unknown option: $1 (see: gensokyo help)" ;;
    esac
    shift
  done
  if [ -n "${TMUX:-}" ]; then
    if inside_own_server; then
      die "you are already inside gensokyo ($(prefix_label) ? lists the keys)"
    elif [ -z "$nested" ]; then
      die "running inside another tmux; use 'gensokyo --nested' if you really want nested tmux (prefix conflicts are yours)"
    else
      warn "nested tmux: the outer prefix may shadow $(prefix_label)"
      unset TMUX TMUX_PANE
    fi
  fi
  mode=$(home_mode "$tty" "$nested")
  start_server
  ensure_clock
  if [ -n "$detach" ]; then
    say "gensokyo is running detached (socket $SOCKET); 'gensokyo' attaches."
    return 0
  fi
  [ "$mode" = tty ] && [ -z "$tty" ] && [ -z "$nested" ] &&
    warn "not started from iTerm2 (TERM_PROGRAM=${TERM_PROGRAM:-unset}): using the plain tmux client, $(prefix_label) then ? lists its keys"
  other=cc; [ "$mode" = cc ] && other=tty
  n=$(clients_in_mode "$other")
  [ "$n" -gt 0 ] && warn "$n other client(s) are attached the other way; one status bar serves both, so theirs goes blank until they detach"
  apply_status "$mode"
  apply_user_conf
  push_bar   # so iTerm2 has the chips the moment it attaches, not three seconds later
  if [ "$mode" = cc ]; then exec "$TMUX_BIN" -L "$SOCKET" -CC attach-session -t "=$SESSION"; fi
  exec "$TMUX_BIN" -L "$SOCKET" attach-session -t "=$SESSION"
}

cmd__keys() {
  local table key label cmd
  printf '  %s is the prefix. Press it, release, then:\n\n' "$(prefix_label)"
  while IFS='|' read -r table key label cmd; do
    [ "$table" = prefix ] && printf '    %-4s %s\n' "$key" "$label"
  done <<EOF
$(key_table)
EOF
  printf '\n  %s then g, then:\n\n' "$(prefix_label)"
  while IFS='|' read -r table key label cmd; do
    [ "$table" = gensokyo ] && printf '    %-4s %s\n' "$key" "$label"
  done <<EOF
$(key_table)
EOF
  printf '\n  Everything else goes to the resident.\n  Shift+Enter or Option+Enter: newline.\n\n  any key to close '
  read -r -s -n 1 _ 2>/dev/null
}

cmd__menu-shrine() {
  local client=$1 table key label cmd args
  args=()
  while IFS='|' read -r table key label cmd; do
    [ "$table" = gensokyo ] || continue
    [ "$key" = g ] && continue
    args=(${args[@]+"${args[@]}"} "$label" "$key" "$cmd")
  done <<EOF
$(key_table)
EOF
  tmux_ display-menu -c "$client" -T " gensokyo " -x C -y C ${args[@]+"${args[@]}"}
}

# ---------------------------------------------------------------- status bar
# Row 1: one chip per resident (slot, glyph, name, model, context used); gold when the
# resident awaits you, dim when departed, bold for the focused pane. Right: waiting count and
# outsiders. Row 2: clock, the always-visible key legend and, at the right, the account-wide
# 5-hour and weekly usage from the newest status line report.
#
# Two readers with different rules. The plain tmux client runs `_bar` from `status-format` as
# a `#()` command and understands `#[...]` styles: that is the `styled` text, and `#()` output
# must never fail or print more than one line. iTerm2 never draws tmux's status line; it shows
# the *value* of `status-left` / `status-right`, where one `#[...]` blanks the whole bar. So
# `plain` carries no styles at all: a resident who needs you is told apart by its ✦ / ✧ glyph
# and by coming first, and row 2 is the usage alone (iTerm2 has its own clock, and no key of
# ours to put in a legend).
cmd__bar() {   # cmd__bar <row 1|2> [active pane] [styled|plain]
  local right
  case $1 in
    1) bar_chips "${2:-}" "${3:-styled}" ;;
    2)
      right=$(usage_text)
      if [ "${3:-styled}" = plain ]; then
        printf '%s' "$right"
      else
        printf ' ⛩ %s   %s %s' "$(date +%H:%M)" "$(legend_text)" "${right:+#[align=right]$right }"
      fi ;;
  esac
  return 0
}

bar_chips() {   # bar_chips <active pane> <styled|plain>
  local active=$1 style=$2 rows out='' first='' right='' waiting=0 n chip \
    slot id name state cwd pane win mode detail model ctx rest attrs tele
  prune_records
  load_registry
  rows=$(resident_rows)
  if [ -z "$rows" ]; then
    if [ "$style" = plain ]; then
      printf ' ⛩ gensokyo  no residents yet '
    else
      printf ' ⛩ gensokyo  no residents yet · %s then g n to summon ' "$(prefix_label)"
    fi
    return 0
  fi
  while IFS='|' read -r slot id name state cwd pane win mode detail model ctx rest; do
    [ -n "$slot" ] || continue
    attrs= tele=
    case $state in
      waiting|question) waiting=$((waiting + 1)); attrs="bg=$CFG_COLOR_AWAIT,fg=black" ;;
      departed) attrs=dim; model= ;;
    esac
    [ "$pane" = "$active" ] && attrs="${attrs:+$attrs,}bold"
    [ -n "$model" ] && tele=" $(model_short "$model")${ctx:+ $ctx%}"
    # The separator is braced off every chip: bash 3.2 reads "$chip│" as the name "chip│".
    chip=" $slot $(glyph_for "$state") ${name:0:14}$tele "
    if [ "$style" = plain ]; then
      case $state in
        waiting|question) first="$first${chip}│" ;;
        *) out="$out${chip}│" ;;
      esac
    else
      out="$out#[${attrs:-default}]${chip}#[default]│"
    fi
  done <<EOF
$rows
EOF
  out="$first$out"
  n=$(outsider_count)
  if [ "$style" = plain ]; then
    [ "$waiting" -gt 0 ] && right="✦ $waiting "
    [ "$n" -gt 0 ] && right="$right+$n outside "
    printf '%s%s' "${out%│}" "${right:+  $right}"
  else
    [ "$waiting" -gt 0 ] && right="#[bg=$CFG_COLOR_AWAIT,fg=black] ✦ $waiting #[default] "
    [ "$n" -gt 0 ] && right="$right+$n outside "
    printf '%s%s' "${out%│}" "${right:+#[align=right]$right}"
  fi
  return 0
}

# push_bar: render the bar and write it into `status-left` / `status-right`, the two options
# tmux exposes to iTerm2, which draws them in its own status bar and follows a change within
# a second with no re-attach. Called every few seconds by the clock and immediately by the
# hooks, so those two options belong to gensokyo while the cockpit runs: a
# ~/.config/gensokyo/tmux.conf that sets them is overwritten at the next tick. The plain tmux
# client reads neither (it draws `status-format` itself), so this costs it nothing and leaves
# the bar correct for the next client, whichever kind it is.
push_bar() {   # push_bar [stale registry ok]
  local left right ttl=$REGISTRY_TTL
  [ -n "${1:-}" ] && ttl=86400
  local REGISTRY_TTL=$ttl   # dynamic scope: it reaches load_registry, and only for this call
  # The tabs first: they are the other always-visible surface, and they must not go stale
  # because a bar render came back empty.
  load_registry
  push_titles "$(resident_rows)"
  left=$(cmd__bar 1 '' plain 2>/dev/null)
  right=$(cmd__bar 2 '' plain 2>/dev/null)
  # The chips are never legitimately empty (with no residents they say so), so an empty render
  # is a failure: keep the last good bar rather than blanking it. The usage half is empty on
  # accounts without rate limits.
  [ -n "$left" ] || return 0
  left=$(bar_escape "$left"); right=$(bar_escape "$right")
  [ "$(tmux_ show -gv status-left 2>/dev/null)" = "$left" ] || tmux_ set -g status-left "$left"
  [ "$(tmux_ show -gv status-right 2>/dev/null)" = "$right" ] || tmux_ set -g status-right "$right"
  return 0
}

# resident_title <slot> <state> <name>: the short form a tab carries, e.g. "1 ✦ Reimu".
# iTerm2 shrinks tab titles as tabs multiply, so it is the slot, the state glyph and the
# name and nothing else; the full telemetry is in the status bar, the shrine tab and `list`.
resident_title() { printf '%s %s %s' "$1" "$(glyph_for "$2")" "$3"; }

# push_titles <resident_rows output>: keep every resident's tab title current. iTerm2 shows
# the tmux window name as the tab title (share/tmux.conf) and follows a `rename-window`
# live, including tabs the user is not looking at, which is what makes the tab bar a
# sidebar: who is here, who needs you, who you are in. Renaming only on change keeps the tab
# from flickering, like the bar.
push_titles() {
  local rows=$1 names slot id name state cwd pane win rest title
  names=$'\n'$(tmux_ list-windows -t "=$SESSION" -F '#{window_id} #{window_name}' 2>/dev/null)$'\n'
  [ "$names" != $'\n\n' ] || return 0
  while IFS='|' read -r slot id name state cwd pane win rest; do
    case $win in @[0-9]*) ;; *) continue ;; esac   # no window yet, or an outsider's row
    title=$(resident_title "$slot" "$state" "$name")
    case $names in *$'\n'"$win $title"$'\n'*) continue ;; esac
    # `rename-window` expands formats and a resident can rename itself (/rename), so a `#`
    # in the name is doubled to arrive as one. `%` means nothing here (no strftime).
    tmux_ rename-window -t "$win" "${title//\#/\#\#}" 2>/dev/null
  done <<EOF
$rows
EOF
  return 0
}

# tmux expands both options before iTerm2 sees their value: `%` goes through strftime and `#`
# starts a format substitution, so `ctx 42% used` would arrive as `ctx 42 used`. Doubling both
# characters is what survives.
bar_escape() {
  local s=${1//\#/\#\#}
  printf '%s' "${s//%/%%}"
}

# Pane border: "slot name · dir ⎇ branch · model→⚖ advisor · effort · mode · ⚡cache · $cost"
# for a resident (unknown fields left out), "⛩" for the shrine. Claude Code sets the pane
# title to "✳ <name>" (from --name, following /rename); the title wins over the record
# because it changes within a second, tmux's default hostname title is ignored.
cmd__border() {
  local pane=$1 title=$2 f name branch tele
  f=$(record_by_pane "$pane")
  if [ -n "$f" ]; then
    rec_load "$f"
    name=$R_name
    case $title in "✳ "*) name=${title#✳ } ;; esac
    if [ -n "$R_departed" ]; then
      printf ' %s %s · departed  [r] recall  [x] close ' "$R_slot" "$name"
    else
      status_load "${f##*/}"; tele_load "${f##*/}"
      branch=$(git_branch "$R_cwd")
      tele=$(tele_fields "$T_model" "$T_ctx" "$T_effort" "$T_cache" "$T_tcache" "$T_cost" "$branch" "$T_advisor" "${S_mode:-$R_mode}")
      printf ' %s %s · %s%s%s ' "$R_slot" "$name" "$(basename "$R_cwd")" "${branch:+ ⎇ $branch}" "${tele:+ · $tele}"
    fi
  elif [ "$(tmux_ show -p -v -t "$pane" @shrine 2>/dev/null)" = 1 ]; then
    printf ' ⛩ '
  fi
  return 0
}

# ---------------------------------------------------------------- key actions (run-shell)
# Called by tmux; print nothing (run-shell shows any output over the pane) and exit 0.
cmd__focus() {
  local n=$1 client=$2 f
  f=$(grep -l "^slot=$n\$" "$RES_DIR"/* 2>/dev/null | head -n 1)
  [ -n "$f" ] || { tmux_ display-message -c "$client" "no resident $n"; return 0; }
  rec_load "$f"
  # Between the summon and the resident's first breath the record has neither field yet;
  # say so rather than doing nothing, since this is the key pressed right after a summon.
  [ -n "${R_window:-$R_pane}" ] || { tmux_ display-message -c "$client" "$R_name is still starting"; return 0; }
  focus_window "${R_window:-$R_pane}"
}

# The key and the shrine's button both come here, because `reload` respawns the shrine's pane and
# would otherwise be killing the very process that asked for it. This one runs as a child of the
# tmux server (run-shell), where nothing it does can cut its own ground away.
cmd__reload() {
  local client=${1:-} out
  out=$("$SELF" reload 2>&1)
  out=$(printf '%s\n' "$out" | grep -v '^[[:space:]]*$' | tail -n 1)
  if [ -n "$client" ]; then tmux_ display-message -c "$client" "${out//\#/##}"
  else tmux_ display-message "${out//\#/##}"; fi
  return 0
}

cmd__menu-summon() {
  local client=$1 s d i=0 args=() label
  s=$(sq "$SELF")
  if [ -f "$STATE_DIR/recent-dirs" ]; then
    while IFS= read -r d; do
      [ -d "$d" ] || continue
      i=$((i + 1)); [ "$i" -le 8 ] || break
      label=$(tilde "$d" 60)
      args=(${args[@]+"${args[@]}"} "$label" "$i" "display-popup -E -c $client -w 72 -h 10 '$s _popup-summon $(sq "$d")'")
    done < "$STATE_DIR/recent-dirs"
  fi
  args=(${args[@]+"${args[@]}"} "other directory..." o "display-popup -E -c $client -w 72 -h 10 '$s _popup-summon'" "" "cancel" q "")
  tmux_ display-menu -c "$client" -T " summon a resident into " -x C -y C "${args[@]}"
}

# Runs inside a display-popup: asks for the directory (tab completion) unless given, then a name.
cmd__popup-summon() {
  local dir=${1:-} name
  printf '\033[2J\033[H'
  if [ -z "$dir" ]; then
    printf '  directory (Tab completes, Enter for %s):\n  ' "$(tilde "$PWD")"
    read -e -r dir || exit 0
    dir=${dir:-$PWD}
  else
    printf '  directory: %s\n' "$(tilde "$dir")"
  fi
  printf '  name (Enter for a random one): '
  read -e -r name || exit 0
  set -- "$dir" --focus
  [ -n "$name" ] && set -- "$@" -n "$name"
  if ! "$SELF" new "$@"; then printf '\n  any key to close '; read -r -s -n 1 _ 2>/dev/null; fi
}

cmd__menu-banish() {
  local client=$1 pane=$2 f
  f=$(record_by_pane "$pane")
  [ -n "$f" ] || { tmux_ display-message -c "$client" "no resident in this pane"; return 0; }
  rec_load "$f"
  # A menu rather than confirm-before (which fails with "invalid confirm key" from run-shell).
  # display-menu blocks this process until the menu closes; run-shell tolerates that.
  if [ -n "$R_departed" ]; then
    tmux_ display-menu -c "$client" -T " $R_name has departed " -x C -y C \
      "close this pane" y "run-shell -b '$(sq "$SELF") close ${f##*/} >/dev/null 2>&1'" "keep it" n ""
  else
    tmux_ display-menu -c "$client" -T " banish $R_name (slot $R_slot)? " -x C -y C \
      "yes, ask $R_name to /exit" y "run-shell -b '$(sq "$SELF") close ${f##*/} >/dev/null 2>&1'" "no" n ""
  fi
}
