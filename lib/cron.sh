# lib/cron.sh - the schedule line of a ritual: what it means, whether it means this minute,
# and which minute it means next. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# Five fields, local time, and our own parser rather than a dependency: minute, hour, day of
# month, month, day of week. `*`, lists (`1,15`), ranges (`1-5`), steps (`*/15`, `1-9/2`,
# `5/10`), `0` or `7` for Sunday. No names (`mon`, `jan`), no `L`/`W`/`#`, no seconds field -
# the same subset Claude Code's own schedules take, so a spec that works in one works in both.
# A file the user writes should not be able to run, so nothing here is evalled: fields are
# picked apart with pattern matching and arithmetic only.
#
# Everything below takes and returns epoch seconds; the clock injects the time it fires
# against, so a test can hand this a fixed minute. Time arithmetic goes through `date`, which
# is BSD's here (macOS): `-r <epoch>` reads a time rather than a file, and `-j -f <format>`
# parses a local date without setting the system clock.

# cron_desugar <spec>: the spec as five fields, or failure. `@hourly` and friends are the
# usual crontab shorthands; `every 30m` is ours, for the schedules people say out loud, and
# only takes a length that divides its unit evenly - `every 45m` would fire at :00 and :45
# and then wait fifteen minutes, which is not what anyone means by it, so it is refused and
# `ritual add` says so. Lowercase only: the error names the forms that work.
cron_desugar() {
  local spec=$1 n unit
  case $spec in
    @hourly)           printf '0 * * * *\n'; return 0 ;;
    @daily|@midnight)  printf '0 0 * * *\n'; return 0 ;;
    @weekly)           printf '0 0 * * 0\n'; return 0 ;;
    @monthly)          printf '0 0 1 * *\n'; return 0 ;;
    @yearly|@annually) printf '0 0 1 1 *\n'; return 0 ;;
    @*) return 1 ;;
    'every '*)
      n=${spec#every }
      while :; do case $n in ' '*) n=${n# } ;; *) break ;; esac; done
      unit=${n##*[0-9]}          # what follows the digits: the unit, blanks and all
      n=${n%"$unit"}
      unit=${unit#"${unit%%[![:space:]]*}"}
      case $n in ''|*[!0-9]*) return 1 ;; esac
      n=$((10#$n))
      case $unit in
        m|min|mins|minute|minutes)
          { [ "$n" -ge 1 ] && [ "$n" -le 59 ] && [ $((60 % n)) -eq 0 ]; } || return 1
          printf '*/%s * * * *\n' "$n" ;;
        h|hr|hrs|hour|hours)
          { [ "$n" -ge 1 ] && [ "$n" -le 23 ] && [ $((24 % n)) -eq 0 ]; } || return 1
          printf '0 */%s * * *\n' "$n" ;;
        *) return 1 ;;
      esac
      return 0 ;;
  esac
  cron_fields "$spec" || return 1
  printf '%s %s %s %s %s\n' "$CRON_F1" "$CRON_F2" "$CRON_F3" "$CRON_F4" "$CRON_F5"
}

# cron_fields <spec>: the five fields into CRON_F1..CRON_F5, and failure unless there are
# exactly five. `read` rather than `set -- $spec`: an unquoted expansion hands the spec to the
# shell's globbing, and `*` - which every spec is made of - would come back as whatever files
# are in the current directory. That cost an afternoon; the fields are read, never expanded.
CRON_F1='' CRON_F2='' CRON_F3='' CRON_F4='' CRON_F5=''
cron_fields() {
  local extra=''
  CRON_F1='' CRON_F2='' CRON_F3='' CRON_F4='' CRON_F5=''
  IFS=$' \t' read -r CRON_F1 CRON_F2 CRON_F3 CRON_F4 CRON_F5 extra <<EOF
$1
EOF
  [ -n "$CRON_F5" ] && [ -z "$extra" ]
}

# cron_expand <field> <lowest> <highest>: the values that field stands for, into CRON_SET as
# " 0 15 30 45 " so that membership is one `case` and no process. A global rather than stdout
# because the matcher runs against every ritual and the walk below runs it per day, and a
# command substitution there would be a fork each time.
CRON_SET=''
cron_expand() {
  local spec=$1 lo=$2 hi=$3 part step a b v
  CRON_SET=' '
  case $spec in ''|*[!0-9,/*-]*) return 1 ;; esac
  while [ -n "$spec" ]; do
    case $spec in *,*) part=${spec%%,*}; spec=${spec#*,} ;; *) part=$spec; spec='' ;; esac
    step=1
    case $part in
      */*) step=${part##*/}; part=${part%%/*}
           case $step in ''|*[!0-9]*) return 1 ;; esac
           step=$((10#$step)); [ "$step" -ge 1 ] || return 1 ;;
    esac
    case $part in
      '*') a=$lo b=$hi ;;
      *-*) a=${part%%-*}; b=${part#*-} ;;
      # A bare number with a step is vixie's "from here to the top of the range", so `5/10`
      # in the minute field is :05 :15 :25 ... and `5` on its own is only :05.
      *)   a=$part; b=$part; [ "$step" -eq 1 ] || b=$hi ;;
    esac
    case $a in ''|*[!0-9]*) return 1 ;; esac
    case $b in ''|*[!0-9]*) return 1 ;; esac
    a=$((10#$a)); b=$((10#$b))
    { [ "$a" -ge "$lo" ] && [ "$b" -le "$hi" ] && [ "$a" -le "$b" ]; } || return 1
    v=$a
    while [ "$v" -le "$b" ]; do
      case $CRON_SET in *" $v "*) ;; *) CRON_SET="$CRON_SET$v " ;; esac
      v=$((v + step))
    done
  done
  [ "$CRON_SET" != ' ' ] || return 1
  return 0
}

# The day-of-week field, where 7 and 0 are both Sunday and `date +%w` only ever says 0.
cron_expand_dow() {
  cron_expand "$1" 0 7 || return 1
  case $CRON_SET in
    *' 7 '*) CRON_SET=${CRON_SET/ 7 / }
             case $CRON_SET in *' 0 '*) ;; *) CRON_SET="${CRON_SET}0 " ;; esac ;;
  esac
  return 0
}

cron_hit() { case $CRON_SET in *" $1 "*) return 0 ;; esac; return 1; }
# shellcheck disable=SC2086  # the set is a list of words, and splitting it is the point
cron_sorted() { printf '%s\n' $CRON_SET | sort -n | tr '\n' ' '; }

# cron_day_hit <day-of-month field> <day-of-week field> <day of month> <day of week>: whether
# the two day fields, which are one condition between them, admit that day.
#
# vixie's rule, which is what people's crontabs are written against: when both day fields say
# something, a day matching either one fires (`0 0 13 * 5` is the 13th and every Friday, not
# Friday the 13th); when one of them is a star, only the other counts. "Says something" is
# vixie's own test - the field does not begin with `*` - so `*/2` in a day field counts as a
# star, and `0 0 */2 * 5` is every Friday that is also an odd day of the month.
cron_day_hit() {
  local dom_ok='' dow_ok=''
  cron_expand "$1" 1 31 || return 1
  cron_hit "$3" && dom_ok=1
  cron_expand_dow "$2" || return 1
  cron_hit "$4" && dow_ok=1
  case $1 in
    '*'*) case $2 in
            '*'*) [ -n "$dom_ok" ] && [ -n "$dow_ok" ] ;;
            *)    [ -n "$dow_ok" ] ;;
          esac ;;
    *)    case $2 in
            '*'*) [ -n "$dom_ok" ] ;;
            *)    [ -n "$dom_ok" ] || [ -n "$dow_ok" ] ;;
          esac ;;
  esac
}

# cron_ok <spec>: parseable, without asking anything about the time. A spec can be parseable
# and still never fire (`0 0 30 2 *`), which is cron_next's answer to give, not this one's.
cron_ok() {
  local spec
  spec=$(cron_desugar "$1") || return 1
  cron_fields "$spec" || return 1
  cron_expand "$CRON_F1" 0 59 || return 1
  cron_expand "$CRON_F2" 0 23 || return 1
  cron_expand "$CRON_F3" 1 31 || return 1
  cron_expand "$CRON_F4" 1 12 || return 1
  cron_expand_dow "$CRON_F5"
}

# cron_match <spec> <epoch>: does this spec fire in the minute that epoch falls in.
cron_match() {
  local spec epoch mi ho dm mo dw
  epoch=$2
  spec=$(cron_desugar "$1") || return 1
  IFS=' ' read -r mi ho dm mo dw <<EOF
$(date -r "$epoch" '+%M %H %d %m %w')
EOF
  mi=$((10#$mi)) ho=$((10#$ho)) dm=$((10#$dm)) mo=$((10#$mo)) dw=$((10#$dw))
  cron_fields "$spec" || return 1
  { cron_expand "$CRON_F1" 0 59 && cron_hit "$mi"; } || return 1
  { cron_expand "$CRON_F2" 0 23 && cron_hit "$ho"; } || return 1
  { cron_expand "$CRON_F4" 1 12 && cron_hit "$mo"; } || return 1
  cron_day_hit "$CRON_F3" "$CRON_F5" "$dm" "$dw"
}

# cron_next <spec> <epoch> [days]: the epoch of the first minute after <epoch> that fires.
# cron_prev <spec> <epoch> [days]: the last minute at or before <epoch> that fired - which is
# what a catch-up run asks for, and it passes its own window rather than searching four years
# back for a fire it would refuse anyway.
#
# Both fail rather than print when nothing is found inside <days>: a spec can be perfectly
# valid and fire never (`0 0 30 2 *`) or once a decade, and a caller that prints "next fire"
# has to be able to say "never" instead. Four years is enough to reach a 29 February.
CRON_SEARCH_DAYS=1500
cron_next() { cron_walk "$1" "$(( $2 - $2 % 60 + 60 ))" fwd  "${3:-$CRON_SEARCH_DAYS}"; }
cron_prev() { cron_walk "$1" "$(( $2 - $2 % 60 ))"      back "${3:-$CRON_SEARCH_DAYS}"; }

# The walk is by day, not by minute: a minute-by-minute scan of four years is 2.1 million
# rounds, while the days are 1500 and almost always one - the fields say which days can fire
# at all, and on such a day the clock fields say which minute. The two clock lists are sorted
# once (backwards for cron_prev) and read in order, so the first hit is the nearest one.
cron_walk() {
  local spec dir=$3 days=$4 t0 e day sh sm mins hours n=0 first=1 h m y mon dom dow step
  spec=$(cron_desugar "$1") || return 1
  t0=$2
  IFS=' ' read -r day sh sm <<EOF
$(date -r "$t0" '+%Y-%m-%d %H %M')
EOF
  sh=$((10#$sh)) sm=$((10#$sm))
  cron_fields "$spec" || return 1
  cron_expand "$CRON_F1" 0 59 || return 1
  mins=$(cron_sorted)
  cron_expand "$CRON_F2" 0 23 || return 1
  hours=$(cron_sorted)
  if [ "$dir" = back ]; then
    # shellcheck disable=SC2086
    mins=$(printf '%s\n' $mins | sort -rn | tr '\n' ' ')
    # shellcheck disable=SC2086
    hours=$(printf '%s\n' $hours | sort -rn | tr '\n' ' ')
    step=-86400
  else
    step=86400
  fi
  # The day is carried as its noon, so that stepping by 86400 lands on the next day whatever
  # the clocks did overnight: a DST jump makes it 11:00 or 13:00 and never yesterday.
  e=$(date -j -f '%Y-%m-%d %H:%M:%S' "$day 12:00:00" +%s)
  while [ "$n" -lt "$days" ]; do
    IFS=' ' read -r day y mon dom dow <<EOF
$(date -r "$e" '+%Y-%m-%d %Y %m %d %w')
EOF
    y=$((10#$y)) mon=$((10#$mon)) dom=$((10#$dom)) dow=$((10#$dow))
    cron_expand "$CRON_F4" 1 12 || return 1
    if ! cron_hit "$mon"; then
      # The month is out, so every day in it is: step over the whole month rather than asking
      # `date` about each of its thirty days. `0 0 29 2 *` is otherwise a walk of two years,
      # one process per day, in front of a user waiting to be told when their ritual fires.
      if [ "$dir" = back ]; then
        n=$((n + dom)); e=$((e - dom * 86400))
      else
        m=$(( $(days_in_month "$y" "$mon") - dom + 1 ))
        n=$((n + m)); e=$((e + m * 86400))
      fi
      first=''
      continue
    fi
    if cron_day_hit "$CRON_F3" "$CRON_F5" "$dom" "$dow"; then
      for h in $hours; do
        if [ -n "$first" ]; then
          if [ "$dir" = back ]; then [ "$h" -le "$sh" ] || continue
          else [ "$h" -ge "$sh" ] || continue; fi
        fi
        for m in $mins; do
          if [ -n "$first" ] && [ "$h" -eq "$sh" ]; then
            if [ "$dir" = back ]; then [ "$m" -le "$sm" ] || continue
            else [ "$m" -ge "$sm" ] || continue; fi
          fi
          # An hour that local time skips (the spring DST jump) has no epoch of its own, and
          # `date` answers with the hour it became instead. That fire runs an hour off, once
          # a year, which is the price of schedules being in the time the user reads.
          date -j -f '%Y-%m-%d %H:%M:%S' "$(printf '%s %02d:%02d:00' "$day" "$h" "$m")" +%s
          return 0
        done
      done
    fi
    first=''
    e=$((e + step))
    n=$((n + 1))
  done
  return 1
}

# days_in_month <year> <month>, for the month skip above: Gregorian, so a century is a leap
# year only every four hundred years.
days_in_month() {
  case $2 in
    1|3|5|7|8|10|12) printf '31\n' ;;
    4|6|9|11)        printf '30\n' ;;
    *) if { [ $(($1 % 4)) -eq 0 ] && [ $(($1 % 100)) -ne 0 ]; } || [ $(($1 % 400)) -eq 0 ]
       then printf '29\n'; else printf '28\n'; fi ;;
  esac
}
