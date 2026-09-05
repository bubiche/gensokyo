# lib/cockpit.sh - what the user sees and presses inside tmux: the attach command, the shrine
# pane, the key legend, the status bar and border formats, and the run-shell actions behind
# the keys and menus. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# ---------------------------------------------------------------- commands: cockpit
cmd_home() {
  local nested='' detach='' cc=''
  while [ $# -gt 0 ]; do
    case $1 in
      --nested) nested=1 ;;
      --detach) detach=1 ;;
      --cc) cc=1 ;;
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
  start_server
  if [ -n "$detach" ]; then
    say "gensokyo is running detached (socket $SOCKET); 'gensokyo' attaches."
    return 0
  fi
  if [ -n "$cc" ]; then exec "$TMUX_BIN" -L "$SOCKET" -CC attach-session -t "=$SESSION"; fi
  exec "$TMUX_BIN" -L "$SOCKET" attach-session -t "=$SESSION"
}

# The shrine pane fills an empty stage: banner + legend. It is replaced by the first
# resident summoned into that stage and comes back when the last one leaves.
cmd__shrine() {
  [ -n "${TMUX_PANE:-}" ] && tmux_ set -p -t "$TMUX_PANE" @shrine 1 2>/dev/null
  printf '\033]2;gensokyo\007\033[2J\033[H'   # reset the pane title left by a departed resident, clear
  [ -f "$SHARE/banner.txt" ] && cat "$SHARE/banner.txt"
  printf '\n  Nobody is here yet.\n\n  %s then  g n   summon a resident\n  %s then  ?     every key\n  %s then  d     detach (gensokyo keeps running)\n\n  From a shell:  gensokyo new [dir] [-n name]\n' \
    "$(prefix_label)" "$(prefix_label)" "$(prefix_label)"
  while :; do read -r -s -n 1 _ 2>/dev/null || sleep 3600; done
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
# Row 1: one chip per resident (slot, glyph, name); gold when the resident awaits you, dim
# when departed, bold for the focused pane. Right: waiting count and outsiders.
# Row 2: clock and the always-visible key legend (usage bars arrive with the statusLine).
# `#()` output may carry #[...] styles; it must never fail or print more than one line.
cmd__bar() {
  local active=${2:-} rows out='' right='' waiting=0 n slot id name state cwd pane win rest attrs
  case $1 in
    1)
      prune_records
      load_registry
      rows=$(resident_rows)
      if [ -z "$rows" ]; then
        printf ' ⛩ gensokyo  no residents yet · %s then g n to summon ' "$(prefix_label)"
        return 0
      fi
      while IFS='|' read -r slot id name state cwd pane win rest; do
        [ -n "$slot" ] || continue
        attrs=
        case $state in
          waiting|question) waiting=$((waiting + 1)); attrs="bg=$CFG_COLOR_AWAIT,fg=black" ;;
          departed) attrs=dim ;;
        esac
        [ "$pane" = "$active" ] && attrs="${attrs:+$attrs,}bold"
        out="$out#[${attrs:-default}] $slot $(glyph_for "$state") ${name:0:14} #[default]│"
      done <<EOF
$rows
EOF
      [ "$waiting" -gt 0 ] && right="#[bg=$CFG_COLOR_AWAIT,fg=black] ✦ $waiting #[default] "
      n=$(outsider_count)
      [ "$n" -gt 0 ] && right="$right+$n outside "
      printf '%s%s' "${out%│}" "${right:+#[align=right]$right}" ;;
    2)
      printf ' ⛩ %s   %s ' "$(date +%H:%M)" "$(legend_text)" ;;
  esac
  return 0
}

# Pane border: "slot name · dir" for a resident, "⛩" for the shrine. Claude Code sets the
# pane title to "✳ <name>" (from --name, following /rename); the title wins over the record
# because it changes within a second, tmux's default hostname title is ignored.
cmd__border() {
  local pane=$1 title=$2 f name
  f=$(record_by_pane "$pane")
  if [ -n "$f" ]; then
    rec_load "$f"
    name=$R_name
    case $title in "✳ "*) name=${title#✳ } ;; esac
    if [ -n "$R_departed" ]; then
      printf ' %s %s · departed  [r] recall  [x] close ' "$R_slot" "$name"
    else
      printf ' %s %s · %s ' "$R_slot" "$name" "$(basename "$R_cwd")"
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
  focus_pane "$R_pane" "$client"
}

cmd__cycle() {
  local dir=$1 pane=$2 first='' prev='' target='' found='' last='' slot id name state cwd p win rest
  while IFS='|' read -r slot id name state cwd p win rest; do
    [ -n "$slot" ] && [ "$p" != - ] || continue
    [ -z "$first" ] && first=$p
    [ -n "$found" ] && [ -z "$target" ] && [ "$dir" = next ] && target=$p
    if [ "$p" = "$pane" ]; then found=1; [ "$dir" = prev ] && target=$prev; fi
    prev=$p; last=$p
  done <<EOF
$(resident_rows)
EOF
  [ -n "$target" ] || { if [ "$dir" = prev ] && [ -n "$found" ]; then target=$last; else target=$first; fi; }
  [ -n "$target" ] && focus_pane "$target"
  return 0
}

cmd__layout() {
  local client=$1 win cur next
  win=$(tmux_ display -p -c "$client" '#{window_id}')
  cur=$(tmux_ show -w -v -t "$win" @layout 2>/dev/null)
  case $cur in tiled) next=main-vertical ;; main-vertical) next=even-horizontal ;; *) next=tiled ;; esac
  tmux_ select-layout -t "$win" "$next" \; set -w -t "$win" @layout "$next" \; display-message -c "$client" "layout: $next"
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
