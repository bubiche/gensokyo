# tests/cases/shrine.sh - the shrine tab: what a frame draws, where a click lands, what a letter does.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

shrine_tests() {
  local f mon was_now
  # These tests say what a frame draws, so the timetable it draws from is written here rather
  # than read off whatever rituals this gensokyo ships: dated well ahead, so that nothing
  # rebuilds it from the files while a frame is being asserted on.
  shrine_timetable() {
    printf '%s' "${1:-}" > "$STATE_DIR/timetable"
    touch -t 203001010000 "$STATE_DIR/timetable"
  }
  shrine_timetable
  t "shrine: with nobody here the frame is the banner and the buttons, and nothing to focus"
  fresh; rm -f "$REGISTRY"
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" 'Nobody is here yet.'
  assert_match "$SHRINE_TEXT" '[ summon n ]  [ banish x ]  [ recall r ]  [ cast s ]  [ timetable t ]'
  assert_match "$SHRINE_TEXT" '[ reload l ]  [ quit q ]  [ ? ]'   # 80 columns is short of a single row
  assert_nomatch "$SHRINE_MAP" '|focus|'

  t "shrine: a line-block per resident, with its directory, branch and telemetry"
  fresh; rm -f "$REGISTRY"
  rec 11111111-aaaa-4000-8000-000000000001 slot=1 name=Reimu "cwd=$scratch/work/alpha" window=@1 pane=%1 mode=acceptEdits
  rec 22222222-bbbb-4000-8000-000000000002 slot=2 name=Marisa cwd=/tmp/mozart window=@2 pane=%2 departed=1
  mkdir -p "$scratch/work/alpha/.git"; echo 'ref: refs/heads/main' > "$scratch/work/alpha/.git/HEAD"
  printf 'model=Opus 5\nctx=42\neffort=high\ncache=88\ncost=0.42\nadvisor=opus\nat=%s\n' "$(date +%s)" \
    > "$TELE_DIR/11111111-aaaa-4000-8000-000000000001.kv"
  shrine_render 100 24
  assert_re "$SHRINE_TEXT" '^  1 ○ Reimu +alpha +⎇ main · Opus 5→⚖ Opus · high · accept-edits · ⚡88% · \$0\.42$'
  assert_re "$SHRINE_TEXT" '^  2 · Marisa +mozart +departed$'
  assert_match "$SHRINE_TEXT" "  $SHRINE_NAME"
  assert_match "$SHRINE_TEXT" '  ⏲ no rituals yet'
  assert_match "$SHRINE_TEXT" '  click a resident above to open its tab'

  t "shrine: the row map points at the line each resident was drawn on"
  assert_match "$(shrine_at "$(shrine_map_find focus 1)")" '1 ○ Reimu'
  assert_match "$(shrine_at "$(shrine_map_find focus 2)")" '2 · Marisa'
  assert_eq "$(shrine_map_find focus 7)" ''

  t "shrine: and at the columns each button covers, so a click on one is not a click on its neighbour"
  assert_eq "$(shrine_at "$(shrine_map_find summon)")" '[ summon n ]'
  assert_eq "$(shrine_at "$(shrine_map_find timetable)")" '[ timetable t ]'
  assert_eq "$(shrine_at "$(shrine_map_find help)")" '[ ? ]'
  assert_eq "$(shrine_buttons | cut -d'|' -f2 | tr -d '\n')" 'nxrstlq?'

  t "shrine: every button still has its own columns when the pane is too narrow for one row"
  shrine_render 44 24
  assert_eq "$(shrine_at "$(shrine_map_find summon)")" '[ summon n ]'
  assert_eq "$(shrine_at "$(shrine_map_find help)")" '[ ? ]'
  assert_ok test "$(shrine_map_find summon | cut -d'|' -f1)" -lt "$(shrine_map_find help | cut -d'|' -f1)"

  t "shrine: the frame never outgrows the pane; residents that do not fit are counted"
  fresh; rm -f "$REGISTRY"
  for f in 1 2 3 4 5 6 7; do rec "0000000$f-cccc-4000-8000-00000000000$f" "slot=$f" "name=R$f" cwd=/tmp window="@$f" "pane=%$f"; done
  shrine_render 80 14
  assert_eq "$(printf '%s\n' "$SHRINE_TEXT" | wc -l | tr -d ' ')" 14
  assert_match "$SHRINE_TEXT" '  … 3 more (gensokyo list)'
  assert_match "$SHRINE_TEXT" '[ summon n ]'
  assert_eq "$(shrine_map_find focus 6)" ''
  shrine_render 80 24                       # room for everyone: nothing is given up
  assert_nomatch "$SHRINE_TEXT" ' more (gensokyo list)'
  assert_match "$(shrine_at "$(shrine_map_find focus 7)")" '7 ○ R7'

  t "shrine: a pane too narrow for one buttons row does not push the frame past its height"
  shrine_render 60 14
  assert_ok test "$(printf '%s\n' "$SHRINE_TEXT" | wc -l | tr -d ' ')" -le 14
  assert_eq "$(shrine_at "$(shrine_map_find summon)")" '[ summon n ]'
  assert_match "$(shrine_at "$(shrine_map_find focus 1)")" '1 ○ R1'

  t "shrine: lines are cut to the width, or every row below a wrapped one is out of step"
  shrine_render 30 24
  assert_eq "$(shrine_widest)" 30
  assert_ok test "$(shrine_widest)" -le 30

  t "shrine: a wide character counts as the two columns it takes, not as one"
  assert_eq "$(shrine_text_width 'ab')" 2
  assert_eq "$(shrine_text_width '幻 想 郷')" 8
  assert_eq "$(shrine_text_width '  ⛩ gensokyo')" 13
  assert_eq "$(shrine_text_width '⎇ main · ○')" 10

  t "shrine: the torii is drawn whole or not at all; a pane too small for it keeps the buttons"
  fresh; rm -f "$REGISTRY"
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" '幻 想 郷'
  shrine_render 40 24                       # too narrow for the gate
  assert_nomatch "$SHRINE_TEXT" '幻 想 郷'
  assert_match "$SHRINE_TEXT" 'Nobody is here yet.'
  assert_match "$SHRINE_TEXT" '[ summon n ]'
  assert_ok test "$(shrine_widest)" -le 40
  shrine_render 80 12                       # too short for it
  assert_nomatch "$SHRINE_TEXT" '幻 想 郷'
  assert_match "$SHRINE_TEXT" '[ summon n ]'
  assert_ok test "$(printf '%s\n' "$SHRINE_TEXT" | wc -l | tr -d ' ')" -le 12

  t "shrine: a mouse report becomes a row and a column; anything else on that stream is dropped"
  shrine_typed 'q';                  assert_eq "$SHRINE_KEY|$SHRINE_CLICK" 'q|'
  shrine_typed $'\033';              assert_eq "$SHRINE_KEY|$SHRINE_CLICK" 'esc|'
  shrine_typed $'\033[I';            assert_eq "$SHRINE_KEY|$SHRINE_CLICK" '|'        # the pane got the focus
  shrine_typed $'\033[<0;59;8M';     assert_eq "$SHRINE_KEY|$SHRINE_CLICK" '|'        # the press: not ours to act on
  shrine_typed $'\033[<0;59;8m';     assert_eq "$SHRINE_KEY|$SHRINE_CLICK" '|8 59'    # the release is
  shrine_typed $'\033[<65;59;8m';    assert_eq "$SHRINE_KEY|$SHRINE_CLICK" '|'        # a scroll wheel

  t "shrine: a click is answered by what was drawn where it landed, and nowhere else"
  fresh; rm -f "$REGISTRY"
  rec 11111111-aaaa-4000-8000-000000000001 slot=1 name=Reimu cwd=/tmp/alpha window=@1 pane=%1
  rec 22222222-bbbb-4000-8000-000000000002 slot=4 name=Marisa cwd=/tmp/mozart window=@2 pane=%2
  SHRINE_VIEW=main; shrine_render 80 24
  assert_eq "$(shrine_hit "$(shrine_map_find focus 4 | cut -d'|' -f1)" 3)" 'focus|4'
  assert_eq "$(shrine_hit "$(shrine_map_find summon | cut -d'|' -f1)" "$(shrine_map_find summon | cut -d'|' -f2)")" 'summon|'
  assert_eq "$(shrine_hit 1 1)" ''            # the blank line above the first block
  assert_eq "$(shrine_hit 99 1)" ''           # below everything drawn

  t "shrine: a letter runs the button that carries it, and only on the screen that shows it"
  assert_eq "$(shrine_letter n)" summon
  assert_eq "$(shrine_letter '?')" help
  assert_eq "$(shrine_letter z)" ''
  SHRINE_VIEW=banish
  assert_eq "$(shrine_letter n)" ''           # not a summon behind the picker
  assert_eq "$(shrine_letter q)" cancel
  SHRINE_VIEW=confirm
  assert_eq "$(shrine_letter y)" banish-yes
  assert_eq "$(shrine_letter n)" cancel

  t "shrine: the summon picker offers the directories gensokyo has been used in, and another"
  SHRINE_VIEW=summon
  printf '%s\n/tmp\n' "$scratch/work/alpha" > "$STATE_DIR/recent-dirs"
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" '  summon a resident into'
  assert_match "$(shrine_at "$(shrine_map_find pick-dir "$scratch/work/alpha")")" '1  '
  assert_match "$(shrine_at "$(shrine_map_find pick-dir /tmp)")" '2  /tmp'
  assert_match "$(shrine_at "$(shrine_map_find pick-dir '')")" '3  other directory…'
  assert_eq "$(shrine_digit 2)" 'pick-dir|/tmp'
  assert_eq "$(shrine_at "$(shrine_map_find cancel)")" '[ cancel q ]'

  t "shrine: the banish picker lists who is here, and picking one asks before it does anything"
  SHRINE_VIEW=banish; shrine_render 80 24
  assert_match "$SHRINE_TEXT" '  banish which resident?'
  assert_match "$(shrine_at "$(shrine_map_find pick-banish 11111111-aaaa-4000-8000-000000000001)")" 'Reimu'
  assert_eq "$(shrine_digit 2)" 'pick-banish|22222222-bbbb-4000-8000-000000000002'
  shrine_do pick-banish 22222222-bbbb-4000-8000-000000000002
  assert_eq "$SHRINE_VIEW" confirm
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" '  banish Marisa?'
  assert_eq "$(shrine_at "$(shrine_map_find banish-yes)")" '[ yes y ]'
  shrine_do cancel
  assert_eq "$SHRINE_VIEW|$SHRINE_ARG" 'main|'

  t "shrine: quit asks before it closes anything, and its keys are y and n, not q"
  SHRINE_VIEW=main
  assert_eq "$(shrine_letter q)" quit
  shrine_do quit
  assert_eq "$SHRINE_VIEW" quit
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" '  close gensokyo?'
  assert_match "$SHRINE_TEXT" 'the next gensokyo offers them all back under recall'
  assert_eq "$(shrine_at "$(shrine_map_find quit-yes)")" '[ yes y ]'
  assert_eq "$(shrine_letter y)" quit-yes
  assert_eq "$(shrine_letter n)" cancel
  assert_eq "$(shrine_letter q)" ''            # a second q lands on the question, not on the quit
  shrine_do cancel
  assert_eq "$SHRINE_VIEW|$SHRINE_ARG" 'main|'

  t "shrine: the recall picker lists who has departed, newest first"
  rec_set "$RES_DIR/11111111-aaaa-4000-8000-000000000001" departed 1700000000
  SHRINE_VIEW=recall; shrine_render 80 24
  assert_match "$SHRINE_TEXT" '  recall which resident?'
  assert_match "$(shrine_at "$(shrine_map_find pick-recall 11111111-aaaa-4000-8000-000000000001)")" 'Reimu'
  assert_eq "$(shrine_digit 1)" 'pick-recall|11111111-aaaa-4000-8000-000000000001'

  t "shrine: a picker does not outgrow its pane either, and never numbers past nine"
  SHRINE_VIEW=summon
  : > "$STATE_DIR/recent-dirs"
  for f in 1 2 3 4 5 6 7 8 9; do mkdir -p "$scratch/d$f"; printf '%s\n' "$scratch/d$f" >> "$STATE_DIR/recent-dirs"; done
  shrine_render 80 10
  assert_ok test "$(printf '%s\n' "$SHRINE_TEXT" | wc -l | tr -d ' ')" -le 10
  assert_match "$SHRINE_TEXT" ' more'
  assert_eq "$(shrine_at "$(shrine_map_find cancel)")" '[ cancel q ]'
  shrine_render 80 24
  assert_ok test "$(printf '%s\n' "$SHRINE_TEXT" | wc -l | tr -d ' ')" -le 24
  assert_eq "$(shrine_digit 9)" 'pick-dir|'   # eight directories at most, then the way to name another
  assert_match "$(shrine_at "$(shrine_map_find pick-dir '')")" 'other directory…'
  SHRINE_VIEW=main

  t "shrine: the help screen says what every letter does, from the table the buttons come from"
  SHRINE_VIEW=help; shrine_render 80 24
  assert_match "$SHRINE_TEXT" '  n   start a resident in a directory of your choosing'
  assert_match "$SHRINE_TEXT" '  ?   this screen'
  assert_eq "$(shrine_at "$(shrine_map_find cancel)")" '[ cancel q ]'
  assert_ok test "$(printf '%s\n' "$SHRINE_TEXT" | wc -l | tr -d ' ')" -le 24
  SHRINE_VIEW=main

  t "shrine: the main screen's one line about the schedule is what fires next, and it is clickable"
  fresh; rm -f "$REGISTRY"
  mon=$(rt_at '2026-09-07 09:05:00'); was_now=${GENSOKYO_NOW:-}; GENSOKYO_NOW=$mon
  shrine_timetable "slack-morning|yes|$((mon + 3600))|3 9 * * 1-5|check Slack for me
nightly|no||@daily|the checks"
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  ⏲ next  slack-morning  2026-09-07 10:05 (in 1h) · 2 in all'
  assert_match "$(shrine_at "$(shrine_map_find timetable next)")" '⏲ next  slack-morning'
  # The row and the button run the same action; only the button is what `[ timetable t ]` is.
  assert_eq "$(shrine_at "$(shrine_map_find timetable)")" '[ timetable t ]'

  t "shrine: rituals with none of them on, and no rituals at all, each say which"
  shrine_timetable 'nightly|no||@daily|the checks'
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  ⏲ one ritual, and it is not on'
  shrine_timetable "a|no||@daily|
b|no||@daily|"
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  ⏲ 2 rituals, none of them on'
  shrine_timetable
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  ⏲ no rituals yet'
  assert_eq "$(shrine_map_find timetable next)" ''

  t "shrine: the timetable is every ritual, when each fires next, and which are not going to"
  shrine_timetable "slack-morning|yes|$((mon + 3600))|3 9 * * 1-5|check Slack for me
never|yes||0 0 30 2 *|a date that never comes
nightly|no||@daily|the checks"
  shrine_do timetable
  assert_eq "$SHRINE_VIEW" timetable
  shrine_render 100 24
  assert_re "$SHRINE_TEXT" '^  1  slack-morning +on +3 9 \* \* 1-5 +2026-09-07 10:05$'
  assert_re "$SHRINE_TEXT" '^  2  never +on +0 0 30 2 \* +on, but its schedule never comes round$'
  assert_re "$SHRINE_TEXT" '^  3  nightly +off +@daily +paused$'
  assert_match "$SHRINE_TEXT" '[ cancel q ]'

  t "shrine: with nothing scheduled the timetable says how to start one rather than '(nobody)'"
  shrine_timetable
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '(nothing is scheduled: gensokyo ritual new <name>)'
  assert_nomatch "$SHRINE_TEXT" '(nobody)'

  t "shrine: clicking a ritual opens what it is, when it fires and when it last ran"
  mkdir -p "$CONFIG_DIR/rituals"
  printf -- '---\nschedule: "3 9 * * 1-5"\ncwd: %s\ndescription: check Slack for me\n---\nlook at Slack\n' \
    "$HOME" > "$CONFIG_DIR/rituals/slack-morning.md"
  ritual_note slack-morning 'ran (due 2026-09-07 09:03)'
  shrine_timetable "slack-morning|yes|$((mon + 3600))|3 9 * * 1-5|check Slack for me"
  shrine_do timetable
  shrine_render 100 24
  f=$(shrine_digit 1)
  shrine_do "${f%%|*}" "${f#*|}"
  assert_eq "$SHRINE_VIEW" ritual
  assert_eq "$SHRINE_ARG" slack-morning
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  ⏲ slack-morning'
  assert_match "$SHRINE_TEXT" '  what it does check Slack for me'
  assert_match "$SHRINE_TEXT" '  schedule     3 9 * * 1-5'
  assert_match "$SHRINE_TEXT" '  next fire    2026-09-08 09:03 (in 23h)'
  assert_match "$SHRINE_TEXT" '  last ran     2026-09-07 09:05 (0s ago)'
  assert_match "$SHRINE_TEXT" '  in           ~'
  assert_match "$SHRINE_TEXT" '[ run now r ]  [ pause p ]  [ cancel q ]'

  t "shrine: and the letters on that screen are its own, not the shrine's behind it"
  assert_eq "$(shrine_letter r)" ritual-run
  assert_eq "$(shrine_letter p)" ritual-off
  assert_eq "$(shrine_letter q)" cancel
  assert_eq "$(shrine_letter n)" ''

  t "shrine: pausing one stays on its screen, where the button and the next fire have flipped"
  shrine_do ritual-off
  assert_eq "$SHRINE_VIEW" ritual
  assert_match "$SHRINE_SAID" 'slack-morning'
  assert_eq "$(rit_value "$(grep '^enabled:' "$CONFIG_DIR/rituals/slack-morning.md" | head -n 1 | cut -d: -f2-)")" false
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '[ run now r ]  [ unpause p ]  [ cancel q ]'
  assert_match "$SHRINE_TEXT" '  next fire    nothing: it is paused'
  assert_eq "$(shrine_letter p)" ritual-on

  t "shrine: a ritual that has gone since the timetable was drawn says so instead of drawing blanks"
  SHRINE_ARG=not-a-ritual
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" 'there is no ritual called not-a-ritual any more'
  assert_match "$SHRINE_TEXT" '[ cancel q ]'

  shrine_do cancel; SHRINE_SAID=''
  rm -f "$CONFIG_DIR/rituals"/*.md "$STATE_DIR/timetable"
  rm -rf "$STATE_DIR/rituals/slack-morning"
  GENSOKYO_NOW=$was_now
  unset -f shrine_timetable
}
