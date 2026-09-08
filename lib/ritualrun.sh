# lib/ritualrun.sh - firing a ritual: what has come round, whether the last run is still going,
# and the resident that runs it. lib/rituals.sh reads the file; this decides and acts.
# Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# Everything a ritual keeps between runs lives in one directory of its own, named by the
# ritual: the memory file the prompt points at, the stamp that says which minute it last ran
# for, the log a skipped or refused run leaves behind, the prompt as it was last sent, a copy of
# that prompt per run for the pane that is about to read it, and the fire waiting its turn.
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
#
# `target: <name>` is the one kind that gets the prompt and nothing else. That resident was
# summoned by hand, so it has no `--add-dir` for the ritual's directory and reading the file
# would be a permission prompt every single morning - and it has none of the forgetting the
# memory file exists for: one ongoing conversation of its own is why somebody named it.
ritual_prompt_text() {
  case $RIT_target in
    new|persistent)
      printf '%s\n\nYour notes from previous runs are at `%s`. Read them first; update them before you finish.\n' \
        "$RIT_prompt" "$(ritual_memory "$RIT_slug")" ;;
    *) printf '%s\n' "$RIT_prompt" ;;
  esac
}

# ritual_mode: the permission mode a run gets - the ritual's own, else whatever a resident
# summoned by hand would get, so one setting in the config still governs both.
ritual_mode() { printf '%s' "${RIT_mode:-$CFG_PERMISSION_MODE}"; }

# ritual_args: the launch flags for the record, shell-quoted the way cmd_new writes them
# (lib/residents.sh eval's this back apart inside the pane). --allowedTools is variadic and
# comes last of these, which is safe because _run always puts the prompt behind a `--`.
#
# --add-dir is always the ritual's own directory, because the memory file lives there and every
# prompt ends with a sentence naming it. Without it, the run has to ask permission to *read* a
# file gensokyo put outside its working directory and the ritual never asked for - measured, and
# worse than the same file inside the repo would be. With it the file reads freely and a write
# asks exactly as a write in the working directory would, which is the ritual's own
# `mode:` / `allowed_tools` decision rather than an accident of where gensokyo keeps its state.
ritual_args() {
  local out='' t mode
  mode=$(ritual_mode)
  [ -n "$RIT_model" ]  && out="$out --model $(sq "$RIT_model")"
  [ -n "$RIT_effort" ] && out="$out --effort $(sq "$RIT_effort")"
  [ -n "$mode" ]       && out="$out --permission-mode $(sq "$mode")"
  [ -n "$RIT_mcp" ]    && out="$out --mcp-config $(sq "${RIT_mcp/#\~/$HOME}")"
  out="$out --add-dir $(sq "$(ritual_dir "$RIT_slug")")"
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

# ritual_busy <slug>: a run of it going right now, of either kind. Both are asked whichever the
# ritual says today: a ritual switched from a pane to `headless: true` (or back) can have a run of
# the other kind still going, and either one is about to write the notes a second run would write
# too. The pane half needs a tmux server to answer, so this is for the sweep, which runs inside
# one; a command with the cockpit down asks ritual_headless_running on its own.
ritual_busy() { ritual_headless_running "$1" && return 0; ritual_running "$1"; }

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

# ---------------------------------------------------------------- overlap: the fire in the way
# What to do about a fire that lands while the last run is still going. `skip` is the default and
# says so in the log; `parallel` starts a second run beside the first; `queue` holds the fire and
# runs it when the ritual is free again.
#
# The queue is one fire deep and the newest one wins, for the same reason catch-up runs once for
# the most recent miss: a run that takes all day would otherwise queue every minute of it and
# then fire them all, which is not what anybody asking for "run it afterwards" wants. And it is
# given a life, because a fire that finally starts hours after its minute is worse than a fire
# that was skipped - the prompt was written for that minute.
RITUAL_QUEUE_LIFE=3600
rit_queue_file() { printf '%s\n' "$(ritual_dir "$1")/queued"; }
rit_queue_set() {
  local d
  d=$(ritual_dir "$1"); mkdir -p "$d" || return 1
  printf '%s\n' "$2" > "$d/queued.tmp.$$" && mv "$d/queued.tmp.$$" "$d/queued"
}
rit_queue_clear() { rm -f "$(ritual_dir "$1")/queued"; return 0; }

# rit_queue_due <slug>: "queued <epoch>" for a fire whose turn has come, and nothing at all
# otherwise - nothing queued, the run it is waiting behind still going, or a fire too old to be
# worth running now, which is dropped here with a line saying so.
rit_queue_due() {
  local when now
  when=$(sed -n 1p "$(rit_queue_file "$1")" 2>/dev/null)
  case ${when:-x} in ''|*[!0-9]*) return 1 ;; esac
  now=$(now_epoch)
  if [ $((now - when)) -gt "$RITUAL_QUEUE_LIFE" ]; then
    rit_queue_clear "$1"
    ritual_note "$1" \
      "dropped the fire queued for $(ritual_when "$when"): the run in its way took over $(fmt_age "$RITUAL_QUEUE_LIFE")"
    return 1
  fi
  # Asked here rather than left to the sweep: a queued fire reported while the ritual is still
  # busy would be queued again by the same sweep, and the log would say so every twenty seconds.
  ritual_busy "$1" && return 1
  printf 'queued %s\n' "$when"
}

# ritual_launch <why>: fire it, whichever way the ritual says. The two ways have almost nothing
# in common - one opens a window and hands the prompt to a resident, the other is a process with
# no terminal - so the decision is made once, here, and both callers just say what the run is for.
ritual_launch() {
  rit_bool "$RIT_headless" && { ritual_launch_headless "${1:-}"; return $?; }
  case $RIT_target in
    new) ritual_launch_pane ;;
    *)   ritual_launch_send "${1:-}" ;;
  esac
}

# ritual_launch_pane: the resident that does the work. The same record cmd_new writes, plus the two
# fields a run needs: `ritual`, which is how the notifications, the overlap check and the
# cockpit know whose run this is, and `prompt_file`, because a ritual's prompt is many lines
# and a record holds one line per key. `launched` is now and not the minute the run is for:
# every reader of that field takes it for the time the resident started - `list` and the recall
# rows print it, and prune_records drops a record that has no pane 30 s after it - so a
# catch-up run stamped with a minute from last week would be born prunable.
ritual_launch_pane() {
  local id slot rec name mode pf keep
  ensure_dirs
  id=$(new_uuid)
  mkdir -p "$(ritual_dir "$RIT_slug")" || return 1
  # Two copies of the prompt, and each has one job. `prompt` is the prompt as it was last sent,
  # for whoever opens the ritual's directory. `prompt.<session>` is this run's own, because the
  # pane reads it later - a moment later, or a slow moment later - and `overlap: parallel` means
  # two runs of one ritual can be starting at once. One file would have the second launch
  # rewriting what the first one's pane has not read yet, and a ritual edited in between would
  # send the wrong prompt into a run already under way. It goes when its record does
  # (drop_side_files), so a run leaves no more behind than it did before.
  pf=$(ritual_dir "$RIT_slug")/prompt.$id
  ritual_prompt_text > "$pf" || return 1
  cp "$pf" "$(ritual_dir "$RIT_slug")/prompt" 2>/dev/null
  # The ritual's own name, so its tab reads like the job it is doing. A resident name has to
  # start with a letter (a leading digit is a slot), and a departed run may still be holding it,
  # in which case the run gets a placeholder name and the record still says which ritual it is.
  name=$RIT_slug
  case $name in [A-Za-z]*) ;; *) name='' ;; esac
  [ -n "$name" ] && find_resident "$name" >/dev/null 2>&1 && name=''
  [ -n "$name" ] || name=$(pick_name)
  mode=$(ritual_mode)
  # How long this run's tab stays, in seconds, decided now and written down - the reaper reads
  # the record and never the ritual file. A ritual edited or deleted this afternoon must not
  # change what happens to a tab that is already open, and a run whose ritual has gone still has
  # a tab somebody has to close. Nothing at all for `forever`, which is a tab that stays.
  # Only a run of its own has a tab whose life `keep` is about. A persistent ritual's session is
  # the ritual, not one run of it, and a reaper that closed its tab two hours after the last
  # answer would be undoing the one thing `persistent` is for.
  keep=''
  [ "$RIT_target" = new ] && keep=$(rit_keep_secs "$RIT_keep")
  slot=$(next_slot); rec=$RES_DIR/$id
  {
    printf 'slot=%s\nname=%s\ncwd=%s\nlaunched=%s\nargs=%s\nritual=%s\nprompt_file=%s\n' \
      "$slot" "$name" "$RIT_cwd" "$(now_epoch)" "$(ritual_args)" "$RIT_slug" "$pf"
    [ -n "$mode" ] && printf 'mode=%s\n' "$mode"
    [ -n "$keep" ] && printf 'keep=%s\n' "$keep"
  } > "$rec"
  # `back` rather than a plain summon: a ritual fires while the owner is working in another tab,
  # and iTerm2 moves to a new tab whether it was asked to or not (lib/residents.sh).
  open_pane "$id" "$(resident_title "$slot" starting "$name")" back >/dev/null \
    || { rm -f "$rec"; return 1; }
  # A persistent ritual keeps one session, and this is it from now on: written after the pane
  # opened, so a launch that failed does not leave the ritual pointing at a session that never was.
  [ "$RIT_target" = persistent ] &&
    printf '%s\n' "$id" > "$(ritual_session_file "$RIT_slug")" 2>/dev/null
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
  local catch=${1:-} f reason why when problem alongside
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
    if reason=$(ritual_reason "$catch"); then
      :
    else
      ritual_seen
      # Nothing is due, which is the one moment a fire held behind a run gets its turn. Only for
      # a ritual that is still queueing: one switched back to `skip` with something waiting has
      # said what it wants, and the file goes with the next launch.
      [ "$RIT_overlap" = queue ] || continue
      reason=$(rit_queue_due "$RIT_slug") || continue
    fi
    problem=$(ritual_problem)
    if [ -n "$problem" ]; then ritual_complain "$RIT_slug" "$problem"; continue; fi
    ritual_complaint_clear "$RIT_slug"
    why=${reason%% *}; when=${reason#* }
    # The stamp goes down before the run starts, not after: opening a window takes long enough
    # for the next sweep to arrive inside the same minute, and a fire is once per minute. A
    # queued fire is left out of that, and it is belt and braces: a fire is only ever queued in
    # the minute it was due, so the stamp already reads that minute and re-stamping it would do
    # nothing today. What it is against is the day that stops being true - a stamp put back to an
    # earlier minute makes catch-up count a run that has already happened as one that was missed,
    # and that is a whole extra run of somebody's ritual for a reason nobody would find.
    [ "$why" = queued ] || ritual_stamp_set "$RIT_slug" "$when"
    alongside=''
    # Only a target that starts a run of its own can have one of its own still going. A prompt
    # sent to a resident that is already there is Claude Code's to queue if that resident is
    # mid-turn, which is why `overlap` is a `new` setting (lib/rituals.sh says so out loud) - and
    # a persistent ritual's session is a record of ours that is busy most of the time it is used.
    if [ "$RIT_target" = new ] && ritual_busy "$RIT_slug"; then
      case $RIT_overlap in
        parallel) alongside=1 ;;
        queue)
          rit_queue_set "$RIT_slug" "$when"
          ritual_note "$RIT_slug" "queued ($why $(ritual_when "$when")): the last run is still going"
          continue ;;
        *)
          ritual_note "$RIT_slug" "skipped ($why $(ritual_when "$when")): the last run is still going"
          continue ;;
      esac
    fi
    # A run starting now is what any waiting fire was waiting for, whichever fire it is: two runs
    # of one ritual an hour apart are not what `queue` was asked for.
    rit_queue_clear "$RIT_slug"
    if ritual_launch "$why $(ritual_when "$when")"; then
      ritual_note "$RIT_slug" \
        "ran ($why $(ritual_when "$when"))${alongside:+, alongside the run that was still going}"
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

# ------------------------------------------------- target: a resident that is already there
# `target: <name>` sends the ritual's prompt to a resident the user manages themselves.
# `target: persistent` gives the ritual one session of its own and sends into that, recalling it
# when it has departed and starting it the first time. Neither is a fresh session per fire, which
# is the whole difference: what they are for is a job that wants one ongoing conversation rather
# than a clean context, and the user should expect compaction and a growing bill.
#
# Typing into somebody's input line is the same act a spell card performs, so it is the same
# code: cast_type holds a multi-line prompt together with bracketed paste and waits for it to
# appear before pressing Enter, and cast_blocked refuses the states where that Enter would answer
# a dialog instead. Both were measured the hard way (lib/spellcards.sh - the one time it went
# wrong the Enter granted a directory trust), and nothing here re-implements either.
ritual_session_file() { printf '%s\n' "$(ritual_dir "$1")/session-id"; }

# ritual_launch_send <why>: hand the fire to a job of the server's. cast_type waits up to five
# seconds for the prompt to appear on screen, and a session that has to be recalled first takes
# as long as claude takes to start - neither can happen inside the clock's own loop.
#
# `run-shell -b` and not the headless run's nohup: this types into a pane, so a cockpit that is
# going away takes the whole point of it with it.
ritual_launch_send() {
  tmux_ run-shell -b "$(sq "$SELF") _send $(sq "$RIT_slug") $(sq "${1:-}") >/dev/null 2>&1"
}

# rit_wait_ready <session-id>: nothing at all once that resident can be typed into, else the
# reason it cannot, as cast_blocked words it. Only "still starting up" is waited out - a session
# that has just been recalled is exactly that for a second or two - because the other two states
# are a dialog somebody has to answer, and a fire is worth reporting rather than sitting behind
# one for a minute.
#
# The registry cache is dropped on each turn of the loop: it has a three-second life for the sake
# of the bar, and a loop that read it would be answering with what was true before the recall.
RITUAL_SEND_WAIT=60
rit_wait_ready() {
  local n=0 why
  while :; do
    rm -f "$REGISTRY"
    load_registry
    why=$(cast_blocked "$1")
    [ -n "$why" ] || return 0
    [ "$why" = 'is still starting up' ] || { printf '%s' "$why"; return 1; }
    [ "$n" -lt "$RITUAL_SEND_WAIT" ] || { printf '%s' "$why"; return 1; }
    nap 1; n=$((n + 1))
  done
}

# rit_session_ready: the persistent ritual's own session, in a pane and ready to be typed into.
# Prints its id; 2 means a fresh one was started and already has the prompt, so there is nothing
# left to send; 1 means it could not be got to, and has said so.
#
# Four states, and they are all ordinary: it is running, it is sitting on its departed screen, it
# belonged to a cockpit that has since stopped, or there has never been one.
rit_session_ready() {
  local id
  id=$(sed -n 1p "$(ritual_session_file "$RIT_slug")" 2>/dev/null)
  if [ -n "$id" ] && [ -f "$RES_DIR/$id" ]; then
    rec_load "$RES_DIR/$id"
    if [ -z "$R_departed" ] && pane_live "$R_pane"; then printf '%s\n' "$id"; return 0; fi
  fi
  if [ -n "$id" ] && { [ -f "$RES_DIR/$id" ] || [ -f "$DEPARTED_DIR/$id" ]; }; then
    # It has left, or its cockpit has, and `resume` is the whole of bringing either back. In a
    # subshell because it reports every refusal with `die`, and this process has no terminal to
    # die to: what would be a message on somebody's screen has to become a line in the journal.
    if ( cmd_resume "$id" >/dev/null 2>&1 ); then printf '%s\n' "$id"; return 0; fi
    ritual_note "$RIT_slug" "not sent: could not recall the session this ritual keeps ($id)"
    ritual_notify "$RIT_slug" 'could not recall the session it keeps, so the fire was skipped'
    return 1
  fi
  # Nothing to recall: this fire starts the session, with the prompt as its first argument the
  # way a `new` run gets it, and that session is the ritual's from then on.
  ritual_launch_pane || {
    ritual_note "$RIT_slug" 'not run: tmux could not open a window for the session it keeps'
    ritual_notify "$RIT_slug" 'could not open a window for the session it keeps'
    return 1
  }
  return 2
}

# rit_send_prompt <session-id> <why>: the prompt into that resident's input line, and what
# happened either way written where somebody will find it.
rit_send_prompt() {
  local id=$1 why=${2:-} blocked rc
  blocked=$(rit_wait_ready "$id") || {
    rec_load "$RES_DIR/$id"
    ritual_note "$RIT_slug" "not sent: ${R_name:-$id} $blocked"
    ritual_notify "$RIT_slug" "${R_name:-$id} $blocked, so the fire was not delivered"
    return 1
  }
  rec_load "$RES_DIR/$id"
  cast_type "$R_pane" "$(ritual_prompt_text)"; rc=$?
  case $rc in
    0) # No notification: the prompt is sitting in a tab, and the resident's own chip says the
       # rest. The journal keeps it, because that is where "did it fire?" is answered.
       ritual_note "$RIT_slug" "sent to ${R_name:-$id}${why:+ ($why)}" ;;
    2) ritual_note "$RIT_slug" "not sent: the prompt never reached ${R_name:-$id}'s input line"
       ritual_notify "$RIT_slug" "the prompt never reached ${R_name:-$id}'s input line, so nothing was submitted"
       return 1 ;;
    *) ritual_note "$RIT_slug" "not sent: tmux would not type into ${R_name:-$id}'s pane"
       ritual_notify "$RIT_slug" "could not type into ${R_name:-$id}'s pane"
       return 1 ;;
  esac
  return 0
}

# `gensokyo _send <slug> [why]`: the fire that goes into a resident that is already there, in a
# job of the server's - so with nobody to report to, and every way out of it ending in the
# ritual's journal instead. The same shape as `_headless`, and the ritual is looked up again here
# by its whole name for the same reason: find_ritual takes a part of one, and a ritual deleted in
# the moment since the fire must not let a different one run under this name.
cmd__send() {
  local slug=${1:-} why=${2:-} path id f rc
  [ -n "$slug" ] || return 1
  # The panes are going one after another; a prompt typed into one of them now is a prompt into a
  # session that is about to be told to leave.
  quit_in_progress && return 0
  path=$(find_ritual "$slug") && [ "${path##*/}" = "$slug.md" ] && ritual_load "$path" || {
    ritual_note "$slug" 'not sent: the ritual was gone by the time its fire was sent'
    ritual_notify "$slug" 'the ritual was gone by the time its fire was sent'
    return 1
  }
  if [ "$RIT_target" = persistent ]; then
    id=$(rit_session_ready); rc=$?
    case $rc in
      0) ;;
      2) return 0 ;;   # a fresh session, started with the prompt already in it
      *) return 1 ;;   # said so for itself
    esac
  else
    f=$(find_resident "$RIT_target") || {
      ritual_note "$slug" "not sent: there is no resident called $RIT_target"
      ritual_notify "$slug" "$RIT_target is not here, so the fire was not delivered"
      return 1
    }
    id=${f##*/}
    rec_load "$f"
    # A departed screen has no input line, and recalling somebody else's resident is not this
    # ritual's business: the user chose to let it go.
    [ -z "$R_departed" ] || {
      ritual_note "$slug" "not sent: $RIT_target has left, and a departed screen has no prompt to type into"
      ritual_notify "$slug" "$RIT_target has left, so the fire was not delivered (gensokyo resume $RIT_target brings it back)"
      return 1
    }
  fi
  rit_send_prompt "$id" "$why"
}

# ---------------------------------------------------------------- keep: the finished run's tab
# A ritual that fires every morning leaves a tab every morning, and a run that has finished has
# nothing left to say. `keep` is how long that tab stays afterwards, and this is what acts on it:
# the same two things a person does by hand, in the same order - ask the resident to leave, then
# take its tab - so the session ends the way every other one does. The record is archived rather
# than deleted, which is the one difference from `close`: nobody was there to decide the
# transcript was finished with, so it stays in the recall list under `resume`.
#
# What starts the clock is the run finishing and nothing happening since: the status file's
# `since` while `pending` is `stopped`, which status_write moves whenever pending changes - so a
# prompt typed into the tab this afternoon puts the tab's life back to the full `keep`. A run
# sitting at a permission prompt is not finished and never expires, which is right and is also
# the one thing `keep` cannot bound.
#
# `keep` is read from the record and never from the ritual file: ritual_launch_pane writes it
# down at launch, so a ritual edited or deleted since cannot change what happens to a tab that
# is already open, and a run whose ritual has gone still has a tab somebody has to close.

# rit_take_tab <record> <pane> <ritual> <note>: the tab goes and the session stays recallable.
rit_take_tab() {
  # The record moves first and the line is written after it: a journal that says a tab was closed
  # when the record could not be moved aside - and so the pane is still there - is worse than no
  # line at all, and the clock will come round again.
  archive_record "$1" || return 1
  ritual_note "$3" "$4"
  tmux_ kill-pane -t "$2" 2>/dev/null
  return 0
}

# ritual_reap: every ritual run whose tab has outstayed its `keep`, dealt with. Called on the
# sweep's beat, from the clock, because `keep` is written in minutes and hours and nothing here
# needs to be noticed within three seconds.
#
# Records and not ritual files: a resident somebody summoned by hand has no `ritual` line and is
# nobody's to reap, whatever the rituals say.
ritual_reap() {
  local f id now live
  # The panes are going one after another and `quit` is watching these very records for the
  # `departed` it asked for; a second /exit sent into the middle of that is nobody's idea of tidy.
  quit_in_progress && return 0
  live=$'\n'$(live_panes)$'\n'
  [ "$live" != $'\n\n' ] || return 0
  now=$(date +%s)
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    rec_load "$f"
    [ -n "$R_ritual" ] || continue
    [ -n "$R_keep" ] || continue            # keep: forever, and a tab that stays
    [ -n "$R_pane" ] || continue            # no pane yet: a launch from a moment ago
    case $live in *$'\n'"$R_pane"$'\n'*) ;; *) continue ;; esac
    id=${f##*/}
    # A resident that has already left is showing the departed screen, and there is nothing to
    # ask it: the tab is all that is left, and taking it is two fast calls with no waiting in
    # them. Its own clock is the moment it left, so `keep` is what the screen sits there for.
    if [ -n "$R_departed" ]; then
      [ $((now - R_departed)) -ge "$R_keep" ] || continue
      rit_take_tab "$f" "$R_pane" "$R_ritual" \
        "closed ${R_name:-the run}'s tab, $(fmt_age "$R_keep") after it left (keep)"
      continue
    fi
    status_load "$id"
    [ "$S_pending" = stopped ] || continue  # mid-turn, waiting on the user, or not started yet
    [ -n "$S_since" ] || continue
    [ $((now - S_since)) -ge "$R_keep" ] || continue
    # Asking a resident to leave takes seconds and may have to be done twice, and this is the
    # clock's own loop. `run-shell -b` and not the headless run's nohup: a tab belongs to the
    # server, so there is nothing here worth outliving it for.
    tmux_ run-shell -b "$(sq "$SELF") _reap $(sq "$id") >/dev/null 2>&1"
  done
  return 0
}

# `gensokyo _reap <session-id>`: one finished run's tab, taken. A job of the server's, so with
# nobody to report to - `die` from here goes to /dev/null - and every way out of it ends in the
# ritual's journal instead.
#
# Everything the clock just checked is checked again: this starts a moment later, and a resident
# that has been given a prompt in between is not a finished run any more.
cmd__reap() {
  local id=${1:-} f now
  [ -n "$id" ] || return 1
  f=$RES_DIR/$id
  [ -f "$f" ] || return 0
  quit_in_progress && return 0
  rec_load "$f"
  [ -n "$R_ritual" ] || return 0
  [ -n "$R_keep" ] || return 0
  pane_live "$R_pane" || return 0
  if [ -z "$R_departed" ]; then
    status_load "$id"
    [ "$S_pending" = stopped ] || return 0
    now=$(date +%s)
    [ -n "$S_since" ] && [ $((now - S_since)) -ge "$R_keep" ] || return 0
    # The same gesture `close` makes, twice if the first one goes unheard, and for the same
    # reason: send-keys says tmux wrote the /exit, never that the resident read it. A resident
    # that will not leave keeps its tab - killing a session that is not answering is a decision
    # for the person whose session it is - and the journal says so.
    if ! ask_leave "$R_pane" "$id" && ! ask_leave "$R_pane" "$id"; then
      # And that is the end of it: the `keep` line goes out of the record, so the clock does not
      # come back in twenty seconds with another Ctrl-C and another /exit for a resident that is
      # sitting right there - which is what "its tab stays" says to the person reading the log.
      # Same shape as ritual_complain's `complained` file: said once, not every sweep.
      rec_del "$f" keep
      ritual_note "$R_ritual" \
        "${R_name:-the run} did not answer /exit, so its tab stays (keep $(fmt_age "$R_keep"))"
      return 1
    fi
    rec_load "$f"   # `departed` is in it now, and the name may have changed with a /rename
  fi
  rit_take_tab "$f" "$R_pane" "$R_ritual" \
    "closed ${R_name:-the run}'s tab, idle $(fmt_age "$R_keep") since the run finished (keep)"
}

# ---------------------------------------------------------------- the headless run
# `headless: true` is a run with no pane: no window, no tab, no resident, nothing to watch it
# with. `claude -p` is the whole of it - one process that prints one JSON object and exits - and
# because nobody is watching, everything the run did has to be written where a person will find
# it the next morning. That is three places, and they are meant to be read in this order: the
# notification when it lands, the line it leaves in the ritual's journal, and the run's own log
# under runs/, which holds what the run actually said.
ritual_runs_dir() { printf '%s\n' "$(ritual_dir "$1")/runs"; }

# The pid of the last headless run of that ritual, which is the only thing there is to recognise
# a run with no pane by. Deliberately not deleted when a run ends: the file says which process it
# was, and `kill -0` plus that process's own command line say whether it is still that run - so a
# pid left by a run that was killed rather than finished answers "not running" by itself, with
# nothing to tidy up after it.
ritual_pidfile() { printf '%s\n' "$(ritual_dir "$1")/headless.pid"; }

# rit_clip <text> <max>: the front of it, and a … for the rest. The other way round from `tilde`,
# on purpose: for a path the file name at the end is what identifies it, and for a sentence the
# beginning is.
rit_clip() {
  local t=$1
  [ "${#t}" -le "$2" ] && { printf '%s' "$t"; return 0; }
  printf '%s…' "${t:0:$(($2 - 1))}"
}

# ritual_headless_running <slug>: whether a headless run of that ritual is going right now. It
# has no pane, so ritual_running cannot see it and this is asked instead - and answered with no
# tmux server needed, which is what lets a hand run and `remove` refuse one with the cockpit down.
#
# The process's command line and not the pid alone: pids come round again, and a daily ritual
# leaves its pid file sitting there for a day. The slug is matched where `_headless` puts it.
ritual_headless_running() {
  local pid cmd
  pid=$(sed -n 1p "$(ritual_pidfile "$1")" 2>/dev/null)
  case ${pid:-x} in ''|*[!0-9]*) return 1 ;; esac
  kill -0 "$pid" 2>/dev/null || return 1
  # -ww: the whole command line, never as much of it as a terminal would fit. No truncation was
  # observed without it on this macOS even at 161 characters, with a tty and without - but a `ps`
  # that clipped the line would answer "no run in progress" while one was going, and then
  # `overlap: skip` stops skipping and two runs write one memory file. One flag against that.
  cmd=$(ps -ww -o command= -p "$pid" 2>/dev/null) || return 1
  case $cmd in *"_headless $1"|*"_headless $1 "*) return 0 ;; esac
  return 1
}

# ritual_launch_headless <why>: the run, started and then let go of. Nothing about it is decided
# here: `_headless` loads the ritual itself and says everything it has to say in the log and the
# notification, because the caller is either the clock's own loop, which has to be back at its
# next tick in three seconds, or a command line that has to return.
#
# nohup, and no streams: the run outlives the cockpit on purpose. `quit` takes the panes and the
# clock with it, and cutting a turn off halfway through would leave the ritual's notes half
# written for nothing gained - the log is still written, and with no server to toast on the
# desktop alert still lands.
ritual_launch_headless() {
  local pid
  mkdir -p "$(ritual_runs_dir "$RIT_slug")" || return 1
  nohup "$SELF" _headless "$RIT_slug" "${1:-}" </dev/null >/dev/null 2>&1 &
  pid=$!
  # Written here and not by the run itself: the next sweep is 20 s away, and a run that has not
  # yet got as far as its first line of output would otherwise read as no run at all.
  printf '%s\n' "$pid" > "$(ritual_pidfile "$RIT_slug")" 2>/dev/null
  return 0
}

RITUAL_RUNS_KEEP=50
# rit_runs_trim <slug>: the newest RITUAL_RUNS_KEEP run logs, and no more - a ritual that fires
# daily would otherwise leave a file a day there for ever. Their names start with the minute, so
# the glob's own order is oldest first and the ones to drop are at the front of it.
rit_runs_trim() {
  local dir drop
  dir=$(ritual_runs_dir "$1")
  set -- "$dir"/*.log
  [ -f "$1" ] || return 0            # the glob matched nothing and is standing in for itself
  drop=$(($# - RITUAL_RUNS_KEEP))
  while [ "$drop" -gt 0 ]; do
    rm -f "$1"; shift; drop=$((drop - 1))
  done
  return 0
}

# `gensokyo _headless <slug> [why]`: the headless run itself, in a process with no pane, no
# terminal, and nobody to report to - `die` from here would go to /dev/null. So every way this
# can end, the failures included, ends in the run's log and a notification instead.
#
# What it deliberately does not do, next to what a resident in a pane gets (lib/residents.sh):
# no hooks and no --settings, because the chips and the "needs you" alerts they drive are about a
# pane and there is none - this process watches the run itself and says so when it ends; no
# --plugin-dir, because the skills are for a resident being talked to; and no system paragraph,
# which would tell a run with no tab that it is a resident in one and name peers it cannot reach.
# The three cron tools go, for the reason cmd__run gives: a schedule they appear to make dies
# with the process that made it, and here that is a process nobody will even see exit.
cmd__headless() {
  local slug=${1:-} why=${2:-} d path log out err rc started t0 took prompt args
  local meta cost session turns denials first
  [ -n "$slug" ] || return 1
  d=$(ritual_dir "$slug")
  mkdir -p "$d/runs" 2>/dev/null
  # The whole name and not a part of one: find_ritual takes a partial when it picks out exactly
  # one ritual, and if this ritual's file has gone in the moment since the fire, a *different*
  # ritual whose name contains this one would be the single hit - and its prompt would run under
  # this name, into this ritual's notes. So the file it names has to be the file this is.
  path=$(find_ritual "$slug") && [ "${path##*/}" = "$slug.md" ] && ritual_load "$path" || {
    ritual_note "$slug" 'not run: the ritual was gone by the time its run started'
    ritual_notify "$slug" 'the ritual was gone by the time its run started'
    return 1
  }
  # Two clocks on purpose. When the run happened is gensokyo's clock, the one every other time in
  # a log or a listing comes from and the one GENSOKYO_NOW moves; how long it took can only be the
  # real one, since a frozen clock would make every run take no time at all.
  started=$(now_epoch); t0=$(date +%s)
  # The pid in the name as well as the minute: a fire and a hand run can land in the same second,
  # and the second of them writing over the first one's log would lose the only copy of it.
  log=$d/runs/$(date -r "$started" '+%Y%m%d-%H%M%S').$$.log
  out=$d/.run.out.$$ err=$d/.run.err.$$
  prompt=$(ritual_prompt_text)
  printf '%s\n' "$prompt" > "$d/prompt" 2>/dev/null
  args=$(ritual_args)
  {
    printf 'ritual   %s\n' "$slug"
    printf 'started  %s%s\n' "$(ritual_when "$started")" "${why:+  ($why)}"
    printf 'in       %s\n' "$RIT_cwd"
    printf 'flags    %s\n' "$args"
    printf 'notes    %s\n' "$d/memory.md"
    printf '\nNo pane and nobody to answer a prompt: this run finishes on its own, and goes on\n'
    printf 'doing so if the cockpit is quit under it.\n\n'
  } > "$log" 2>/dev/null
  # The directory is asked about rather than left to `cd` to notice, because `cd ""` succeeds and
  # changes nothing: a ritual edited between the fire and the run can have lost its `cwd:` line,
  # and a run in whatever directory the tmux server happened to start in is not this ritual's run.
  [ -d "$RIT_cwd" ] && cd "$RIT_cwd" 2>/dev/null || {
    printf 'cwd is not there any more, so nothing ran.\n' >> "$log"
    ritual_note "$slug" "not run: ${RIT_cwd:-it names no directory} is not there"
    ritual_notify "$slug" "${RIT_cwd:+$(tilde "$RIT_cwd") }is not there, so the run could not start"
    return 1
  }
  scrub_env
  eval "set -- $args"
  # </dev/null and not just the redirect a background process would get anyway: `claude -p` reads
  # stdin for a prompt to add to the one given, and waits three seconds for it before saying so on
  # stderr. Measured against 2.1.263; without it every headless run starts three seconds late.
  claude_ -p --output-format json --disallowed-tools CronCreate CronList CronDelete \
    "$@" -- "$prompt" </dev/null >"$out" 2>"$err"
  rc=$?
  took=$(($(date +%s) - t0))
  if [ "$rc" -ne 0 ]; then
    # `claude -p` says what went wrong on stderr and prints nothing at all on stdout, so the log
    # is where the sentence goes. The first lines of it: an error is a line, a stack trace is not.
    {
      printf -- '--- claude exited %s, and said:\n' "$rc"
      sed -n '1,20p' "$err" 2>/dev/null
      printf -- '---\nfailed after %s\n' "$(fmt_age "$took")"
    } >> "$log" 2>/dev/null
    rm -f "$out" "$err"
    ritual_note "$slug" "failed (headless, $(fmt_age "$took")): claude exited $rc"
    ritual_notify "$slug" "the headless run failed - gensokyo ritual log $slug says what it said"
    rit_runs_trim "$slug"
    return 1
  fi
  # One object, and every field of it optional as far as this is concerned: a gensokyo that
  # refused to report a run because a field it wanted was missing would be the worse of the two.
  # One field per line, and never a separator inside a line: the last of these is a sentence
  # somebody's ritual wrote and can hold any character at all - a tab included, which is what
  # made a @tsv row read as one field short the first time this was written, because `read`
  # takes two tabs in a row for one.
  meta=''
  [ -n "$JQ_BIN" ] && meta=$(jq_ -r '
    [ (.total_cost_usd // 0 | tostring),
      (.session_id // ""),
      (.num_turns // 0 | tostring),
      ((.permission_denials // []) | map(.tool_name // "a tool") | unique | join(", ")),
      ((.result // "") | split("\n") | map(select(length > 0)) | (.[0] // "")) ]
    | .[]' "$out" 2>/dev/null)
  if [ -n "$meta" ]; then
    {
      IFS= read -r cost; IFS= read -r session; IFS= read -r turns
      IFS= read -r denials; IFS= read -r first
    } <<EOF
$meta
EOF
    jq_ -r '.result // ""' "$out" >> "$log" 2>/dev/null
  else
    # No jq, or an answer it could not read. The raw object is worth more than a summary of it
    # that gensokyo had to guess at, so the log gets the whole thing and says which this is.
    printf 'gensokyo could not read the result as JSON; it is here as claude printed it.\n\n' >> "$log"
    cat "$out" >> "$log" 2>/dev/null
    cost='' session='' turns='' denials='' first=''
  fi
  {
    printf -- '\n---\ndone in %s' "$(fmt_age "$took")"
    [ -n "$cost" ] && printf ', %s' "$(fmt_cost "$cost")"
    [ -n "$turns" ] && printf ', %s turns' "$turns"
    printf '\n'
    # A headless run has nobody to ask, so a tool it needed and did not have is the way it comes
    # to finish successfully having done none of what it was asked. Named, with the line to add.
    [ -n "$denials" ] &&
      printf "refused  %s - a run with nobody to ask needs it in the ritual's allowed_tools\n" "$denials"
    [ -n "$session" ] && printf 'to read the whole transcript: claude --resume %s\n' "$session"
  } >> "$log" 2>/dev/null
  rm -f "$out" "$err"
  # The result's first line goes in the journal as well as in the notification: the alert is gone
  # in four seconds, and `ritual log` the next morning is where someone actually looks.
  ritual_note "$slug" \
    "done (headless, $(fmt_age "$took")${cost:+, $(fmt_cost "$cost")})${first:+: $(rit_clip "$first" 120)}"
  ritual_notify "$slug" \
    "done${first:+: $(rit_clip "$first" 90)}${denials:+ (it needed $denials and had nobody to ask)}"
  rit_runs_trim "$slug"
  return 0
}
