# lib/residents.sh - the resident commands (new, close, list) and the wrapper
# that runs inside a resident's pane. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

cmd_new() {
  local dir='' name='' model='' effort='' mode='' focus='' prompt='' flags=() argstr='' f
  local id slot rec
  while [ $# -gt 0 ]; do
    case $1 in
      -n|--name) name=${2:-}; shift ;;
      -m|--model) model=${2:-}; shift ;;
      -e|--effort) effort=${2:-}; shift ;;
      -p|--permission-mode) mode=${2:-}; shift ;;
      --prompt) prompt=${2:-}; shift ;;
      --focus) focus=1 ;;
      -*) die "new: unknown option $1 (usage: gensokyo new [dir] [-n name] [-m model] [-e effort] [-p permission-mode] [--focus])" ;;
      *) [ -z "$dir" ] || die "new: one directory only"; dir=$1 ;;
    esac
    shift
  done
  ensure_dirs
  dir=${dir:-$PWD}
  dir=$(cd "${dir/#\~/$HOME}" 2>/dev/null && pwd) || die "new: no such directory: $dir"
  [ -n "$mode" ] || mode=$CFG_PERMISSION_MODE
  case $prompt in *$'\n'*) die "new: --prompt must be a single line" ;; esac
  if [ -n "$name" ]; then
    # A name starts with a letter so that it can never be mistaken for a slot number.
    case $name in ''|[!A-Za-z]*|*[!A-Za-z0-9_.-]*) die "new: a name starts with a letter and uses letters, digits, _ . - only" ;; esac
    find_resident "$name" >/dev/null && die "new: $name is already here (gensokyo list)"
  else
    name=$(pick_name)
  fi
  [ -n "$model" ]  && flags[${#flags[@]}]=--model && flags[${#flags[@]}]=$model
  [ -n "$effort" ] && flags[${#flags[@]}]=--effort && flags[${#flags[@]}]=$effort
  [ -n "$mode" ]   && flags[${#flags[@]}]=--permission-mode && flags[${#flags[@]}]=$mode
  for f in ${flags[@]+"${flags[@]}"}; do argstr="$argstr $(sq "$f")"; done

  start_server
  id=$(new_uuid); slot=$(next_slot); rec=$RES_DIR/$id
  # No `pane=` or `window=` here: `_run` writes both from inside the pane it lands in, so
  # there is one writer for them and no interleaved rec_set to lose a field.
  {
    printf 'slot=%s\nname=%s\ncwd=%s\nlaunched=%s\nargs=%s\n' "$slot" "$name" "$dir" "$(date +%s)" "${argstr# }"
    [ -n "$prompt" ] && printf 'prompt=%s\n' "$prompt"
    [ -n "$mode" ] && printf 'mode=%s\n' "$mode"   # shown until the first prompt reports the live mode
  } > "$rec"

  open_pane "$id" "$(resident_title "$slot" starting "$name")" "$focus" >/dev/null \
    || { rm -f "$rec"; die "new: tmux could not open a window"; }
  remember_dir "$dir"
  say "summoned $name (slot $slot) in $dir"
}

# open_pane <session-id> <title> [focus]: a window of its own, one pane, running
# `_run <session-id>`; prints the window id. One resident per window is the whole layout -
# gensokyo never splits, tiles or zooms, because every tmux window is an iTerm2 tab and the
# tab bar is the sidebar. No `-t <index>`: iTerm2 rewrites window indexes to match the tab
# bar as soon as the user drags a tab, so the index is the user's and the id is ours.
open_pane() {
  local id=$1 title=$2 focus=${3:-} cmd win here
  cmd="$(sq "$SELF") _run $id"
  here=$(tmux_ display -p '#{window_id}' 2>/dev/null)
  win=$(tmux_ new-window -d -P -F '#{window_id}' -n "$title" "$cmd") || return 1
  if [ -n "$focus" ]; then
    focus_window "$win"
  elif [ -n "$here" ] && [ "$here" != "$win" ] && inside_own_server && [ "$(clients_in_mode cc)" -gt 0 ]; then
    # iTerm2 opens the tab and moves to it even for a detached `new-window -d`. That is a
    # rude interruption when the summon came from inside the cockpit - a resident asking for
    # a helper should not rip the user out of the tab they were reading - so focus goes back
    # to where it was. Selecting back at once loses the race with the tab opening; a moment's
    # wait wins it, at the price of a visible flick. A summon typed in a shell outside the
    # cockpit is the other way round: the user asked for that resident and wants to be in it,
    # so the steal is left alone.
    sleep 1.5
    focus_window "$here"
  fi
  printf '%s\n' "$win"
}

# pane_settled <pane>: wait for the Ctrl-C cmd_close just sent to land, then for the pane to
# stop moving.
# How long a resident takes to come back to an empty prompt is its own business, and a fixed wait
# is a race: too short and the interrupt eats the first characters of what is typed next, which
# leaves a resident that was asked to leave and never heard it. A spinner, a redraw or an
# interrupt being handled all read as the pane changing; a pane waiting for input does not.
pane_settled() {
  local a b n=0
  nap 0.3
  a=$(tmux_ capture-pane -p -t "$1" 2>/dev/null)
  while [ "$n" -lt 12 ]; do
    nap 0.15
    b=$(tmux_ capture-pane -p -t "$1" 2>/dev/null)
    [ "$a" = "$b" ] && return 0
    a=$b; n=$((n + 1))
  done
  return 0
}

cmd_close() {
  local f id
  [ -n "${1:-}" ] || die "usage: gensokyo close <name|slot>"
  f=$(find_resident "$1") || die "close: no resident '$1' (gensokyo list)"
  rec_load "$f"; id=${f##*/}
  if [ -n "$R_departed" ]; then
    tmux_ send-keys -t "$R_pane" x     # the departed screen removes the record and closes the pane
    say "closed $R_name's pane"
  elif [ "$(tmux_ display -p -t "$R_pane" '#{pane_dead}' 2>/dev/null)" = 1 ]; then
    rm -f "$f"; tmux_ kill-pane -t "$R_pane"
    say "closed $R_name's dead pane"
  else
    # Ctrl-C first: it clears a half-typed prompt (or interrupts the running turn) so that
    # /exit lands on an empty line. `send-keys -l` then `send-keys Enter` submits reliably.
    tmux_ send-keys -t "$R_pane" C-c
    pane_settled "$R_pane"
    tmux_ send-keys -t "$R_pane" -l '/exit' \; send-keys -t "$R_pane" Enter
    say "asked $R_name to leave (/exit); the pane shows the departed screen once claude exits"
  fi
}

cmd_list() {
  local json='' all='' wait='' rows='' id status name cwd slot state pane win mode detail now tele n
  local model ctx effort cache tcache cost branch advisor five freset week wreset at
  while [ $# -gt 0 ]; do
    case $1 in
      --json) json=1 ;; --all) all=1 ;; --wait) wait=1 ;;
      *) die "list: unknown option $1 (usage: gensokyo list [--json] [--all])" ;;
    esac
    shift
  done
  ensure_dirs
  load_registry
  rows=$(resident_rows)
  if [ -n "$all" ]; then
    while IFS='|' read -r id status name cwd _; do
      [ -n "$id" ] || continue
      [ -f "$RES_DIR/$id" ] && continue
      rows="$rows"$'\n'"-|$id|$name|${status}|$cwd|-|outside|||||||||||||||"
    done <<EOF
$REG
EOF
  fi
  rows=$(printf '%s\n' "$rows" | sed '/^$/d')
  if [ -n "$json" ]; then
    # Columns as in resident_rows (lib/registry.sh); telemetry is null until the resident's
    # status line has reported once.
    # shellcheck disable=SC2016
    printf '%s\n' "$rows" | jq_ -R -s '
      def opt: if . == "" or . == "-" then null else . end;
      def num: if . == "" then null else tonumber end;
      def window($p; $r): if $p == "" then null else {used_pct: ($p | tonumber), resets_at: ($r | num)} end;
      split("\n") | map(select(length > 0) | split("|"))
      | map({slot: (.[0] | opt | if . == null then null else tonumber end), session_id: .[1], name: .[2],
             status: .[3], cwd: .[4], pane: (.[5] | opt), window: .[6], outside: (.[6] == "outside"),
             permission_mode: (.[7] | opt), detail: (.[8] | opt), branch: (.[15] | opt),
             telemetry: (if .[21] == "" then null else
               {model: (.[9] | opt), context_pct: (.[10] | num), effort: (.[11] | opt),
                cache_pct: (.[12] | num), turn_cache_pct: (.[13] | num), cost_usd: (.[14] | num),
                advisor: (.[16] | opt), five_hour: window(.[17]; .[18]), seven_day: window(.[19]; .[20]),
                at: (.[21] | tonumber)} end)})'
    return 0
  fi
  if [ -z "$rows" ]; then
    say "nobody is here yet: gensokyo new [dir] [-n name]"
  else
    now=$(date +%s)
    printf '  %-3s %-2s %-16s %-36s %s\n' '#' '' name directory session
    while IFS='|' read -r slot id name state cwd pane win mode detail model ctx effort cache tcache cost branch advisor five freset week wreset at; do
      [ -n "$slot" ] || continue
      [ "$slot" = - ] && slot=' '
      printf '  %-3s %s  %-16s %-36s %s  %s%s\n' "$slot" "$(glyph_for "$state")" "${name:0:16}" "$(tilde "$cwd" 36)" "${id:0:8}" \
        "$state" "${detail:+ ($detail)}"
      # A second line once the resident's status line has reported (mode alone is not worth one).
      [ -n "$at" ] && printf '         %s · %s ago\n' \
        "$(tele_fields "$model" "$ctx" "$effort" "$cache" "$tcache" "$cost" "$branch" "$advisor" "$mode" verbose)" "$(fmt_age $((now - at)))"
    done <<EOF
$rows
EOF
    say
    say "  ● busy  ✦ awaits you  ✧ asked you a question  ○ resting  · departed"
    usage_newest; tele=$(usage_text)
    [ -n "$tele" ] && say "  usage  $tele   ($(fmt_age $((now - U_at))) ago)"
  fi
  n=$(find "$STATE_DIR/departed" -type f 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" -gt 0 ] && say "  $n from earlier runs can be recalled: gensokyo resume"
  if [ -n "$wait" ]; then printf '\n  any key to close '; read -r -s -n 1 _ 2>/dev/null; fi
  return 0
}

# focus <name|slot>: bring that resident's window (its iTerm2 tab) to the front.

# ---------------------------------------------------------------- resident pane
# `gensokyo _run <session-id>` is what runs inside a resident's pane.
cmd__run() {
  local id=$1 rec=$RES_DIR/$1 cwd name args prompt rc resume
  [ -f "$rec" ] || die "no resident record for $id"
  cwd=$(rec_get "$rec" cwd); name=$(rec_get "$rec" name)
  args=$(rec_get "$rec" args); prompt=$(rec_get "$rec" prompt); resume=$(rec_get "$rec" resume)
  # The only writer of `pane` and `window`: this runs inside them, and it runs again when a
  # departed resident is recalled in place, so both follow the resident wherever it lands.
  rec_set "$rec" pane "${TMUX_PANE:-}"
  rec_set "$rec" window "$(tmux_ display -p -t "${TMUX_PANE:-}" '#{window_id}' 2>/dev/null)"
  rec_del "$rec" departed
  rm -f "$STATE_DIR/status/$id"   # a fresh launch waits for nothing yet
  cd "$cwd" || die "cannot cd to $cwd"
  scrub_env
  # The handbook skill runs the CLI; from a checkout `gensokyo` is not on PATH, so pass it.
  export GENSOKYO_BIN=$SELF GENSOKYO_RESIDENT=$id
  # Ctrl-C in the pane must reach claude (it clears the input) without killing this wrapper.
  # A trap with a command is reset to the default in the child, so claude sees nothing special.
  trap : INT
  eval "set -- $args"
  # Every resident, recalled or new, gets the hooks and status line wrapper (lib/hooks.sh,
  # lib/telemetry.sh), the plugin (handbook skill) and the short paragraph that makes Claude
  # reach for the skill instead of guessing; all are per-session flags, nothing in ~/.claude
  # changes.
  set -- --settings "$(launch_settings "$id" "$cwd")" --plugin-dir "$SHARE/plugin" \
    --append-system-prompt "$(system_paragraph "$name")" "$@"
  if [ -n "$resume" ]; then
    # Recall: same session id and transcript; Claude Code keeps the name it had.
    claude_ --resume "$id" "$@"
  elif [ -n "$prompt" ]; then
    # `--` is mandatory before the prompt: --allowedTools is variadic and would swallow it.
    claude_ --session-id "$id" --name "$name" "$@" -- "$prompt"
  else
    claude_ --session-id "$id" --name "$name" "$@"
  fi
  rc=$?
  rec_set "$rec" departed "$(date +%s)"
  rec_set "$rec" exit "$rc"
  name=$(rec_get "$rec" name)   # follows a /rename made while it ran
  departed_screen "$id" "$name" "$rc"
}

# One paragraph appended to Claude Code's system prompt. Kept short: everything else lives in
# share/plugin/skills/gensokyo/SKILL.md, which Claude loads when a request matches it.
system_paragraph() {
  printf '%s' "You are running inside gensokyo, a tmux cockpit that runs several Claude Code sessions (residents) side by side on this machine; your resident name is $1. The other sessions in \`claude agents\` are residents too and you can message them with SendMessage by name. When the user talks about other sessions or residents, or wants to start, list, close, resume or focus one (also with the words summon, who, banish, recall), use the gensokyo skill and its CLI (\`gensokyo\`, or \$GENSOKYO_BIN when that is not on PATH); do not drive tmux yourself."
}

# What the resident's own tab shows once it has left: the same buttons the shrine draws, drawn by
# the same walk and clicked the same way (lib/shrine.sh). Claude Code runs in the alternate screen,
# so the pane is back to what it held before the resident started and there is nothing here to
# keep. INT is already ignored, set before claude was started.
departed_screen() {
  local id=$1 name=$2 rc=$3 cols hit
  cols=$(tmux_ display -p -t "${TMUX_PANE:-}" '#{pane_width}' 2>/dev/null)
  case $cols in ''|*[!0-9]*) cols=80 ;; esac
  SHRINE_TEXT='' SHRINE_MAP='' SHRINE_ROW=0 SHRINE_COLS=$cols
  shrine_line ''
  shrine_line ''
  shrine_line "  ⛩  $name has left the shrine. (exit $rc)"
  shrine_line ''
  shrine_draw_buttons departed_buttons
  shrine_line ''
  shrine_line '  click a button, or press its letter'
  printf '\033[2J\033[?25l\033[?1000h\033[?1006h'
  trap 'printf "\033[?1006l\033[?1000l\033[?25h"' EXIT
  trap shrine_paint WINCH
  shrine_paint
  # Without a terminal there is nobody to read: wait to be closed rather than spinning on EOF.
  [ -t 0 ] || while :; do sleep 3600; done
  while :; do
    shrine_event
    if [ -n "$SHRINE_CLICK" ]; then
      hit=$(shrine_hit "${SHRINE_CLICK%% *}" "${SHRINE_CLICK##* }")
      hit=${hit%%|*}
    else
      hit=''
      case $SHRINE_KEY in r|R) hit=recall ;; x|X|q|Q) hit=close ;; esac
    fi
    case $hit in
      recall)
        rec_set "$RES_DIR/$id" resume 1
        printf '\033[?1006l\033[?1000l\033[?25h'   # exec leaves no EXIT trap to do it
        exec "$SELF" _run "$id" ;;
      close) close_pane "$id" ;;
    esac
  done
}

# Remove the record and let the pane go, which takes its window (its tab) with it. The
# shrine has a window of its own and never goes, so the tmux server survives the last
# resident leaving with nothing to hand over.
close_pane() {
  drop_record "$RES_DIR/$1"
  exit 0
}
