# lib/residents.sh - the resident commands (new, close, list, focus, stage) and the wrapper
# that runs inside a resident's pane. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

cmd_new() {
  local dir='' name='' model='' effort='' mode='' focus='' prompt='' flags=() argstr='' f
  local id slot stage win pane cmd rec out n shrine
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
  id=$(new_uuid); slot=$(next_slot); stage=$(stage_of_slot "$slot"); rec=$RES_DIR/$id
  {
    printf 'slot=%s\nname=%s\ncwd=%s\nwindow=%s\nlaunched=%s\nargs=%s\n' "$slot" "$name" "$dir" "$stage" "$(date +%s)" "${argstr# }"
    [ -n "$prompt" ] && printf 'prompt=%s\n' "$prompt"
    [ -n "$mode" ] && printf 'mode=%s\n' "$mode"   # shown until the first prompt reports the live mode
  } > "$rec"

  # Stage windows hold CFG_STAGE_SIZE residents each. The first resident of a stage replaces
  # the shrine pane in place; later ones split the window and re-tile it.
  cmd="$(sq "$SELF") _run $id"
  win=$(find_window "$stage")
  if [ -z "$win" ]; then
    out=$(tmux_ new-window -d -P -F '#{window_id} #{pane_id}' -n "$stage" "$cmd") || { rm -f "$rec"; die "new: tmux could not open a window"; }
    pane=${out#* }
  else
    n=$(tmux_ list-panes -t "$win" | wc -l | tr -d ' ')
    shrine=$(tmux_ list-panes -t "$win" -F '#{pane_id} #{@shrine}' | awk '$2 == "1" {print $1; exit}')
    if [ "$n" -eq 1 ] && [ -n "$shrine" ]; then
      tmux_ set -p -t "$shrine" -u @shrine \; respawn-pane -k -t "$shrine" "$cmd" || { rm -f "$rec"; die "new: tmux could not start the pane"; }
      pane=$shrine
    else
      pane=$(tmux_ split-window -d -P -F '#{pane_id}' -t "$win" "$cmd") || { rm -f "$rec"; die "new: tmux could not split the window"; }
      tmux_ select-layout -t "$win" tiled
    fi
  fi
  rec_set "$rec" pane "$pane"
  remember_dir "$dir"
  [ -n "$focus" ] && focus_pane "$pane"
  say "summoned $name (slot $slot) in $dir"
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
    perl -e 'select(undef, undef, undef, 0.3)' 2>/dev/null || sleep 1
    tmux_ send-keys -t "$R_pane" -l '/exit' \; send-keys -t "$R_pane" Enter
    say "asked $R_name to leave (/exit); the pane shows the departed screen once claude exits"
  fi
}

cmd_list() {
  local json='' all='' wait='' rows='' id status name cwd slot state pane win mode detail now tele
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
  if [ -n "$wait" ]; then printf '\n  any key to close '; read -r -s -n 1 _ 2>/dev/null; fi
  return 0
}

# stage [layout]: re-arrange every stage window (tiled, main-vertical, even-horizontal, ...).
# focus <name|slot>: show that resident's pane (the skill's "zoom on Marisa").
cmd_focus() {
  local f
  [ -n "${1:-}" ] || die "usage: gensokyo focus <name|slot>"
  f=$(find_resident "$1") || die "focus: no resident '$1' (gensokyo list)"
  rec_load "$f"
  [ -n "$R_pane" ] && [ "$R_pane" != - ] || die "focus: $R_name has no pane yet"
  server_running || die "focus: the cockpit is not running"
  focus_pane "$R_pane"
  say "focused $R_name (slot $R_slot)"
}

cmd_stage() {
  local layout=${1:-} w
  if [ -z "$layout" ]; then
    tmux_ list-windows -t "=$SESSION" -F '#{window_name}: #{window_panes} pane(s)#{?window_zoomed_flag, (zoomed),}' 2>/dev/null \
      || die "stage: the cockpit is not running"
    return 0
  fi
  for w in $(tmux_ list-windows -t "=$SESSION" -F '#{window_id}' 2>/dev/null); do
    tmux_ select-layout -t "$w" "$layout" 2>/dev/null || die "stage: unknown layout '$layout' (tiled, main-vertical, main-horizontal, even-horizontal, even-vertical)"
    tmux_ set -w -t "$w" @layout "$layout"
  done
  say "stage: $layout"
}

# ---------------------------------------------------------------- resident pane
# `gensokyo _run <session-id>` is what runs inside a resident's pane.
cmd__run() {
  local id=$1 rec=$RES_DIR/$1 cwd name args prompt rc resume
  [ -f "$rec" ] || die "no resident record for $id"
  cwd=$(rec_get "$rec" cwd); name=$(rec_get "$rec" name)
  args=$(rec_get "$rec" args); prompt=$(rec_get "$rec" prompt); resume=$(rec_get "$rec" resume)
  rec_set "$rec" pane "${TMUX_PANE:-}"
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

departed_screen() {
  local id=$1 name=$2 rc=$3 key
  printf '\n\n  ⛩  %s has left the shrine. (exit %s)\n\n  [r] recall   [x] close this pane\n\n' "$name" "$rc"
  while :; do
    read -r -s -n 1 key 2>/dev/null || { sleep 3600; continue; }
    case $key in
      r|R) rec_set "$RES_DIR/$id" resume 1; exec "$SELF" _run "$id" ;;
      x|X|q|Q) close_pane "$id" ;;
    esac
  done
}

# Remove the record and let the pane go; if it is the last pane of the last stage,
# become the shrine instead so the tmux server survives.
close_pane() {
  local id=$1 panes
  drop_record "$RES_DIR/$id"
  panes=$(tmux_ list-panes -s -F '#{pane_id}' 2>/dev/null | wc -l | tr -d ' ')
  if [ "${panes:-0}" -le 1 ]; then exec "$SELF" _shrine; fi
  exit 0
}
