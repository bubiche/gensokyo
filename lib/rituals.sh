# lib/rituals.sh - rituals: the markdown files that say what to run and when, and the reading
# of one. Nothing here fires anything. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# A ritual is a file, the way a spell card is: rituals ship in share/rituals/ and the user's
# own live in ~/.config/gensokyo/rituals/, where a file of the same name shadows the shipped
# one. The file name is the ritual's name - what `ritual run` takes, what the last-run stamp
# and the resident's record are keyed by - so two rituals can never share one, and a `name:`
# line that disagrees with the file name is reported rather than obeyed.
#
# The frontmatter is YAML-shaped and read by hand, never evalled, for the same reason cards
# are: it is a text file the user writes, and nothing in it should be able to run.
RIT_slug='' RIT_path='' RIT_name='' RIT_description='' RIT_schedule='' RIT_target='' RIT_cwd=''
RIT_model='' RIT_effort='' RIT_mode='' RIT_allowed='' RIT_mcp='' RIT_keep='' RIT_overlap=''
RIT_headless='' RIT_catch_up='' RIT_enabled='' RIT_prompt='' RIT_unknown=''

# The same character rule spell cards go by, and for one more reason: the name is a file name
# as well, and `ritual add` makes the file from it.
ritual_name_ok() { slug_ok "$1"; }

# ritual_files: every usable ritual file, the user's before the shipped ones so that the first
# hit for a given file name wins.
ritual_files() {
  local d f base seen=$'\n'
  for d in "$CONFIG_DIR/rituals" "$SHARE/rituals"; do
    [ -d "$d" ] || continue
    for f in "$d"/*.md; do
      [ -f "$f" ] || continue
      base=${f##*/}
      ritual_name_ok "${base%.md}" || continue
      case $seen in *$'\n'"$base"$'\n'*) continue ;; esac
      seen="$seen$base"$'\n'
      printf '%s\n' "$f"
    done
  done
  return 0
}

# ritual_unusable: ritual files left out by the rule above, so nothing is skipped in silence.
ritual_unusable() {
  local d f base
  for d in "$CONFIG_DIR/rituals" "$SHARE/rituals"; do
    [ -d "$d" ] || continue
    for f in "$d"/*.md; do
      [ -f "$f" ] || continue
      base=${f##*/}
      ritual_name_ok "${base%.md}" || printf '%s\n' "$f"
    done
  done
  return 0
}

# rit_value <the part after the colon>: that, as a value. YAML enough for a file a person
# types: a quoted value ends at its closing quote, so the shipped template can write
# `schedule: "3 9 * * 1-5"   # weekdays` and mean the cron; an unquoted one loses a trailing
# ` #` comment; both lose the blanks around them. Everything is a string here - the fields
# that mean yes or no are read by rit_bool, and the schedule by lib/cron.sh.
rit_value() {
  local v=$1
  v=${v#"${v%%[![:space:]]*}"}
  case $v in
    \"*) v=${v#\"}; v=${v%%\"*} ;;
    \'*) v=${v#\'}; v=${v%%\'*} ;;
    '#'*) v='' ;;
    *) case $v in *' #'*) v=${v%% #*} ;; esac ;;
  esac
  printf '%s' "${v%"${v##*[![:space:]]}"}"
}

# rit_bool <word>: whether that word means yes. rit_bool_word says whether it means anything
# at all, which ritual_problem asks separately: `enabled: ture` reads as no here, and should be
# a complaint rather than a ritual that quietly never fires and never says why.
rit_bool()      { case $1 in true|True|TRUE|yes|Yes|YES|on|On|ON|1) return 0 ;; esac; return 1; }
rit_bool_word() {
  case $1 in
    true|True|TRUE|yes|Yes|YES|on|On|ON|1) return 0 ;;
    false|False|FALSE|no|No|NO|off|Off|OFF|0) return 0 ;;
  esac
  return 1
}

# rit_list <inline list>: `["a", "b"]` or a bare `a, b`, one item per line. Commas separate,
# so a tool pattern containing one has to go on a line of its own in the block form:
#
#   allowed_tools:
#     - Bash(npm run test:*)
#
# which ritual_load reads as well. The list is kept one item per line all the way to the
# command line, because a permission pattern can contain spaces and splitting on those would
# quietly turn one pattern into three.
rit_list() {
  local v=$1 item
  v=$(rit_value "$v")
  v=${v#[}; v=${v%]}
  while [ -n "$v" ]; do
    case $v in *,*) item=${v%%,*}; v=${v#*,} ;; *) item=$v; v='' ;; esac
    item=$(rit_value "$item")
    [ -n "$item" ] && printf '%s\n' "$item"
  done
  return 0
}

# The keys a ritual file may set. Anything else is a typo as far as a reader can tell - a
# `schedul:` line is a ritual that never fires - so unknown keys are collected and named.
RIT_KEYS=' name description schedule target cwd model effort mode permission_mode allowed_tools allowedTools mcp_config keep overlap headless catch_up enabled '

# ritual_load <file>: the ritual into RIT_*. A file with no frontmatter is all prompt, which
# leaves it without a schedule and so unable to fire; ritual_problem says so.
ritual_load() {
  local f=$1 line l key val item state=start list=''
  RIT_slug='' RIT_path='' RIT_name='' RIT_description='' RIT_schedule='' RIT_cwd=''
  RIT_model='' RIT_effort='' RIT_mode='' RIT_allowed='' RIT_mcp='' RIT_keep='' RIT_prompt=''
  RIT_unknown=''
  # The defaults, which are the answers the Slack example never has to write down: one fresh
  # resident in a pane, a run skipped rather than doubled, and on.
  RIT_target=new RIT_overlap=skip RIT_headless=false RIT_catch_up=true RIT_enabled=true
  [ -f "$f" ] || return 1
  RIT_path=$f
  RIT_slug=${f##*/}; RIT_slug=${RIT_slug%.md}
  while IFS= read -r line || [ -n "$line" ]; do
    case $state in
      start)
        case $line in
          '---') state=front; continue ;;
          *) state=body ;;
        esac ;;
      front)
        case $line in '---') state=body; continue ;; esac
        l=${line#"${line%%[![:space:]]*}"}
        case $l in ''|'#'*) continue ;; esac
        if [ -n "$list" ]; then
          case $l in
            -*) item=$(rit_value "${l#-}")
                [ -n "$item" ] && RIT_allowed="${RIT_allowed:+$RIT_allowed$'\n'}$item"
                continue ;;
          esac
          list=''
        fi
        key=${l%%:*}
        [ "$key" != "$l" ] || continue
        case $RIT_KEYS in
          *" $key "*) ;;
          *) RIT_unknown="${RIT_unknown:+$RIT_unknown, }$key"; continue ;;
        esac
        val=$(rit_value "${l#*:}")
        case $key in
          name)        RIT_name=$val ;;
          description) RIT_description=$val ;;
          schedule)    RIT_schedule=$val ;;
          target)      RIT_target=$val ;;
          cwd)         RIT_cwd=$val ;;
          model)       RIT_model=$val ;;
          effort)      RIT_effort=$val ;;
          mode|permission_mode) RIT_mode=$val ;;
          mcp_config)  RIT_mcp=$val ;;
          keep)        RIT_keep=$val ;;
          overlap)     RIT_overlap=$val ;;
          headless)    RIT_headless=$val ;;
          catch_up)    RIT_catch_up=$val ;;
          enabled)     RIT_enabled=$val ;;
          allowed_tools|allowedTools)
            # An empty value is the block form's opening line; the items are the `- ` lines
            # that follow, which is what someone editing the file by hand writes.
            if [ -n "$val" ]; then RIT_allowed=$(rit_list "$val"); else list=allowed; fi ;;
        esac
        continue ;;
    esac
    # Leading blank lines belong to the fence, not to the prompt.
    [ -n "$RIT_prompt" ] || [ -n "$line" ] || continue
    if [ -z "$RIT_prompt" ]; then RIT_prompt=$line; else RIT_prompt="$RIT_prompt"$'\n'"$line"; fi
  done < "$f"
  [ -n "$RIT_cwd" ] && RIT_cwd=${RIT_cwd/#\~/$HOME}
  return 0
}

# ritual_problem: what is wrong with the ritual just loaded, as a sentence to show the user,
# or nothing at all. `ritual add` refuses on it and the listing carries it, because a ritual
# with one bad line does not fire and nothing else would ever say why.
#
# The model, effort and permission mode are not checked against a list of allowed words:
# `gensokyo new` passes those straight through to Claude Code too, and a gensokyo that knows
# better than the installed claude which models exist would be wrong within a month.
ritual_problem() {
  ritual_name_ok "$RIT_slug" || {
    say "the file name is not a usable ritual name (a letter or digit first, then letters, digits, . _ -)"; return 0; }
  [ -z "$RIT_name" ] || [ "$RIT_name" = "$RIT_slug" ] || {
    say "name: $RIT_name is not the file's name ($RIT_slug.md is the ritual, and the file name wins)"; return 0; }
  [ -n "$RIT_schedule" ] || {
    say 'schedule: missing (five cron fields, @hourly @daily @weekly @monthly, or "every 30m")'; return 0; }
  cron_ok "$RIT_schedule" || {
    say "schedule: $RIT_schedule is not one gensokyo can read (five fields with * , - /, @daily, or \"every 30m\" with a length that divides the hour)"; return 0; }
  cron_next "$RIT_schedule" "$(now_epoch)" >/dev/null || {
    say "schedule: $RIT_schedule never comes round (a date that does not exist, like 30 February)"; return 0; }
  [ -n "$RIT_prompt" ] || {
    say 'no prompt: the lines under the frontmatter are what the resident is asked to do'; return 0; }
  case $RIT_target in
    new) ;;
    persistent) say 'target: persistent is not wired up yet; new starts a fresh resident per run'; return 0 ;;
    *) say "target: $RIT_target is not wired up yet; new starts a fresh resident per run"; return 0 ;;
  esac
  [ -n "$RIT_cwd" ] || { say 'cwd: missing (the directory the run works in)'; return 0; }
  # Nothing a ritual is read from has a directory of its own: the clock runs from wherever the
  # tmux server was started, so a relative path would mean one thing to the check and another
  # to the run. Refused rather than resolved against a directory nobody chose.
  case $RIT_cwd in
    /*) ;;
    *) say "cwd: $RIT_cwd is not a full path (a run starts from the clock, which is in no directory of yours: ~/... or /...)"; return 0 ;;
  esac
  [ -d "$RIT_cwd" ] || { say "cwd: $(tilde "$RIT_cwd") is not a directory"; return 0; }
  # A directory Claude Code has not been trusted in stops the run at its trust dialog, and a
  # stalled run is worse than a refused one: it is alive, so `overlap: skip` counts it as still
  # going and every later fire is skipped without a word. Said here instead, with the answer.
  dir_trusted "$RIT_cwd" || {
    say "cwd: nothing has answered Claude Code's trust prompt for $(tilde "$RIT_cwd") (open it once and accept, or the run stops at that dialog with nobody there to answer)"; return 0; }
  case $RIT_overlap in
    skip) ;;
    queue|parallel) say "overlap: $RIT_overlap is not wired up yet; skip leaves the running one alone"; return 0 ;;
    *) say "overlap: $RIT_overlap is not a thing to do about a run that is still going (skip)"; return 0 ;;
  esac
  case $RIT_keep in
    ''|forever|until_banished) ;;
    *[0-9][smhd]) case ${RIT_keep%?} in ''|*[!0-9]*) say "keep: $RIT_keep is not a length (30m, 2h, 1d, forever, until_banished)"; return 0 ;; esac ;;
    *) say "keep: $RIT_keep is not a length (30m, 2h, 1d, forever, until_banished)"; return 0 ;;
  esac
  rit_bool_word "$RIT_enabled"  || { say "enabled: $RIT_enabled is neither true nor false"; return 0; }
  rit_bool_word "$RIT_headless" || { say "headless: $RIT_headless is neither true nor false"; return 0; }
  rit_bool "$RIT_headless" && { say 'headless: true is not wired up yet; every run gets a pane you can watch'; return 0; }
  rit_bool_word "$RIT_catch_up" || { say "catch_up: $RIT_catch_up is neither true nor false"; return 0; }
  [ -z "$RIT_unknown" ] || { say "not a ritual setting: $RIT_unknown"; return 0; }
  return 0
}

ritual_enabled() { rit_bool "$RIT_enabled"; }

# ritual_rows: "slug|enabled|schedule|target|headless|description|path" per ritual, by name,
# which is the order every listing shows. The next fire is not in here: it costs a walk per
# row, and the caller that wants it has the schedule to ask cron_next with.
ritual_rows() {
  local f on
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    ritual_load "$f" || continue
    on=no; ritual_enabled && on=yes
    # The columns are cut apart with IFS='|', so the one field a person writes prose into does
    # not get to carry one.
    printf '%s|%s|%s|%s|%s|%s|%s\n' \
      "$RIT_slug" "$on" "$RIT_schedule" "$RIT_target" "$RIT_headless" "${RIT_description//|/ }" "$RIT_path"
  done <<EOF
$(ritual_files)
EOF
  return 0
}
ritual_rows_sorted() { ritual_rows | sort -t '|' -k1,1; }

# find_ritual <name>: the ritual's path, or nothing. The name is the file name, so this is an
# exact match on it; a part of one is accepted when it picks out exactly one ritual, the way
# find_card does, because `ritual run slack` should reach slack-morning.
find_ritual() {
  local want slug on sched target headless desc path part='' n=0
  want=$(lower "$1")
  while IFS='|' read -r slug on sched target headless desc path; do
    [ -n "$slug" ] || continue
    if [ "$(lower "$slug")" = "$want" ]; then printf '%s\n' "$path"; return 0; fi
    case $(lower "$slug") in *"$want"*) n=$((n + 1)); part=$path ;; esac
  done <<EOF
$(ritual_rows_sorted)
EOF
  [ "$n" -eq 1 ] || return 1
  printf '%s\n' "$part"
}

# ---------------------------------------------------------------- the timetable
# When each ritual comes round next, worked out once and kept in a file. The shrine redraws
# every few seconds, the bar is pushed with it and the timetable screen wants the answer for
# every ritual at once - and cron_next walks the schedule forward, which for a `0 0 30 2 *` is
# a four-year search. That is exactly the cost ritual_sweep's check order exists to keep out of
# the clock's loop, so it is paid here instead: when a ritual file has changed, or when the fire
# the file names has already gone by, and never otherwise.
#
#   slug|on|next fire epoch|schedule|description
#
# Soonest first, then the ones with no next fire - paused, or a schedule that never comes round
# - by name, which is the order a timetable is read in.
TIMETABLE=''   # the rows, once timetable_rows has read or rebuilt them

# timetable_source_mtime: the newest thing that could have changed a ritual - the two
# directories, so that a file added or deleted counts, and every ritual file in them. One stat
# for all of it: this is asked once per frame, and a fork per ritual would not be.
timetable_source_mtime() {
  local p newest=0 m
  set --
  for p in "$CONFIG_DIR/rituals" "$SHARE/rituals"; do
    [ -d "$p" ] && set -- "$@" "$p"
  done
  while IFS= read -r p; do
    [ -n "$p" ] && set -- "$@" "$p"
  done <<EOF
$(ritual_files)
EOF
  [ $# -gt 0 ] || { printf '0\n'; return 0; }
  for m in $(stat -f %m "$@" 2>/dev/null || stat -c %Y "$@" 2>/dev/null); do
    [ "$m" -gt "$newest" ] && newest=$m
  done
  printf '%s\n' "$newest"
}

# timetable_build: the rows, from the ritual files. cron_next is asked only about a ritual that
# is on: a paused one is not going to fire, so the minute it would have fired at is not a fact
# worth a walk - and every shipped example is paused.
timetable_build() {
  local slug on sched target headless desc path next soon='' rest=''
  while IFS='|' read -r slug on sched target headless desc path; do
    [ -n "$slug" ] || continue
    next=''
    if [ "$on" = yes ] && cron_ok "$sched"; then
      next=$(cron_next "$sched" "$(now_epoch)") || next=''
    fi
    if [ -n "$next" ]; then
      soon="$soon$slug|$on|$next|$sched|$desc"$'\n'
    else
      rest="$rest$slug|$on||$sched|$desc"$'\n'
    fi
  done <<EOF
$(ritual_rows_sorted)
EOF
  [ -z "$soon" ] || printf '%s' "$soon" | sort -t '|' -k3,3n
  [ -z "$rest" ] || printf '%s' "$rest"
  return 0
}

# timetable_rows: the timetable into TIMETABLE, rebuilt when it can no longer be right. Into a
# variable rather than printed, because the shrine wants it once a frame and a command
# substitution there is a fork a frame. A rebuild is written for whoever asks next; two of them
# racing (a frame and a bar push) write the same answer, and the rename is atomic either way.
timetable_rows() {
  local f=$STATE_DIR/timetable first next stale='' now
  TIMETABLE=''
  now=$(now_epoch)
  if [ -f "$f" ]; then
    TIMETABLE=$(cat "$f" 2>/dev/null)
    first=${TIMETABLE%%$'\n'*}
    next=${first#*|}; next=${next#*|}; next=${next%%|*}
    # Strictly newer, not as new: a ritual written in the same second as the cache is a ritual
    # the cache may have been built without, and `ritual add` then the shrine is one second.
    [ "$(mtime_of "$f")" -gt "$(timetable_source_mtime)" ] || stale=1
    case $next in
      ''|*[!0-9]*) ;;
      *) [ "$next" -gt "$now" ] || stale=1 ;;   # the fire it named has gone by
    esac
  else
    stale=1
  fi
  if [ -n "$stale" ]; then
    TIMETABLE=$(timetable_build)
    mkdir -p "$STATE_DIR" 2>/dev/null
    if ! printf '%s\n' "$TIMETABLE" > "$f.tmp.$$" || ! mv "$f.tmp.$$" "$f"; then rm -f "$f.tmp.$$"; fi
  fi
  return 0
}

# timetable_next: "slug|epoch|description" for the ritual that fires next, or nothing at all.
# The rows are soonest first, so it is the first one with a fire in it.
timetable_next() {
  local slug on next sched desc
  while IFS='|' read -r slug on next sched desc; do
    [ -n "$slug" ] && [ -n "$next" ] && { printf '%s|%s|%s' "$slug" "$next" "$desc"; return 0; }
  done <<EOF
$TIMETABLE
EOF
  return 0
}

# timetable_count: how many rituals there are at all, which is what the shrine says when none
# of them is on.
timetable_count() {
  local n=0
  [ -n "$TIMETABLE" ] && n=$(printf '%s\n' "$TIMETABLE" | grep -c '^')
  printf '%s' "$n"
}
