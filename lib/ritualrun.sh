# lib/ritualrun.sh - firing a ritual: what has come round, whether the last run is still going,
# and the resident that runs it. lib/rituals.sh reads the file; this decides and acts.
# Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# Everything a ritual keeps between runs lives in one directory of its own, named by the
# ritual: the memory file the prompt points at, the stamp that says which minute it last ran
# for, the log a skipped or refused run leaves behind, and the prompt as it was last sent.
ritual_dir() { printf '%s\n' "$STATE_DIR/rituals/$1"; }

# The minute a ritual last ran for, as an epoch already floored to the minute - not a
# %Y%m%d%H%M key, because then "has it run this minute" is integer arithmetic with no `date`
# to fork (the sweep asks it of every ritual every 20 s) and catch-up compares it directly.
ritual_stamp() { sed -n 1p "$(ritual_dir "$1")/last-run" 2>/dev/null; }
ritual_stamp_set() {
  local d
  d=$(ritual_dir "$1"); mkdir -p "$d" || return 1
  printf '%s\n' "$2" > "$d/last-run.tmp.$$" && mv "$d/last-run.tmp.$$" "$d/last-run"
}

# ritual_note <slug> <text>: one line in the ritual's log, which is where a skipped run, a
# refused one and every fire are written down. `ritual log` reads it back; nothing else does,
# so it is a plain journal and not a format.
ritual_note() {
  local d
  d=$(ritual_dir "$1"); mkdir -p "$d" || return 1
  printf '%s\t%s\n' "$(now_epoch)" "$2" >> "$d/log"
}

# ritual_notify <slug> <text>: the news about a ritual rather than about a resident - a run
# that could not start, a line gensokyo cannot read. The ⏲ is in the text because the desktop
# alert has no glyph of its own, and there is no bell: nothing here has a pane to ring.
ritual_notify() {
  local text="⏲ $1: $2"
  toast '' "$text"
  [ "$CFG_NOTIFY_DESKTOP" = on ] && desktop_notify "$1" "$text"
  return 0
}

# ritual_complain <slug> <text>: say what is wrong with a ritual, once. A ritual with a bad
# line is read again every sweep, and the alert would be too; the last complaint is kept and a
# repeat of it goes to the log alone. The stamp is deliberately not touched: a ritual that could
# not run has not run, and spending the minute on it would make the log read as though it had.
ritual_complain() {
  local d
  d=$(ritual_dir "$1"); mkdir -p "$d" || return 1
  [ "$2" = "$(cat "$d/complained" 2>/dev/null)" ] && return 0
  printf '%s\n' "$2" > "$d/complained"
  ritual_note "$1" "not run: $2"
  ritual_notify "$1" "$2"
}
ritual_complaint_clear() { rm -f "$(ritual_dir "$1")/complained"; return 0; }

# ritual_memory <slug>: the file the prompt sends the resident to, made if it is not there. A
# fresh session per run forgets everything; this is where the continuity lives, so a run should
# find a file rather than a missing path.
ritual_memory() {
  local d
  d=$(ritual_dir "$1"); mkdir -p "$d" || return 1
  [ -f "$d/memory.md" ] || printf '# %s\n\nNotes kept across runs of this ritual.\n' "$1" > "$d/memory.md"
  printf '%s\n' "$d/memory.md"
}

# ritual_prompt_text: the ritual's prompt with the memory file named at the end of it. A fresh
# session per run has a clean context and forgets everything; the file is where the continuity
# lives, and the sentence naming it is sent with every prompt whether the ritual mentions it or
# not, so a ritual nobody wrote a memory instruction for still keeps its notes.
ritual_prompt_text() {
  printf '%s\n\nYour notes from previous runs are at `%s`. Read them first; update them before you finish.\n' \
    "$RIT_prompt" "$(ritual_memory "$RIT_slug")"
}

# ritual_mode: the permission mode a run gets - the ritual's own, else whatever a resident
# summoned by hand would get, so one setting in the config still governs both.
ritual_mode() { printf '%s' "${RIT_mode:-$CFG_PERMISSION_MODE}"; }

# ritual_args: the launch flags for the record, shell-quoted the way cmd_new writes them
# (lib/residents.sh eval's this back apart inside the pane). --allowedTools is variadic and
# comes last of these, which is safe because _run always puts the prompt behind a `--`.
ritual_args() {
  local out='' t mode
  mode=$(ritual_mode)
  [ -n "$RIT_model" ]  && out="$out --model $(sq "$RIT_model")"
  [ -n "$RIT_effort" ] && out="$out --effort $(sq "$RIT_effort")"
  [ -n "$mode" ]       && out="$out --permission-mode $(sq "$mode")"
  [ -n "$RIT_mcp" ]    && out="$out --mcp-config $(sq "${RIT_mcp/#\~/$HOME}")"
  if [ -n "$RIT_allowed" ]; then
    out="$out --allowedTools"
    while IFS= read -r t || [ -n "$t" ]; do
      [ -n "$t" ] && out="$out $(sq "$t")"
    done <<EOF
$RIT_allowed
EOF
  fi
  printf '%s' "${out# }"
}

# ritual_running <slug>: whether the last run of that ritual is still going, which is what
# `overlap: skip` skips for. Still *going*, not still *there*: a run that has finished sits in
# its tab until the owner closes it, and a ritual whose finished run counted as busy would fire
# exactly once and never again. The status file is the same one the chips are drawn from - it is
# removed when a resident launches and says `stopped` when the Stop hook fires - so a run
# mid-turn and a run stuck on a permission prompt both count, and a finished one does not.
ritual_running() {
  local f live id
  live=$'\n'$(live_panes)$'\n'
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    rec_load "$f"
    [ "$R_ritual" = "$1" ] || continue
    [ -z "$R_departed" ] || continue
    id=${f##*/}
    # No pane yet: a launch from a moment ago that has not reached `_run`. prune_records gives
    # such a record 30 s before it counts as lost, and until then it is a run starting.
    [ -n "$R_pane" ] || return 0
    case $live in *$'\n'"$R_pane"$'\n'*) ;; *) continue ;; esac
    status_load "$id"
    [ "$S_pending" = stopped ] && continue
    return 0
  done
  return 1
}

# ritual_reason <catch-up allowed>: the minute this ritual should run for and why - "due
# <epoch>" for the minute that has just come round, "catch-up <epoch>" for the most recent one
# missed - or nothing, which is the answer almost every sweep gets. Pure: it reads the loaded
# RIT_*, the stamp and the clock, and changes nothing.
RITUAL_CATCH_UP_DAYS=7
ritual_reason() {
  local catch=$1 now this stamp miss
  now=$(now_epoch); this=$((now - now % 60))
  stamp=$(ritual_stamp "$RIT_slug")
  if cron_match "$RIT_schedule" "$this"; then
    # Fires are idempotent per minute: the sweep comes round several times inside one minute,
    # and the stamp is what keeps that one fire.
    [ "$stamp" = "$this" ] && return 1
    printf 'due %s\n' "$this"
    return 0
  fi
  [ -n "$catch" ] || return 1
  rit_bool "$RIT_catch_up" || return 1
  # A ritual gensokyo has never seen does not run for a minute that passed before it existed:
  # writing the file at 10:00 must not fire this morning's 09:00. The sweep stamps a ritual the
  # first time it sees one, so a miss is only a miss from then on.
  [ -n "$stamp" ] || return 1
  miss=$(cron_prev "$RIT_schedule" "$this" "$RITUAL_CATCH_UP_DAYS") || return 1
  [ "$miss" -gt "$stamp" ] || return 1
  printf 'catch-up %s\n' "$miss"
}

# ritual_launch: the resident that does the work. The same record cmd_new writes, plus the two
# fields a run needs: `ritual`, which is how the notifications, the overlap check and the
# cockpit know whose run this is, and `prompt_file`, because a ritual's prompt is many lines
# and a record holds one line per key. `launched` is now and not the minute the run is for:
# every reader of that field takes it for the time the resident started - `list` and the recall
# rows print it, and prune_records drops a record that has no pane 30 s after it - so a
# catch-up run stamped with a minute from last week would be born prunable.
ritual_launch() {
  local id slot rec name mode pf
  ensure_dirs
  pf=$(ritual_dir "$RIT_slug")/prompt
  mkdir -p "$(ritual_dir "$RIT_slug")" || return 1
  ritual_prompt_text > "$pf" || return 1
  # The ritual's own name, so its tab reads like the job it is doing. A resident name has to
  # start with a letter (a leading digit is a slot), and a departed run may still be holding it,
  # in which case the run gets a placeholder name and the record still says which ritual it is.
  name=$RIT_slug
  case $name in [A-Za-z]*) ;; *) name='' ;; esac
  [ -n "$name" ] && find_resident "$name" >/dev/null 2>&1 && name=''
  [ -n "$name" ] || name=$(pick_name)
  mode=$(ritual_mode)
  id=$(new_uuid); slot=$(next_slot); rec=$RES_DIR/$id
  {
    printf 'slot=%s\nname=%s\ncwd=%s\nlaunched=%s\nargs=%s\nritual=%s\nprompt_file=%s\n' \
      "$slot" "$name" "$RIT_cwd" "$(now_epoch)" "$(ritual_args)" "$RIT_slug" "$pf"
    [ -n "$mode" ] && printf 'mode=%s\n' "$mode"
  } > "$rec"
  # `back` rather than a plain summon: a ritual fires while the owner is working in another tab,
  # and iTerm2 moves to a new tab whether it was asked to or not (lib/residents.sh).
  open_pane "$id" "$(resident_title "$slot" starting "$name")" back >/dev/null \
    || { rm -f "$rec"; return 1; }
  return 0
}

# ritual_sweep [catch-up]: one pass over the rituals, which is what the clock calls. With an
# argument it also looks for a fire missed while nothing was running - a machine that slept, a
# cockpit that was down - which the clock asks for at its first tick and after a gap in its own
# ticking, and which is otherwise not worth a walk backwards through the schedule every 20 s.
#
# The order of the checks is the order of what they cost. Reading a file is cheap and matching
# a minute is one `date`; ritual_problem walks the schedule forward to prove it comes round at
# all, so it is asked only about a ritual that is actually about to run - a `0 0 30 2 *` would
# otherwise pay for the full four-year search every sweep, inside the loop that draws the bar.
ritual_sweep() {
  local catch=${1:-} f reason why when problem
  # A quit is under way: the panes are going one after another and the server with them, so a
  # run summoned now would be a tab in a cockpit that is being taken down - and the stamp would
  # spend the minute, leaving the next start with nothing to catch up either.
  quit_in_progress && return 0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    ritual_load "$f" || continue
    ritual_enabled || continue
    if ! cron_ok "$RIT_schedule"; then
      # The one problem no due-check can ever surface: an unreadable schedule matches nothing,
      # so without this the ritual would sit there for good and never say why.
      ritual_complain "$RIT_slug" "schedule: $RIT_schedule is not one gensokyo can read"
      continue
    fi
    reason=$(ritual_reason "$catch") || { ritual_seen; continue; }
    problem=$(ritual_problem)
    if [ -n "$problem" ]; then ritual_complain "$RIT_slug" "$problem"; continue; fi
    ritual_complaint_clear "$RIT_slug"
    why=${reason%% *}; when=${reason#* }
    # The stamp goes down before the run starts, not after: opening a window takes long enough
    # for the next sweep to arrive inside the same minute, and a fire is once per minute.
    ritual_stamp_set "$RIT_slug" "$when"
    if ritual_running "$RIT_slug"; then
      ritual_note "$RIT_slug" "skipped ($why $(ritual_when "$when")): the last run is still going"
      continue
    fi
    if ritual_launch; then
      ritual_note "$RIT_slug" "ran ($why $(ritual_when "$when"))"
    else
      ritual_note "$RIT_slug" "not run: tmux could not open a window"
      ritual_notify "$RIT_slug" 'could not open a window for the run'
    fi
  done <<EOF
$(ritual_files)
EOF
  return 0
}

# ritual_seen: the stamp a ritual gets the first time the sweep sees it and finds nothing due.
# From here on the schedule's own minutes are what catch-up measures against; before it there
# is nothing to say whether a minute that has passed was ever gensokyo's to run.
ritual_seen() {
  local now
  [ -n "$(ritual_stamp "$RIT_slug")" ] && return 0
  now=$(now_epoch)
  ritual_stamp_set "$RIT_slug" "$((now - now % 60))"
}

# ritual_when <epoch>: a minute as the log and the listings write it.
ritual_when() { date -r "$1" '+%Y-%m-%d %H:%M' 2>/dev/null; }
