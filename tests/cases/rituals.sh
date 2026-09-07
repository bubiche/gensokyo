# tests/cases/rituals.sh - the schedule line (lib/cron.sh) and the ritual file (lib/rituals.sh):
# what a spec means, which minute it means next, and what a ritual file says. Sourced by
# tests/run.sh, which holds the harness, the scratch state dir and the sourced bin/gensokyo
# these tests call. The times are fixed epochs in the runner's fixed TZ.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script

# rt_at <local time>: the epoch of a written-out local time. rt_when: back the other way, so a
# failure reads as a date rather than as ten digits.
rt_at()   { date -j -f '%Y-%m-%d %H:%M:%S' "$1" +%s; }
rt_when() { date -r "$1" '+%Y-%m-%d %H:%M %a'; }
rt_next() { local e; e=$(cron_next "$1" "$2") || { printf 'never\n'; return 0; }; rt_when "$e"; }
rt_prev() { local e; e=$(cron_prev "$1" "${2:-}" ${3:+"$3"}) || { printf 'never\n'; return 0; }; rt_when "$e"; }

ritual_tests() {
  local mon out mine=$CONFIG_DIR/rituals was_share was_now
  mkdir -p "$mine"
  mon=$(rt_at '2026-09-07 09:05:00')   # a Monday morning, the Slack example's own hour

  t "cron_desugar: the shorthands, and the five fields left alone"
  assert_eq "$(cron_desugar '@hourly')" '0 * * * *'
  assert_eq "$(cron_desugar '@daily')" '0 0 * * *'
  assert_eq "$(cron_desugar '@midnight')" '0 0 * * *'
  assert_eq "$(cron_desugar '@weekly')" '0 0 * * 0'
  assert_eq "$(cron_desugar '@monthly')" '0 0 1 * *'
  assert_eq "$(cron_desugar '3 9 * * 1-5')" '3 9 * * 1-5'
  assert_eq "$(cron_desugar '  3   9 * * 1-5 ')" '3 9 * * 1-5'
  assert_fails cron_desugar '@nightly'
  assert_fails cron_desugar '1 2 3'
  assert_fails cron_desugar '1 2 3 4 5 6'
  assert_fails cron_desugar ''

  t "cron_desugar: 'every 30m', and the lengths that would not divide their unit evenly"
  assert_eq "$(cron_desugar 'every 30m')" '*/30 * * * *'
  assert_eq "$(cron_desugar 'every 5 minutes')" '*/5 * * * *'
  assert_eq "$(cron_desugar 'every 2h')" '0 */2 * * *'
  assert_eq "$(cron_desugar 'every 1h')" '0 */1 * * *'
  # :00 and :45 and then a wait of fifteen minutes is not what anyone means by "every 45m".
  assert_fails cron_desugar 'every 45m'
  assert_fails cron_desugar 'every 7h'
  assert_fails cron_desugar 'every 90m'
  assert_fails cron_desugar 'every m'
  assert_fails cron_desugar 'every 30 fortnights'

  t "cron_expand: stars, lists, ranges and steps into the set membership is read from"
  cron_expand '*/15' 0 59; assert_eq "$CRON_SET" ' 0 15 30 45 '
  cron_expand '1-9/3' 0 59; assert_eq "$CRON_SET" ' 1 4 7 '
  cron_expand '5/10' 0 59; assert_eq "$CRON_SET" ' 5 15 25 35 45 55 '   # vixie: from 5 to the top
  cron_expand '0,30,0' 0 59; assert_eq "$CRON_SET" ' 0 30 '             # said twice, held once
  cron_expand '08' 0 23; assert_eq "$CRON_SET" ' 8 '                    # a written-out hour
  cron_expand '*' 1 12; assert_eq "$CRON_SET" ' 1 2 3 4 5 6 7 8 9 10 11 12 '
  assert_fails cron_expand '60' 0 59
  assert_fails cron_expand '9-5' 0 59
  assert_fails cron_expand 'mon' 0 7
  assert_fails cron_expand '*/0' 0 59
  assert_fails cron_expand '1,,2' 0 59
  assert_fails cron_expand '' 0 59

  t "cron_expand_dow: 7 and 0 are the same Sunday, which is the only one date says"
  cron_expand_dow '7'; assert_eq "$CRON_SET" ' 0 '
  cron_expand_dow '5-7'; assert_eq "$CRON_SET" ' 5 6 0 '
  cron_expand_dow '0,7'; assert_eq "$CRON_SET" ' 0 '
  assert_fails cron_expand_dow '8'

  t "cron_ok: a spec that reads, whatever the time says about it"
  assert_ok cron_ok '3 9 * * 1-5'
  assert_ok cron_ok 'every 30m'
  assert_ok cron_ok '0 0 30 2 *'      # reads perfectly; simply never comes round
  assert_fails cron_ok '0 0 * * mon'
  assert_fails cron_ok '0 25 * * *'
  assert_fails cron_ok '0 0 0 * *'

  t "cron_match: the minute a spec means, and the minutes it does not"
  assert_ok   cron_match '5 9 * * 1' "$mon"
  assert_ok   cron_match '3-9 9 * * *' "$mon"
  assert_ok   cron_match 'every 5m' "$mon"
  assert_ok   cron_match '@daily' "$(rt_at '2026-09-07 00:00:00')"
  assert_fails cron_match '5 9 * * 2' "$mon"        # a Tuesday, and this is a Monday
  assert_fails cron_match '3 9 * * 1-5' "$mon"      # :03, and this is :05
  assert_fails cron_match '5 10 * * *' "$mon"
  assert_fails cron_match '5 9 * 10 *' "$mon"
  assert_fails cron_match 'every 45m' "$mon"        # unreadable, so it never fires

  t "cron_match: the two day fields are one condition, the way a crontab means it"
  # vixie: both fields saying something means either one fires. `0 0 13 * 5` is the 13th and
  # every Friday, not Friday the 13th - and 7 September 2026 is a Monday.
  assert_ok    cron_match '5 9 13 * 1' "$mon"
  assert_fails cron_match '5 9 13 * *' "$mon"
  assert_ok    cron_match '5 9 7 * 5' "$mon"
  # A day field beginning with `*` is a star to that rule, so this one is an AND: an odd day
  # of the month that is also a Monday.
  assert_ok    cron_match '5 9 */2 * 1' "$mon"
  assert_fails cron_match '5 9 */2 * 2' "$mon"

  t "cron_next: the nearest minute after a given one"
  assert_eq "$(rt_next '3 9 * * 1-5' "$mon")" '2026-09-08 09:03 Tue'
  assert_eq "$(rt_next '5 9 * * 1' "$mon")" '2026-09-14 09:05 Mon'
  assert_eq "$(rt_next 'every 15m' "$mon")" '2026-09-07 09:15 Mon'
  assert_eq "$(rt_next '@daily' "$mon")" '2026-09-08 00:00 Tue'
  assert_eq "$(rt_next '30 8 1 * *' "$mon")" '2026-10-01 08:30 Thu'
  assert_eq "$(rt_next '0 0 * * 0' "$mon")" '2026-09-13 00:00 Sun'
  # The minute a spec is asked about is behind it: next means next, so this is a week away.
  assert_eq "$(rt_next '5 9 * * 1' "$(rt_at '2026-09-07 09:04:59')")" '2026-09-07 09:05 Mon'
  # A leap day is reached (a walk of eighteen months) and 30 February never is.
  assert_eq "$(rt_next '0 0 29 2 *' "$mon")" '2028-02-29 00:00 Tue'
  assert_eq "$(rt_next '0 0 30 2 *' "$mon")" never
  assert_fails cron_next 'every 45m' "$mon"

  t "cron_prev: the last minute that fired, which is what a catch-up run asks about"
  assert_eq "$(rt_prev '3 9 * * 1-5' "$mon")" '2026-09-07 09:03 Mon'
  assert_eq "$(rt_prev '@daily' "$mon")" '2026-09-07 00:00 Mon'
  assert_eq "$(rt_prev '30 8 1 * *' "$mon")" '2026-09-01 08:30 Tue'
  assert_eq "$(rt_prev '0 0 29 2 *' "$mon")" '2024-02-29 00:00 Thu'
  # At or before, not before: the minute it is asked about counts, so a fire is not missed by
  # asking about it in its own minute.
  assert_eq "$(rt_prev '5 9 * * *' "$mon")" '2026-09-07 09:05 Mon'
  # Its own window, so a catch-up does not walk four years back for a fire it would refuse -
  # and inside the window it still answers, which is the call a catch-up run actually makes:
  # asked at 09:05 on Monday, the daily fire it has to catch up on is this morning's midnight.
  assert_eq "$(rt_prev '@daily' "$mon" 7)" '2026-09-07 00:00 Mon'
  assert_eq "$(rt_prev '0 0 * * 0' "$mon" 7)" '2026-09-06 00:00 Sun'
  assert_eq "$(rt_prev '0 0 1 1 *' "$mon" 7)" never
  assert_eq "$(rt_prev '0 0 1 1 *' "$mon")" '2026-01-01 00:00 Thu'

  t "the fields are read, never expanded: a directory full of files is not a schedule"
  # `set -- $spec` would hand `*` to the shell's globbing and get the files back instead. It
  # did, for one afternoon, so the spec is walked from here on.
  mkdir -p "$scratch/globtrap" && : > "$scratch/globtrap/9" && : > "$scratch/globtrap/x"
  out=$(cd "$scratch/globtrap" && cron_desugar '3 9 * * 1-5')
  assert_eq "$out" '3 9 * * 1-5'
  out=$(cd "$scratch/globtrap" && { cron_match '5 9 * * 1' "$mon" && printf yes; })
  assert_eq "$out" yes

  t "ritual_load: the frontmatter into fields, the rest into the prompt"
  cat > "$mine/probe.md" <<'EOF'
---
name: probe
description: a ritual to read
schedule: "3 9 * * 1-5"      # weekdays, just after nine
cwd: ~/
model: sonnet
effort: medium
mode: acceptEdits
allowed_tools: ["mcp__claude_ai_Slack__*", "Read", Grep]
keep: 2h
overlap: skip
headless: false
catch_up: true
enabled: true
# a comment of its own
no colon here
---

Check Slack and say what needs a reply.
Then update your notes.
EOF
  ritual_load "$mine/probe.md"
  assert_eq "$RIT_slug" probe
  assert_eq "$RIT_description" 'a ritual to read'
  assert_eq "$RIT_schedule" '3 9 * * 1-5'
  assert_eq "$RIT_cwd" "$HOME/"
  assert_eq "$RIT_model|$RIT_effort|$RIT_mode" 'sonnet|medium|acceptEdits'
  assert_eq "$RIT_keep|$RIT_overlap|$RIT_headless|$RIT_catch_up|$RIT_enabled" '2h|skip|false|true|true'
  assert_eq "$RIT_allowed" 'mcp__claude_ai_Slack__*
Read
Grep'
  assert_eq "$RIT_prompt" 'Check Slack and say what needs a reply.
Then update your notes.'
  assert_eq "$(ritual_problem)" ''

  t "ritual_load: a # after a space is a comment, the way it is in YAML"
  # Which means a description cannot carry a bare `#42` - YAML itself reads that as a comment
  # too - and quoting the value is the way out, for the same reason it is there.
  printf -- '---\nschedule: "@daily"\ndescription: bug #42 in the tracker\ncwd: %s\n---\nx\n' "$HOME" > "$mine/hash.md"
  ritual_load "$mine/hash.md"
  assert_eq "$RIT_description" bug
  printf -- '---\nschedule: "@daily"\ndescription: "bug #42 in the tracker"\ncwd: %s\n---\nx\n' "$HOME" > "$mine/hash.md"
  ritual_load "$mine/hash.md"
  assert_eq "$RIT_description" 'bug #42 in the tracker'
  rm -f "$mine/hash.md"

  t "ritual_load: the defaults a file does not have to write down"
  printf -- '---\nschedule: "@daily"\ncwd: %s\n---\nsomething\n' "$HOME" > "$mine/plain.md"
  ritual_load "$mine/plain.md"
  assert_eq "$RIT_target|$RIT_overlap|$RIT_headless|$RIT_catch_up|$RIT_enabled" 'new|skip|false|true|true'
  assert_ok ritual_enabled
  assert_eq "$(ritual_problem)" ''

  t "ritual_load: allowed_tools written out as a list, one pattern per line"
  # What someone editing the file by hand writes - and a pattern with a space in it, which
  # arrives whole because the list is kept one per line all the way to the command line.
  cat > "$mine/block.md" <<'EOF'
---
schedule: "@daily"
allowed_tools:
  - Read
  - "Bash(npm run test:*)"
  - mcp__claude_ai_Slack__*
enabled: false
---
do the thing
EOF
  ritual_load "$mine/block.md"
  assert_eq "$RIT_allowed" 'Read
Bash(npm run test:*)
mcp__claude_ai_Slack__*'
  assert_fails ritual_enabled
  assert_eq "$RIT_unknown" ''

  t "ritual_load: a file with no frontmatter is all prompt, and so has nothing to fire on"
  printf 'just the prompt\n' > "$mine/bare.md"
  ritual_load "$mine/bare.md"
  assert_eq "$RIT_prompt" 'just the prompt'
  assert_match "$(ritual_problem)" 'schedule: missing'
  assert_fails ritual_load "$mine/nothing-here.md"

  t "ritual_problem: the line that is wrong, said as a sentence"
  probe_with() {   # probe_with <frontmatter line>...: a ritual that is fine but for those
    { printf -- '---\n'; printf '%s\n' "$@"; printf -- '---\nwork to do\n'; } > "$mine/probe.md"
    ritual_load "$mine/probe.md"; ritual_problem
  }
  assert_eq "$(probe_with 'schedule: "@daily"' "cwd: $HOME")" ''
  assert_match "$(probe_with 'name: something-else' 'schedule: "@daily"' "cwd: $HOME")" "is not the file's name"
  assert_match "$(probe_with "cwd: $HOME")" 'schedule: missing'
  assert_match "$(probe_with 'schedule: "0 0 * * mon"' "cwd: $HOME")" 'not one gensokyo can read'
  assert_match "$(probe_with 'schedule: "0 0 30 2 *"' "cwd: $HOME")" 'never comes round'
  assert_match "$(probe_with 'schedule: "@daily"')" 'cwd: missing'
  assert_match "$(probe_with 'schedule: "@daily"' 'cwd: /no/such/place')" 'is not a directory'
  assert_match "$(probe_with 'schedule: "@daily"' 'cwd: dev/mozart')" 'is not a full path'
  assert_eq "$(probe_with 'schedule: "@daily"' 'cwd: ~')" ''      # a ~ is a full path once expanded
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'target: persistent')" 'target: persistent is not wired up yet'
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'overlap: queue')" 'overlap: queue is not wired up yet'
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'overlap: sideways')" 'not a thing to do about a run'
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'keep: 2 hours')" 'keep: 2 hours is not a length'
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'enabled: ture')" 'enabled: ture is neither'
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'headless: true')" 'headless: true is not wired up yet'
  assert_match "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'schedul: "@daily"')" 'not a ritual setting: schedul'
  assert_eq "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'keep: forever')" ''
  assert_eq "$(probe_with 'schedule: "@daily"' "cwd: $HOME" 'keep: 90m')" ''
  # A prompt is the point of the file, and a file that is only frontmatter has none.
  printf -- '---\nschedule: "@daily"\ncwd: %s\n---\n' "$HOME" > "$mine/probe.md"
  ritual_load "$mine/probe.md"
  assert_match "$(ritual_problem)" 'no prompt'
  rm -f "$mine/probe.md"
  unset -f probe_with

  t "ritual names: what can be a file name and a menu argument, and what cannot"
  assert_ok    ritual_name_ok slack-morning
  assert_ok    ritual_name_ok Nightly.Checks_2
  assert_fails ritual_name_ok 'my ritual'
  assert_fails ritual_name_ok -leading-dash
  printf -- '---\nschedule: "@daily"\n---\nx\n' > "$mine/not a ritual.md"
  assert_nomatch "$(ritual_files)" 'not a ritual.md'
  assert_match "$(ritual_unusable)" 'not a ritual.md'
  rm -f "$mine/not a ritual.md"

  t "ritual_files: the user's own shadow the shipped ones of the same name"
  was_share=$SHARE
  SHARE=$scratch/share-rituals; mkdir -p "$SHARE/rituals"
  printf -- '---\nschedule: "@daily"\ncwd: %s\n---\nthe shipped one\n' "$HOME" > "$SHARE/rituals/plain.md"
  printf -- '---\nschedule: "@weekly"\ncwd: %s\n---\nonly shipped\n' "$HOME" > "$SHARE/rituals/shipped-only.md"
  assert_eq "$(ritual_rows_sorted | cut -d'|' -f1 | tr '\n' ' ')" 'bare block plain shipped-only '
  assert_eq "$(ritual_rows_sorted | grep '^plain|' | cut -d'|' -f3)" '@daily'
  assert_eq "$(ritual_rows_sorted | grep -c '^plain|')" 1
  ritual_load "$(find_ritual plain)"
  assert_eq "$RIT_prompt" 'something'          # the user's file, not the shipped one
  assert_eq "$(ritual_rows_sorted | grep '^block|' | cut -d'|' -f2)" no

  t "find_ritual: the name, or enough of it to mean one ritual"
  assert_eq "$(find_ritual shipped-only)" "$SHARE/rituals/shipped-only.md"
  assert_eq "$(find_ritual shipped)" "$SHARE/rituals/shipped-only.md"
  assert_fails find_ritual nothing-of-the-sort
  printf -- '---\nschedule: "@daily"\ncwd: %s\n---\nx\n' "$HOME" > "$mine/shipped-too.md"
  assert_fails find_ritual shipped                       # two now, so it names neither
  rm -f "$mine/shipped-too.md"
  SHARE=$was_share
  rm -rf "$scratch/share-rituals"

  t "now_epoch: the clock a test can set, so the schedules can be asked about a fixed minute"
  was_now=${GENSOKYO_NOW:-}
  GENSOKYO_NOW=$mon
  assert_eq "$(now_epoch)" "$mon"
  GENSOKYO_NOW=''
  assert_re "$(now_epoch)" '^[0-9]{10}$'
  [ -z "$was_now" ] || GENSOKYO_NOW=$was_now
  rm -f "$mine"/*.md
}
