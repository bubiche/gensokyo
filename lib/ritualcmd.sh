# lib/ritualcmd.sh - the `gensokyo ritual` commands: what is scheduled, when it fires next, and
# the file behind it. lib/rituals.sh reads a ritual and lib/ritualrun.sh fires one; this is how a
# person says so - by hand, or through the skill, which is a person's words with a command
# behind them. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

RITUAL_USAGE='usage: gensokyo ritual [list [--json]] | add --name n --schedule c --cwd d --prompt-file f [--headless]
       gensokyo ritual run|enable|disable|edit <name> | log <name> [-n N] | new|remove <name>'

# ---------------------------------------------------------------- the bits every verb wants
# rit_no_such <verb> <name>: the refusal every verb that takes a name shares. find_ritual takes
# a part of a name when it picks out exactly one ritual and says nothing when it picks out two,
# so the one message has to cover both. Said by the caller rather than inside a helper the caller
# runs in a substitution, where an exit would only leave the substitution and the verb would
# carry on with an empty path.
rit_no_such() {
  die "$1: no ritual called '$2', or more than one that could be (gensokyo ritual lists them)"
}

# rit_next_text <schedule>: the next minute it comes round, as the listings write it.
rit_next_text() {
  local now e
  now=$(now_epoch)
  e=$(cron_next "$1" "$now") || return 1
  printf '%s (in %s)\n' "$(ritual_when "$e")" "$(fmt_age $((e - now)))"
}

# ritual_last_run <slug>: when the ritual last actually ran, from its own log. Deliberately not
# the stamp: that is the minute a fire was *spent* on, and the sweep also stamps a ritual the
# first time it sees one, so a ritual that has never run yet would read as having run then.
ritual_last_run() {
  local d
  d=$(ritual_dir "$1")
  [ -f "$d/log" ] || return 0
  awk -F'\t' '$2 ~ /^ran \(/ { v = $1 } END { if (v != "") print v }' "$d/log" 2>/dev/null
}

# rit_newest_run <slug>: the last headless run's own log, or nothing. Their names begin with the
# minute they were written in, so the glob's order is the order they happened in.
rit_newest_run() {
  local f last=''
  for f in "$(ritual_runs_dir "$1")"/*.log; do
    [ -f "$f" ] && last=$f
  done
  printf '%s' "$last"
}

# rit_problem_line <problem> <path>: what a listing puts under a ritual with something wrong
# with it. One case gets a different line: a shipped example names a directory that is on
# nobody's machine on purpose, and the file is not the user's to fix - an update replaces it -
# so what to do about it is to take a copy, which enable and edit do by themselves.
RIT_EXAMPLE='an example: enable or edit it, and gensokyo copies it to your own rituals first'
rit_problem_line() {
  case $2 in
    "$SHARE"/*) case $1 in cwd:*) printf '%s' "$RIT_EXAMPLE"; return 0 ;; esac ;;
  esac
  printf '! %s' "$1"
}

# rit_open_run <slug>: the name of a resident of that ritual still sitting in a tab, finished or
# not. `remove` says so afterwards. A finished run is not a run in progress - it waits in its tab
# until the owner closes it - so it does not stop the delete; but the prompt it was started with
# names the notes file that just went, and anything typed in that pane writes the directory back
# for a ritual that is no longer there, where no verb here can see it again.
rit_open_run() {
  local f live
  live=$'\n'$(live_panes)$'\n'
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    rec_load "$f"
    [ "$R_ritual" = "$1" ] || continue
    [ -z "$R_departed" ] || continue
    [ -n "$R_pane" ] || continue
    case $live in *$'\n'"$R_pane"$'\n'*) printf '%s\n' "${R_name:-$1}"; return 0 ;; esac
  done
  return 1
}

# rit_mine <path>: the user's own copy of that ritual, made from the shipped one if that is what
# was named. The install tree is not the user's to edit and an update may replace it, and a file
# of the same name in the config directory shadows the shipped one anyway (lib/rituals.sh) - so
# enabling, disabling or editing a shipped example works on a copy, and the caller says so.
rit_mine() {
  local src=$1 dst
  case $src in "$CONFIG_DIR"/*) printf '%s\n' "$src"; return 0 ;; esac
  dst=$CONFIG_DIR/rituals/${src##*/}
  mkdir -p "$CONFIG_DIR/rituals" || return 1
  [ -f "$dst" ] || cp "$src" "$dst" || return 1
  printf '%s\n' "$dst"
}

# rit_set_key <file> <key> <value>: that key in the frontmatter, replaced where it is and added
# before the closing fence where it is not. A file with no frontmatter has no line to set and no
# schedule either, so it comes back as 3 rather than being given one. An inline comment on the
# old line goes with the old line, which is worth knowing before writing `enabled: true # today`.
rit_set_key() {
  local f=$1 key=$2 val=$3 tmp rc
  tmp=$f.tmp.$$
  awk -v key="$key" -v val="$val" '
    BEGIN { state = "start"; done = 0 }
    {
      if (state == "start") {
        if ($0 == "---") { state = "front"; print; next }
        state = "body"
      } else if (state == "front") {
        if ($0 == "---") {
          if (!done) { print key ": " val; done = 1 }
          state = "body"; print; next
        }
        line = $0
        sub(/^[ \t]+/, "", line)
        if (index(line, key ":") == 1) {
          if (!done) { print key ": " val; done = 1 }
          next
        }
      }
      print
    }
    END { exit done ? 0 : 3 }
  ' "$f" > "$tmp"
  rc=$?
  [ "$rc" -eq 0 ] || { rm -f "$tmp"; return "$rc"; }
  mv "$tmp" "$f"
}

# rit_q <value>: a value written so that rit_value reads exactly it back. Quoted rather than
# bare, because a value can hold a `#` or a colon and a bare one would lose the rest of the line.
# A value holding both kinds of quote cannot be written this way at all; rit_q_ok says so before
# anything is written, because a `die` in here would only leave the substitution it runs in.
rit_q() {
  case $1 in
    *'"'*) printf "'%s'" "$1" ;;
    *)     printf '"%s"' "$1" ;;
  esac
}
rit_q_ok() { case $1 in *'"'*) case $1 in *"'"*) return 1 ;; esac ;; esac; return 0; }

# rit_report <file>: what the file says now - the line that is wrong, or the minute it fires
# next. What `add`, `edit` and `new` all end with, since all three leave the user with a file.
rit_report() {
  local problem
  ritual_load "$1" || { warn "cannot read $(tilde "$1")"; return 1; }
  problem=$(ritual_problem)
  [ -z "$problem" ] || { warn "$RIT_slug: $problem"; return 0; }
  if ritual_enabled; then
    say "  next fire  $(rit_next_text "$RIT_schedule")"
  else
    say "  disabled, so it will not fire: gensokyo ritual enable $RIT_slug"
  fi
  return 0
}

# rit_open_editor <file>: $VISUAL, else $EDITOR, else vi. The variable may carry flags of its
# own ("code -w"), which is what every other tool that opens an editor allows, so it is split
# on blanks on purpose.
rit_open_editor() {
  local ed=${VISUAL:-${EDITOR:-vi}}
  # shellcheck disable=SC2086
  $ed "$1"
}

# ---------------------------------------------------------------- the command
# ritual (schedule) [list|add|run|enable|disable|log|edit|new|remove]: the scheduled work. Alone
# it lists what is scheduled, which is what the question "what is scheduled?" wants.
cmd_ritual() {
  local sub=list
  if [ $# -gt 0 ]; then sub=$1; shift; fi
  case $sub in
    list|ls)          ritual_cmd_list "$@" ;;
    add)              ritual_cmd_add "$@" ;;
    run)              ritual_cmd_run "$@" ;;
    enable|disable)   ritual_cmd_toggle "$sub" "$@" ;;
    remove|delete|rm) ritual_cmd_remove "$@" ;;
    log)              ritual_cmd_log "$@" ;;
    edit)             ritual_cmd_edit "$@" ;;
    new)              ritual_cmd_new "$@" ;;
    -h|--help|help)   say "$RITUAL_USAGE" ;;
    *) die "ritual: nothing called '$sub' to do"$'\n'"$RITUAL_USAGE" ;;
  esac
}

# ritual_cmd_rows: one line per ritual, by name, with the columns a listing shows and the row
# form deliberately leaves out (lib/rituals.sh): the next fire, the last real run, and the line
# that is wrong. Each costs a walk through the schedule or a read of the log, which is why they
# are worked out here - for a person asking - and never in the sweep.
#   slug|on|schedule|next|next text|last run|target|headless|keep|overlap|cwd|description|problem|path
ritual_cmd_rows() {
  local slug on sched target headless desc path next problem last
  while IFS='|' read -r slug on sched target headless desc path; do
    [ -n "$slug" ] || continue
    ritual_load "$path" || continue
    problem=$(ritual_problem)
    next=$(cron_next "$sched" "$(now_epoch)") || next=''
    last=$(ritual_last_run "$slug")
    # The columns are cut apart with IFS='|', so the fields a person writes prose (or a path)
    # into do not get to carry one.
    printf '%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s\n' \
      "$slug" "$on" "$sched" "$next" "${next:+$(ritual_when "$next")}" "$last" \
      "$target" "$headless" "$RIT_keep" "$RIT_overlap" "${RIT_cwd//|/ }" "${desc//|/ }" \
      "${problem//|/ }" "$path"
  done <<EOF
$(ritual_rows_sorted)
EOF
  return 0
}

# list [--json]: what is scheduled. The plain form is for a person and says the problem under
# the ritual it belongs to; --json is what the skill reads before it answers "what is
# scheduled?" or changes one. Whether a run is going right now is not in either: that needs the
# tmux server, and this command has to answer the same way with the cockpit down.
ritual_cmd_list() {
  local json='' rows='' n=0 f now
  local slug on sched next nexttext last target headless keep overlap cwd desc problem path
  while [ $# -gt 0 ]; do
    case $1 in
      --json) json=1 ;;
      *) die "ritual list: unknown option $1 (usage: gensokyo ritual list [--json])" ;;
    esac
    shift
  done
  ensure_dirs
  rows=$(ritual_cmd_rows)
  if [ -n "$json" ]; then
    [ -n "$JQ_BIN" ] || die "jq not found: run scripts/vendor.sh (or set GENSOKYO_JQ)"
    # shellcheck disable=SC2016
    printf '%s\n' "$rows" | jq_ -R -s '
      def opt: if . == "" then null else . end;
      def num: if . == "" then null else tonumber end;
      split("\n") | map(select(length > 0) | split("|"))
      | map({name: .[0], enabled: (.[1] == "yes"), schedule: .[2],
             next_fire: (.[3] | num), next_fire_local: (.[4] | opt), last_run: (.[5] | num),
             target: .[6], headless: (.[7] == "true"), keep: .[8], overlap: .[9],
             cwd: (.[10] | opt), description: (.[11] | opt), problem: (.[12] | opt), path: .[13]})'
    return 0
  fi
  now=$(now_epoch)
  while IFS='|' read -r slug on sched next nexttext last target headless keep overlap cwd desc problem path; do
    [ -n "$slug" ] || continue
    [ "$n" -eq 0 ] && printf '  %-18s %-4s %-14s %-26s %s\n' ritual '' schedule 'next fire' 'what it does'
    n=$((n + 1))
    # A paused ritual's next fire is the minute its schedule names, which is in the JSON for
    # whoever wants it - but a column of times beside the word `off` reads as a promise, so
    # here it says what is actually going to happen instead.
    printf '  %-18s %-4s %-14s %-26s %s\n' "$slug" "$([ "$on" = yes ] && printf on || printf off)" \
      "$sched" "$([ "$on" = yes ] && printf '%s' "${next:+$nexttext (in $(fmt_age $((next - now))))}" || printf paused)" \
      "$desc"
    [ -n "$last" ] && printf '  %-18s %s\n' '' "last ran $(ritual_when "$last") ($(fmt_age $((now - last))) ago)"
    # Which kind of run this is, said only of the kind that has nothing to watch: a ritual whose
    # fire opens a tab needs no explaining, and one whose fire opens nothing at all does.
    [ "$headless" = true ] && printf '  %-18s %s\n' '' 'headless: no pane, and its own log of what each run said'
    [ -n "$problem" ] && printf '  %-18s %s\n' '' "$(rit_problem_line "$problem" "$path")"
  done <<EOF
$rows
EOF
  if [ "$n" -eq 0 ]; then
    say "nothing is scheduled yet: gensokyo ritual new <name>"
    say "  your rituals live in $(tilde "$CONFIG_DIR/rituals")/<name>.md"
  else
    say
    say "  gensokyo ritual run <name>      fire one now, which is how its prompts get approved once"
    say "  gensokyo ritual log <name>      what it has done; edit <name> opens the file"
  fi
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    warn "not a usable ritual name (letters, digits, . _ - and .md): $(tilde "$f")"
  done <<EOF
$(ritual_unusable)
EOF
  return 0
}

# add: the non-interactive way in, which is what the skill calls once the user has said yes to
# what it drafted. It writes the file and prints the next fire; the clock finds it on its next
# sweep, so nothing has to be restarted.
#
# What is refused and what is only said matters here. A ritual whose name, schedule or prompt is
# wrong could not fire on any day and there is no next fire to print, so nothing is written. A
# ritual whose `cwd` is not there yet or has never had Claude Code's trust prompt answered is a
# file that is right about everything it can be right about - the fix is outside it, and the
# messages say what to do - so it is written, warned about, and left for the user to sort out.
ritual_cmd_add() {
  local name='' sched='' cwd='' desc='' model='' effort='' mode='' mcp='' target='' keep=''
  local overlap='' catch='' tools='' more prompt='' pf='' enabled='' headless='' path problem t
  while [ $# -gt 0 ]; do
    case $1 in
      --name)          name=${2:-}; shift ;;
      --schedule)      sched=${2:-}; shift ;;
      --cwd)           cwd=${2:-}; shift ;;
      --description)   desc=${2:-}; shift ;;
      --model)         model=${2:-}; shift ;;
      --effort)        effort=${2:-}; shift ;;
      --mode|--permission-mode) mode=${2:-}; shift ;;
      --mcp-config)    mcp=${2:-}; shift ;;
      --target)        target=${2:-}; shift ;;
      --keep)          keep=${2:-}; shift ;;
      --overlap)       overlap=${2:-}; shift ;;
      # A flag and not a value: the file's `headless: false` is what leaving it out means, and a
      # `--headless false` would be a third way of saying the same thing.
      --headless)      headless=true ;;
      --catch-up)      catch=${2:-}; shift ;;
      --prompt)        prompt=${2:-}; shift ;;
      --prompt-file)   pf=${2:-}; shift ;;
      --disabled)      enabled=false ;;
      # Said as many times as there are tools, or once with commas between them. Kept one per
      # line either way, because a permission pattern can hold a space.
      --allowed-tools|--allowed-tool)
        more=$(rit_list "${2:-}")
        [ -n "$more" ] && tools="${tools:+$tools$'\n'}$more"
        shift ;;
      *) die "ritual add: unknown option $1"$'\n'"$RITUAL_USAGE" ;;
    esac
    shift
  done
  ensure_dirs
  [ -n "$name" ] || die "ritual add: --name is the ritual's name, and the name of its file"
  ritual_name_ok "$name" ||
    die "ritual add: '$name' cannot be a ritual name (a letter or digit first, then letters, digits, . _ -)"
  if [ -n "$pf" ]; then
    [ -z "$prompt" ] || die 'ritual add: --prompt or --prompt-file, not both'
    if [ "$pf" = - ]; then prompt=$(cat)
    else
      pf=${pf/#\~/$HOME}
      [ -f "$pf" ] || die "ritual add: no such prompt file: $pf"
      prompt=$(cat "$pf") || die "ritual add: could not read $pf"
    fi
  fi
  [ -n "$prompt" ] || die 'ritual add: --prompt-file <file> (or --prompt "one line", or --prompt-file - for stdin) is what the run is asked to do'
  [ -n "$sched" ] || die 'ritual add: --schedule "3 9 * * 1-5" (five cron fields, @hourly @daily @weekly @monthly, or "every 30m")'
  cron_ok "$sched" ||
    die "ritual add: --schedule $sched is not one gensokyo can read (five fields with * , - /, @daily, or \"every 30m\" with a length that divides the hour)"
  cron_next "$sched" "$(now_epoch)" >/dev/null ||
    die "ritual add: --schedule $sched never comes round (a date that does not exist, like 30 February)"
  # The clock runs from wherever the tmux server was started, so a run has no directory of its
  # own to be relative to; a relative --cwd is resolved here, where the person typing it is.
  cwd=${cwd:-$PWD}
  cwd=${cwd/#\~/$HOME}
  case $cwd in /*) ;; *) cwd=$PWD/$cwd ;; esac
  # A directory that is there is written as it really is, so that `.` and a path through a
  # symlink read back as somewhere a person recognises. One that is not there is left exactly as
  # it was typed, because it is about to be complained about by that name.
  [ -d "$cwd" ] && cwd=$(cd "$cwd" && pwd)
  # Every value that is written quoted, checked here rather than at the printf that writes it:
  # one holding both a ' and a " cannot be written so that the reader gets it back.
  while IFS= read -r t; do
    rit_q_ok "$t" || die "ritual add: a value cannot hold both kinds of quote: $t"
  done <<EOF
$desc
$cwd
$mcp
$tools
EOF
  path=$CONFIG_DIR/rituals/$name.md
  [ -f "$path" ] &&
    die "ritual add: $name is already there: $(tilde "$path") (edit that file, or gensokyo ritual edit $name)"
  # A shipped example of the same name still works - the user's file shadows it - but silently
  # taking over a name that already means something is worth a word.
  [ -f "$SHARE/rituals/$name.md" ] &&
    warn "ritual add: $name is also one of the shipped examples; yours shadows it from now on"
  mkdir -p "$CONFIG_DIR/rituals" || die "ritual add: could not make $(tilde "$CONFIG_DIR/rituals")"
  {
    printf -- '---\n'
    printf 'name: %s\n' "$name"
    [ -n "$desc" ]    && printf 'description: %s\n' "$(rit_q "$desc")"
    printf 'schedule: %s\n' "$(rit_q "$sched")"
    [ -n "$target" ]  && printf 'target: %s\n' "$target"
    printf 'cwd: %s\n' "$(rit_q "$cwd")"
    [ -n "$model" ]   && printf 'model: %s\n' "$model"
    [ -n "$effort" ]  && printf 'effort: %s\n' "$effort"
    [ -n "$mode" ]    && printf 'mode: %s\n' "$mode"
    # The block form, always: an inline list is cut on commas, and a permission pattern is
    # allowed to hold one.
    if [ -n "$tools" ]; then
      printf 'allowed_tools:\n'
      while IFS= read -r t || [ -n "$t" ]; do
        [ -n "$t" ] && printf '  - %s\n' "$(rit_q "$t")"
      done <<EOF
$tools
EOF
    fi
    [ -n "$mcp" ]     && printf 'mcp_config: %s\n' "$(rit_q "$mcp")"
    [ -n "$keep" ]    && printf 'keep: %s\n' "$keep"
    [ -n "$overlap" ] && printf 'overlap: %s\n' "$overlap"
    [ -n "$headless" ] && printf 'headless: %s\n' "$headless"
    [ -n "$catch" ]   && printf 'catch_up: %s\n' "$catch"
    [ -n "$enabled" ] && printf 'enabled: %s\n' "$enabled"
    printf -- '---\n%s\n' "$prompt"
  } > "$path" || die "ritual add: could not write $(tilde "$path")"
  # Read back rather than trusted: the file is what the clock will read, and everything the
  # options do not cover - a target that is not wired up, a keep that is not a length - is
  # ritual_problem's to say. `cwd:` is the one kind of problem the file cannot fix itself, so it
  # is the one kind that is only warned about; anything else takes the file with it.
  ritual_load "$path"
  problem=$(ritual_problem)
  case $problem in
    '') ;;
    'cwd: '*) warn "ritual add: $problem" ;;
    *) rm -f "$path"; die "ritual add: $problem"$'\n'"nothing was written" ;;
  esac
  say "wrote $(tilde "$path")"
  if ritual_enabled; then
    say "  next fire  $(rit_next_text "$sched")"
  else
    say "  disabled, so it will not fire: gensokyo ritual enable $name"
  fi
  # The five fields are read on this machine's clock (lib/cron.sh walks with a local `date`), and
  # this line is here because something else tells a resident otherwise: a session that knows the
  # user's timezone is also told to convert a local time to UTC before writing a cron line, and it
  # will do it to a ritual - measured 2026-09-08, five times in eight, while reporting back the
  # hour the user asked for. Printing the clock the schedule is actually read on is what makes
  # that visible at the one moment somebody is still looking.
  # %z, not %Z: the abbreviation is a zone's own business - Singapore has none and prints `+08`
  # while Tokyo prints `JST` - and the offset is the quantity a UTC conversion got wrong anyway.
  say "  $sched is this machine's clock, now $(date '+%H:%M %z') - a ritual is never in UTC"
  # Which kind of run it is going to get, for the one kind that opens nothing: a person who
  # agreed to a ritual is picturing a tab, and this is where they find out there will not be one.
  [ -n "$headless" ] &&
    say "  headless: no pane to watch and nobody to answer a prompt, so what it needs goes in allowed_tools; what each run says keeps in $(tilde "$(ritual_runs_dir "$name")")"
  say "  gensokyo ritual run $name   fires it now, so its prompts can be approved once"
  return 0
}

# run <name>: fire it now, by hand. The schedule and `enabled` are not consulted - running one
# by hand is how a ritual's permission prompts get approved the first time, and that is worth
# doing before the morning it is meant to work on its own. The stamp is deliberately left alone:
# a hand run is not the minute the schedule asked for, and spending that minute here would make
# the real fire skip itself.
ritual_cmd_run() {
  local name=${1:-} path problem
  [ -n "$name" ] || die "ritual run: which one? (gensokyo ritual lists them)"
  [ $# -le 1 ] || die "ritual run: one ritual at a time"
  path=$(find_ritual "$name") || rit_no_such 'ritual run' "$name"
  ritual_load "$path" || die "ritual run: could not read $(tilde "$path")"
  problem=$(ritual_problem)
  [ -z "$problem" ] || die "ritual run: $RIT_slug: $problem"
  ensure_dirs
  # A headless run wants no cockpit at all: no pane to put it in, and nobody to answer a prompt
  # it stops at, which is the whole of what `headless: true` means. It is asked about first for
  # that reason - the question below starts a tmux server to be able to answer it.
  ritual_headless_running "$RIT_slug" &&
    die "ritual run: $RIT_slug is running headless right now, and both runs would write its notes - it has no pane to close, so wait for it (gensokyo ritual log $RIT_slug says when it started)"
  if rit_bool "$RIT_headless"; then
    # Still asked, and only if there is a server to ask: a ritual that was firing into panes
    # until this morning can have one of those runs still going.
    server_running && ritual_running "$RIT_slug" &&
      die "ritual run: a run of $RIT_slug is still going in a tab (gensokyo list), and both runs would write its notes"
    ritual_enabled || say "$RIT_slug is disabled: this run is by hand, and the schedule stays off"
    rit_queue_clear "$RIT_slug"
    ritual_launch 'by hand' || die "ritual run: could not start a headless run of $RIT_slug"
    ritual_note "$RIT_slug" 'ran (by hand)'
    say "$RIT_slug is running headless in $(tilde "$RIT_cwd"): no pane, and nothing to watch"
    say "  gensokyo will say when it lands; what it said keeps in $(tilde "$(ritual_runs_dir "$RIT_slug")")"
    return 0
  fi
  # Before the overlap check, not after: with no server there are no panes to be alive in, and a
  # record left by a cockpit that has since stopped would read as a run still going.
  start_server
  # A fire into a resident that is already there is a different thing to report: nothing is
  # summoned, there is no pane of its own to watch, and whether it landed is decided a second or
  # two from now by a job of the server's. The overlap check is skipped for the same reason the
  # sweep skips it - what is busy there is somebody else's session, and Claude Code queues a
  # prompt sent into a turn.
  if [ "$RIT_target" != new ]; then
    ritual_enabled || say "$RIT_slug is disabled: this run is by hand, and the schedule stays off"
    rit_queue_clear "$RIT_slug"
    ritual_launch 'by hand' || die "ritual run: could not send $RIT_slug's prompt to $RIT_target"
    ritual_note "$RIT_slug" 'ran (by hand)'
    if [ "$RIT_target" = persistent ]; then
      say "$RIT_slug's prompt is on its way to the session it keeps"
    else
      say "$RIT_slug's prompt is on its way to $RIT_target"
    fi
    say "  gensokyo ritual log $RIT_slug says where it landed, or why it did not"
    return 0
  fi
  ritual_running "$RIT_slug" &&
    die "ritual run: $RIT_slug is still running from last time (gensokyo list), and both runs would write its notes"
  ritual_enabled || say "$RIT_slug is disabled: this run is by hand, and the schedule stays off"
  # A fire waiting behind the last run has just been superseded by this one: running it by hand
  # and then again the moment the ritual is free is the job done twice.
  rit_queue_clear "$RIT_slug"
  ritual_launch 'by hand' || die "ritual run: tmux could not open a window for $RIT_slug"
  ritual_note "$RIT_slug" 'ran (by hand)'
  say "$RIT_slug is running in $(tilde "$RIT_cwd")"
  [ "$(clients_in_mode cc)" -eq 0 ] && [ "$(clients_in_mode tty)" -eq 0 ] &&
    say "  nothing is attached, so nobody can answer a prompt it stops at: run 'gensokyo'"
  return 0
}

# enable|disable <name>: the `enabled:` line, from here rather than from an editor - which is
# what "pause my slack ritual" comes down to.
ritual_cmd_toggle() {
  local verb=$1 name=${2:-} src path want rc
  [ -n "$name" ] || die "ritual $verb: which one? (gensokyo ritual lists them)"
  [ $# -le 2 ] || die "ritual $verb: one ritual at a time"
  want=false; [ "$verb" = enable ] && want=true
  src=$(find_ritual "$name") || rit_no_such "ritual $verb" "$name"
  ensure_dirs
  path=$(rit_mine "$src") ||
    die "ritual $verb: could not copy $(tilde "$src") into $(tilde "$CONFIG_DIR/rituals")"
  [ "$path" = "$src" ] ||
    say "the shipped ${path##*/} is now yours, in $(tilde "$path")"
  rit_set_key "$path" enabled "$want"; rc=$?
  case $rc in
    0) ;;
    3) die "ritual $verb: $(tilde "$path") has no frontmatter, so there is no schedule to turn on or off (gensokyo ritual edit ${path##*/})" ;;
    *) die "ritual $verb: could not write $(tilde "$path")" ;;
  esac
  ritual_load "$path"
  say "$RIT_slug is ${verb}d"
  [ "$verb" = enable ] && rit_report "$path"
  return 0
}

# remove|delete|rm <name>: the ritual, its notes and its journal, gone. `disable` is the answer
# for "not for now"; this is the answer for "never again", and it is the one verb here that
# takes nothing back. There is no confirmation to type: `close` asks for none either, and the
# question belongs on the screens a person clicks (lib/shrine.sh, lib/cockpit.sh) rather than in
# front of a resident that was told to do this in words.
ritual_cmd_remove() {
  local name=${1:-} path slug d open_run=''
  [ -n "$name" ] || die "ritual remove: which one? (gensokyo ritual lists them)"
  [ $# -le 1 ] || die "ritual remove: one ritual at a time"
  path=$(find_ritual "$name") || rit_no_such 'ritual remove' "$name"
  # The slug from the file name and not from ritual_load: the ritual most worth deleting is the
  # one gensokyo cannot read, and find_ritual only ever names a file whose name is a usable slug.
  slug=${path##*/}; slug=${slug%.md}
  # Every other verb takes a part of a name, and this one does not. `run slack` reaching
  # slack-morning saves a word; `remove slack` reaching it spends a file, its notes and its
  # journal on a guess, and the caller is often a resident working from a half-remembered name.
  # So the near miss is named instead, and nothing is touched.
  [ "$(lower "$slug")" = "$(lower "$name")" ] ||
    die "ritual remove: no ritual called '$name' - did you mean $slug? (remove wants the whole name)"
  # An example is not the user's file: it is in the install tree, an update would put it back,
  # and the copy that shadows it is what enable and edit make. Pausing is what it comes down to.
  case $path in
    "$SHARE"/*) die "ritual remove: $slug is one of the examples gensokyo ships, and an update would put it back: 'gensokyo ritual disable $slug' is how you stop it firing" ;;
  esac
  # A run of it going right now is about to write the notes this would delete, and it would
  # carry on afterwards against a ritual that is no longer there. A headless run is asked about
  # whatever the cockpit is doing - it has a pid and not a pane, so the answer needs no server.
  ritual_headless_running "$slug" &&
    die "ritual remove: $slug is running headless right now, and that run writes its notes as it finishes - it has no pane to close, so wait for it (gensokyo ritual log $slug says when it started)"
  # Asked without a cockpit up there are no panes for a run to be alive in, so the pane question
  # is only worth asking with one.
  if server_running; then
    ritual_running "$slug" &&
      die "ritual remove: $slug is running now (gensokyo list), and that run writes its notes as it finishes - close it first, or wait for it"
    open_run=$(rit_open_run "$slug") || open_run=''
  fi
  rm -f "$path" || die "ritual remove: could not delete $(tilde "$path")"
  say "$slug is gone, and $(tilde "$path") with it"
  d=$(ritual_dir "$slug")
  if [ -d "$d" ]; then
    rm -rf "$d" || die "ritual remove: could not delete $(tilde "$d")"
    say "  its notes and its journal went too, from $(tilde "$d")"
  fi
  # The copy hid a shipped example of the same name, and the moment it goes the example is back
  # in the listing. Said out loud: a name still there after a delete reads as a delete that did
  # not work, and the next thing the user does about it is delete it again.
  [ -f "$SHARE/rituals/$slug.md" ] &&
    say "  the example gensokyo ships with that name is in the listing again"
  [ -n "$open_run" ] &&
    say "  a run of it is still in a tab: close $open_run, or what gets typed there writes its notes back"
  return 0
}

# log <name> [-n N]: the ritual's own journal - every fire, every run skipped because the last
# one was still going, and every complaint about a line gensokyo could not read.
ritual_cmd_log() {
  local name='' n=20 path d ts text newest
  while [ $# -gt 0 ]; do
    case $1 in
      -n|--lines) n=${2:-}; shift ;;
      --all) n=0 ;;
      -*) die "ritual log: unknown option $1 (usage: gensokyo ritual log <name> [-n N] [--all])" ;;
      *) [ -z "$name" ] || die "ritual log: one ritual at a time"; name=$1 ;;
    esac
    shift
  done
  [ -n "$name" ] || die "ritual log: which one? (gensokyo ritual lists them)"
  case $n in ''|*[!0-9]*) die "ritual log: -n takes a number of lines" ;; esac
  path=$(find_ritual "$name") || rit_no_such 'ritual log' "$name"
  ritual_load "$path" || die "ritual log: could not read $(tilde "$path")"
  d=$(ritual_dir "$RIT_slug")
  [ -f "$d/log" ] || {
    say "$RIT_slug has not done anything yet (gensokyo ritual run $RIT_slug fires it now)"; return 0; }
  while IFS=$'\t' read -r ts text; do
    [ -n "$ts" ] || continue
    printf '  %s  %s\n' "$(ritual_when "$ts")" "$text"
  done <<EOF
$(if [ "$n" -gt 0 ]; then tail -n "$n" "$d/log"; else cat "$d/log"; fi)
EOF
  say
  say "  $(tilde "$d/log")   (its notes are beside it, in memory.md)"
  # A headless run's own words are not in the journal - it keeps the first line of them - so the
  # file that has all of it is named as well, and the newest is the one somebody asking means.
  newest=$(rit_newest_run "$RIT_slug")
  [ -n "$newest" ] && say "  $(tilde "$newest")   (what the last headless run said, in full)"
  return 0
}

# edit <name>: the file, in the user's own editor. The other way to write a ritual, for people
# who would rather see the whole file than name its parts one option at a time.
ritual_cmd_edit() {
  local name=${1:-} src path
  [ -n "$name" ] || die "ritual edit: which one? (gensokyo ritual lists them)"
  [ $# -le 1 ] || die "ritual edit: one ritual at a time"
  src=$(find_ritual "$name") || rit_no_such 'ritual edit' "$name"
  ensure_dirs
  path=$(rit_mine "$src") ||
    die "ritual edit: could not copy $(tilde "$src") into $(tilde "$CONFIG_DIR/rituals")"
  [ "$path" = "$src" ] ||
    say "the shipped ${path##*/} is now yours, in $(tilde "$path")"
  rit_open_editor "$path" || die "ritual edit: ${VISUAL:-${EDITOR:-vi}} could not open $(tilde "$path")"
  rit_report "$path"
}

# new <name>: a ritual to fill in, from a template that says what every line is for. It arrives
# disabled: a half-written ritual should not fire at midnight because the file was saved.
ritual_cmd_new() {
  local name=${1:-} path
  [ -n "$name" ] || die "ritual new: a name for it, which is its file name too"
  [ $# -le 1 ] || die "ritual new: one name only"
  ritual_name_ok "$name" ||
    die "ritual new: '$name' cannot be a ritual name (a letter or digit first, then letters, digits, . _ -)"
  ensure_dirs
  path=$CONFIG_DIR/rituals/$name.md
  [ -f "$path" ] && die "ritual new: $name is already there: gensokyo ritual edit $name"
  mkdir -p "$CONFIG_DIR/rituals" || die "ritual new: could not make $(tilde "$CONFIG_DIR/rituals")"
  rit_template "$name" "$PWD" > "$path" || die "ritual new: could not write $(tilde "$path")"
  say "wrote $(tilde "$path")"
  rit_open_editor "$path" || die "ritual new: ${VISUAL:-${EDITOR:-vi}} could not open $(tilde "$path")"
  rit_report "$path"
}

# rit_template <name> <cwd>: the file `new` opens. The lines that are commented out are the
# defaults written down - leaving them out is the same as what they say, and a reader should not
# have to go looking for what they could set.
rit_template() {
  cat <<EOF
---
name: $1
description: what this ritual is for, in one line
schedule: "0 9 * * 1-5"     # five cron fields, local time; also @hourly @daily @weekly, "every 30m"
cwd: "$2"                   # the directory the run works in
# model: haiku              # left out: whatever your Claude Code starts with
# effort: low
# mode: acceptEdits         # a run nobody is watching should not have to be asked
# allowed_tools: ["Read", "Grep"]
# target: new               # or a resident's name, or persistent for one session it keeps
# keep: 2h                  # how long the finished run stays in its tab (or forever)
# overlap: skip             # or queue / parallel, if the last run is still going
# headless: true            # no pane at all: it runs in the background and logs what it said
# catch_up: true            # one run for a fire missed while the machine slept
enabled: false              # true once you are happy with it
---
What the resident should do, written for a session that has never seen this job
before: it starts fresh every time. It is told where its notes from previous
runs are - it reads them first, and updates them before it finishes.
EOF
}
