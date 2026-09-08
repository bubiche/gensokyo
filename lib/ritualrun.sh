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

# ritual_launch <why>: fire it, whichever way the ritual says. The two ways have almost nothing
# in common - one opens a window and hands the prompt to a resident, the other is a process with
# no terminal - so the decision is made once, here, and both callers just say what the run is for.
ritual_launch() {
  rit_bool "$RIT_headless" && { ritual_launch_headless "${1:-}"; return $?; }
  ritual_launch_pane
}

# ritual_launch_pane: the resident that does the work. The same record cmd_new writes, plus the two
# fields a run needs: `ritual`, which is how the notifications, the overlap check and the
# cockpit know whose run this is, and `prompt_file`, because a ritual's prompt is many lines
# and a record holds one line per key. `launched` is now and not the minute the run is for:
# every reader of that field takes it for the time the resident started - `list` and the recall
# rows print it, and prune_records drops a record that has no pane 30 s after it - so a
# catch-up run stamped with a minute from last week would be born prunable.
ritual_launch_pane() {
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
    if ritual_busy "$RIT_slug"; then
      ritual_note "$RIT_slug" "skipped ($why $(ritual_when "$when")): the last run is still going"
      continue
    fi
    if ritual_launch "$why $(ritual_when "$when")"; then
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
