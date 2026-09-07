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

# open_pane <session-id> <title> [focus|back]: a window of its own, one pane, running
# `_run <session-id>`; prints the window id. `focus` is 1 to go to it, `back` to come back from
# it whoever asked (what a ritual firing wants). One resident per window is the whole layout -
# gensokyo never splits, tiles or zooms, because every tmux window is an iTerm2 tab and the
# tab bar is the sidebar. No `-t <index>`: iTerm2 rewrites window indexes to match the tab
# bar as soon as the user drags a tab, so the index is the user's and the id is ours.
open_pane() {
  local id=$1 title=$2 focus=${3:-} cmd win here
  cmd="$(sq "$SELF") _run $id"
  # A named target: the clock fires rituals from a `run-shell -b` job, which has no window of
  # its own for an untargeted `display -p` to mean.
  here=$(tmux_ display -p -t "=$SESSION:" '#{window_id}' 2>/dev/null)
  win=$(tmux_ new-window -d -P -F '#{window_id}' -n "$title" "$cmd") || return 1
  if [ "$focus" = 1 ]; then
    focus_window "$win"
  elif [ -n "$here" ] && [ "$here" != "$win" ] \
       && { [ "$focus" = back ] || inside_own_server; } && [ "$(clients_in_mode cc)" -gt 0 ]; then
    # iTerm2 opens the tab and moves to it even for a detached `new-window -d`. That is a
    # rude interruption when the summon came from inside the cockpit - a resident asking for
    # a helper should not rip the user out of the tab they were reading - so focus goes back
    # to where it was. Selecting back at once loses the race with the tab opening; a moment's
    # wait wins it, at the price of a visible flick. A summon typed in a shell outside the
    # cockpit is the other way round: the user asked for that resident and wants to be in it,
    # so the steal is left alone. A ritual asks for `back` either way: it fires on its own
    # schedule, into whatever the owner was doing, and never gets to take the keyboard for it.
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
  local a b n=0 still=0
  nap 0.3
  a=$(tmux_ capture-pane -p -t "$1" 2>/dev/null)
  while [ "$n" -lt 20 ]; do
    nap 0.15
    b=$(tmux_ capture-pane -p -t "$1" 2>/dev/null)
    if [ "$a" = "$b" ]; then
      still=$((still + 1))
      [ "$still" -ge 3 ] && return 0
    else
      still=0
    fi
    a=$b; n=$((n + 1))
  done
  return 0
}

# The two halves of asking a resident to leave, in the order they have to happen: Ctrl-C clears
# a half-typed prompt (or interrupts the running turn) so that /exit lands on an empty line,
# pane_settled waits for that to arrive, and `send-keys -l` then Enter submits reliably. They are
# separate because `quit` sends every interrupt first, so that nobody waits for the resident
# before it; `close`, which has one resident to ask, takes them together in ask_leave below.
ask_interrupt() { [ -n "$1" ] || return 1; tmux_ send-keys -t "$1" C-c; }
ask_exit()      { [ -n "$1" ] || return 1; tmux_ send-keys -t "$1" -l '/exit' \; send-keys -t "$1" Enter; }

# EXIT_WAIT: how long one resident gets to act on one /exit. Claude Code is gone within a second
# of reading it, so this is slack for a loaded machine and for a resident finishing a tool call,
# not for one that is thinking: what runs out of it is asked a second time, not given longer.
EXIT_WAIT=6

# departed_within <session-id> <seconds>: true once that record says the resident has gone.
# The record is the only proof that a /exit was read - cmd__run writes `departed` the moment
# claude returns - and the pane's own text is not: a recalled resident still has the last
# departed screen in its scrollback, which a capture matches just as happily.
departed_within() {
  local limit
  limit=$(( $(date +%s) + $2 ))
  while :; do
    [ -n "$(rec_get "$RES_DIR/$1" departed 2>/dev/null)" ] && return 0
    [ "$(date +%s)" -ge "$limit" ] && return 1
    nap 0.3
  done
}

# ask_leave <pane> <session-id>: the whole gesture, and whether it was heard. Ctrl-C clears a
# half-typed prompt or interrupts the turn, pane_settled waits for that to arrive so the /exit
# does not lose its first characters to it, and the record says whether claude read it.
ask_leave() {
  ask_interrupt "$1"
  pane_settled "$1"
  ask_exit "$1"
  departed_within "$2" "$EXIT_WAIT"
}

cmd_close() {
  local f id
  [ -n "${1:-}" ] || die "usage: gensokyo close <name|slot>"
  f=$(find_resident "$1") || die "close: no resident '$1' (gensokyo list)"
  rec_load "$f"; id=${f##*/}
  # A pane that has nothing left in it is closed here, rather than by sending `x` to the
  # departed screen and trusting its input loop to act on it. `close` returns to whoever
  # called it - the shrine's banish, the CLI, a script - and must not report work it has only
  # asked somebody else to do: under load that keystroke can still be unread when the caller
  # looks, leaving the record, its window and its slot behind. The departed screen keeps its
  # own `[ close ]` button doing the same two things through `close_pane`, so a click and a
  # command each finish their own work and neither waits on the other. Both remove the side
  # files with the record, which a bare `rm` on this path used to leave behind.
  if [ -n "$R_departed" ]; then
    drop_record "$f"; tmux_ kill-pane -t "$R_pane" 2>/dev/null
    say "closed $R_name's pane"
  elif [ "$(tmux_ display -p -t "$R_pane" '#{pane_dead}' 2>/dev/null)" = 1 ]; then
    drop_record "$f"; tmux_ kill-pane -t "$R_pane" 2>/dev/null
    say "closed $R_name's dead pane"
  else
    # A keystroke is not a message: send-keys says tmux wrote it, never that the resident read
    # it, and one that goes missing leaves a resident nothing can shift - this used to send the
    # /exit, report that it had asked and return, with claude still sitting in the tab. So the
    # record is watched, and the whole gesture repeated once if nothing comes of it.
    #
    # The whole gesture, not the /exit on its own: the interrupt is there to clear a half-typed
    # prompt, and a lost keystroke can leave one (part of the last /exit still in the box), which
    # a second /exit typed after it would only lengthen into something that matches nothing.
    #
    # Repeating is safe because the departed screen ignores a slash command (shrine_event): a
    # resident that leaves between the check and the keystroke reads that /exit at that screen,
    # whose close key is the `x` in the middle of it, and it used to take the pane and the record
    # with it. That race is milliseconds wide and it happened, so the guard is there and not here.
    if ask_leave "$R_pane" "$id" || ask_leave "$R_pane" "$id"; then
      say "$R_name has left (/exit); the tab shows the departed screen"
    else
      warn "$R_name did not answer /exit in ${EXIT_WAIT}s, twice; its tab is still open (try again, or /exit in it)"
      return 1
    fi
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
  local id=$1 rec=$RES_DIR/$1 cwd name args prompt rc resume pfile
  [ -f "$rec" ] || die "no resident record for $id"
  cwd=$(rec_get "$rec" cwd); name=$(rec_get "$rec" name)
  args=$(rec_get "$rec" args); prompt=$(rec_get "$rec" prompt); resume=$(rec_get "$rec" resume)
  # A ritual's prompt is a file: it runs to several lines, and a record key holds one.
  pfile=$(rec_get "$rec" prompt_file)
  [ -n "$prompt" ] || [ -z "$pfile" ] || prompt=$(cat "$pfile" 2>/dev/null)
  # The only writer of `pane` and `window`: this runs inside them, and it runs again when a
  # departed resident is recalled in place, so both follow the resident wherever it lands.
  rec_set "$rec" pane "${TMUX_PANE:-}"
  rec_set "$rec" window "$(tmux_ display -p -t "${TMUX_PANE:-}" '#{window_id}' 2>/dev/null)"
  rec_del "$rec" departed
  rm -f "$STATE_DIR/status/$id"   # a fresh launch waits for nothing yet
  cd "$cwd" || die "cannot cd to $cwd"
  scrub_env
  # Nothing in the resident reads GENSOKYO_BIN today - the hooks and the status line are given
  # absolute paths in --settings - but the ritual skill will run the CLI, and from a checkout
  # `gensokyo` is not on PATH. GENSOKYO_RESIDENT is what tells the CLI it is being run from
  # inside a pane it owns.
  export GENSOKYO_BIN=$SELF GENSOKYO_RESIDENT=$id
  # Ctrl-C in the pane must reach claude (it clears the input) without killing this wrapper.
  # A trap with a command is reset to the default in the child, so claude sees nothing special.
  trap : INT
  eval "set -- $args"
  # Every resident, recalled or new, gets the hooks and status line wrapper (lib/hooks.sh,
  # lib/telemetry.sh), the plugin - which carries no skill until there is one worth a resident's
  # context - and the three-sentence paragraph; all are per-session flags, nothing in ~/.claude
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

# One paragraph appended to Claude Code's system prompt, and three sentences is all of it. A
# resident's context window is the user's budget, so nothing is said here that a click already
# does: starting, closing, recalling and switching between residents are buttons and tabs, and a
# resident that never hears of them cannot spend a turn on them. What is left is what no button
# can reach - the resident's own name, that the others can be written to, and where a standing
# schedule goes. The last sentence is the one that has to be here: without it a request like
# "every weekday at 9..." goes to Claude Code's own scheduling, which gensokyo can neither see
# nor stop. `gensokyo-ritual` does not ship yet, so the sentence says so rather than sending the
# resident after something that is not there.
#
# The clause pointing at `gensokyo-peers` is here because the skill does not fire without it,
# measured 2026-09-07: told only that it can message the others by name, a resident asked to
# "get Youmu to review the uncommitted diff" does exactly that and no more - no reply channel
# named, so the review went into Youmu's own pane and never came back. The skill was loaded and
# listed (`gensokyo:gensokyo-peers`); it simply lost to the simpler instruction sitting right
# there. A skill that describes how to do something the system prompt already grants has to be
# named at the point the grant is made, or it is never reached for.
system_paragraph() {
  printf '%s' "You are running inside gensokyo, a cockpit that runs several Claude Code sessions (residents) side by side on this machine; your resident name is $1. The other sessions in \`claude agents\` are residents too and you can message them with SendMessage by name, but never compose the first message of an exchange you want an answer to yourself: use the \`gensokyo-peers\` skill to write it. For any standing or repeating schedule use the \`gensokyo-ritual\` skill and never the built-in \`schedule\` skill, CronCreate or scheduled tasks; that skill is not built yet, so until it is, tell the user to run \`gensokyo ritual new <name>\` themselves rather than scheduling anything for them."
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
