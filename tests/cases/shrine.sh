# tests/cases/shrine.sh - the shrine tab: what a frame draws, where a click lands, what a letter does.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

shrine_tests() {
  local f
  t "shrine: with nobody here the frame is the banner and the buttons, and nothing to focus"
  fresh; rm -f "$REGISTRY"
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" 'Nobody is here yet.'
  assert_match "$SHRINE_TEXT" '[ summon n ]  [ banish x ]  [ recall r ]  [ cast s ]  [ timetable t ]  [ ? ]'
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
  assert_eq "$(shrine_buttons | cut -d'|' -f2 | tr -d '\n')" 'nxrst?'

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
  assert_match "$SHRINE_TEXT" '  … 2 more (gensokyo list)'
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

  t "shrine: what is not built yet says so instead of doing nothing"
  shrine_do cast
  assert_match "$SHRINE_SAID" 'not available yet'
  shrine_render 80 24
  assert_match "$SHRINE_TEXT" 'not available yet'
  shrine_do cancel; SHRINE_SAID=''
}
