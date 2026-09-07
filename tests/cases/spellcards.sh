# tests/cases/spellcards.sh - spell cards: reading a card file, filling its placeholders in,
# who a cast reaches, and the shrine's three-step cast picker. Sourced by tests/run.sh, which
# holds the harness, the scratch state dir and the sourced bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

spellcard_tests() {
  local out f ttl mine=$CONFIG_DIR/spellcards
  ttl=$REGISTRY_TTL ttl2=''
  REGISTRY_TTL=86400   # the fixture registry below is the registry for this whole group
  mkdir -p "$mine"

  t "card_load: frontmatter into fields, the rest into the body, the fence's blank lines dropped"
  cat > "$mine/probe.md" <<'EOF'
---
title: Test Sign "Probe"
summary: a card to read
peer: required
nonsense: ignored
no colon here
---

first line
  second line
EOF
  card_load "$mine/probe.md"
  assert_eq "$CARD_slug" probe
  assert_eq "$CARD_title" 'Test Sign "Probe"'
  assert_eq "$CARD_summary" 'a card to read'
  assert_eq "$CARD_peer" required
  assert_eq "$CARD_body" 'first line
  second line'

  t "card_load: a file with no frontmatter is all prompt, and its title is its name"
  printf 'just the prompt\n' > "$mine/bare.md"
  card_load "$mine/bare.md"
  assert_eq "$CARD_title|$CARD_peer|$CARD_body" 'bare||just the prompt'
  assert_fails card_load "$mine/nothing-here.md"

  t "card names: what a tmux menu can carry through two layers of quoting, and what it cannot"
  assert_ok   card_name_ok status-report
  assert_ok   card_name_ok My.Card_2
  assert_fails card_name_ok 'my card'
  assert_fails card_name_ok "o'brien"
  assert_fails card_name_ok -leading-dash
  printf 'x\n' > "$mine/not a card.md"
  assert_match "$(card_files)" "$mine/probe.md"
  assert_nomatch "$(card_files)" 'not a card.md'
  assert_match "$(card_unusable)" 'not a card.md'
  assert_match "$(cmd_broadcast 2>&1)" 'not a usable card name'
  rm -f "$mine/not a card.md"

  t "card_rows: the shipped cards by title, and a user card of the same name shadowing one"
  rm -f "$mine/probe.md" "$mine/bare.md"
  assert_eq "$(card_rows_sorted | cut -d'|' -f1 | tr '\n' ' ')" 'sync-up second-opinion status-report wrap-up '
  printf -- '---\ntitle: Mine\n---\nmy own wrap up\n' > "$mine/wrap-up.md"
  assert_eq "$(card_rows_sorted | grep '^wrap-up|' | cut -d'|' -f2)" Mine
  assert_eq "$(card_rows_sorted | grep -c '^wrap-up|')" 1
  rm -f "$mine/wrap-up.md"

  t "find_card: slug, title, a substring when it picks out one card, and nothing when it does not"
  assert_eq "$(find_card status-report)" "$SHARE/spellcards/status-report.md"
  assert_eq "$(find_card 'Time Sign "Wrap Up"')" "$SHARE/spellcards/wrap-up.md"
  assert_eq "$(find_card second)" "$SHARE/spellcards/second-opinion.md"
  assert_eq "$(find_card STATUS-REPORT)" "$SHARE/spellcards/status-report.md"
  assert_fails find_card sign          # every shipped title has "Sign" in it
  assert_fails find_card nosuchcard

  t "card_expand: the four placeholders, every time each one appears"
  assert_eq "$(card_expand 'I am {self}; {peer} reviews {self} in {cwd}. Others: {residents}' \
    Reimu Youmu /w 'Marisa, Sakuya')" 'I am Reimu; Youmu reviews Reimu in /w. Others: Marisa, Sakuya'
  assert_eq "$(card_expand 'nothing to fill' A B C D)" 'nothing to fill'

  t "every shipped card expands whole: no placeholder survives, so none of them is misspelt"
  for f in "$SHARE"/spellcards/*.md; do
    card_load "$f"
    assert_nomatch "$(card_expand "$CARD_body" Reimu Youmu /w Marisa)" '{'
  done

  t "a card that asks for a reply names the channel it must come back on, not just its shape"
  for f in "$SHARE"/spellcards/*.md; do
    card_load "$f"
    case $CARD_body in
      *SendMessage*) assert_match "$CARD_body" '{self}' ;;
      *) ok ;;
    esac
  done
  card_load "$SHARE/spellcards/second-opinion.md"
  assert_match "$CARD_body" 'the reply must come back to you as a `SendMessage` addressed to {self}'
  card_load "$SHARE/spellcards/sync-up.md"
  # Sync Up now allows one reply each, so it has to say where a reply goes and where it stops.
  assert_match "$CARD_body" 'reply only if this touches your work'
  assert_match "$CARD_body" 'exactly one `SendMessage` tagged'
  assert_match "$CARD_body" 'Never answer a `[sync-up reply]`'

  t "cast_blocked: a dialog is what must never be typed into; a finished turn is not a dialog"
  fresh
  cat > "$REGISTRY" <<'EOF'
[{"pid": 1, "cwd": "/a", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000001", "name": "Reimu", "status": "idle"},
 {"pid": 2, "cwd": "/b", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000002", "name": "Marisa", "status": "waiting"},
 {"pid": 3, "cwd": "/c", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000003", "name": "Sakuya", "status": "busy"},
 {"pid": 4, "cwd": "/d", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000004", "name": "Youmu", "status": "idle"},
 {"pid": 6, "cwd": "/f", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000006", "name": "Alice", "status": "idle"},
 {"pid": 7, "cwd": "/g", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000007", "name": "Nitori", "status": "idle"}]
EOF
  REG=$'\n'$(registry_filter)
  rec aaaaaaaa-0000-4000-8000-000000000001 slot=1 name=Reimu cwd=/a pane=%1 window=@1
  rec aaaaaaaa-0000-4000-8000-000000000002 slot=2 name=Marisa cwd=/b pane=%2 window=@2
  rec aaaaaaaa-0000-4000-8000-000000000003 slot=3 name=Sakuya cwd=/c pane=%3 window=@3
  rec aaaaaaaa-0000-4000-8000-000000000004 slot=4 name=Youmu cwd=/d pane=%4 window=@4 departed=1
  rec aaaaaaaa-0000-4000-8000-000000000005 slot=5 name=Cirno cwd=/e pane=%5 window=@5   # not in the registry: starting
  rec aaaaaaaa-0000-4000-8000-000000000006 slot=6 name=Alice cwd=/f pane=%6 window=@6
  rec aaaaaaaa-0000-4000-8000-000000000007 slot=7 name=Nitori cwd=/g pane=%7 window=@7
  status_write aaaaaaaa-0000-4000-8000-000000000006 stopped 'done here'   # ✦, but nothing is open
  status_write aaaaaaaa-0000-4000-8000-000000000007 question 'which one?'
  assert_match "$(cast_blocked aaaaaaaa-0000-4000-8000-000000000002)" 'has a dialog waiting for you'
  assert_match "$(cast_blocked aaaaaaaa-0000-4000-8000-000000000007)" 'is asking you a question'
  assert_eq "$(cast_blocked aaaaaaaa-0000-4000-8000-000000000006)" ''
  assert_eq "$(cast_blocked aaaaaaaa-0000-4000-8000-000000000001)" ''
  # Cirno has no registry row. That is what a resident on the workspace trust dialog looks like
  # from here, and the Enter after a swallowed paste answers that dialog - measured 2026-09-07 to
  # grant the directory trust outright. Nothing may be typed at it, named or not.
  assert_match "$(cast_blocked aaaaaaaa-0000-4000-8000-000000000005)" 'is still starting up'

  t "cast_targets: the groups leave out departed, starting and anybody holding a dialog"
  assert_eq "$(cast_targets all | sed 's/.*-//' | tr '\n' ' ')" '000000000001 000000000003 000000000006 '
  assert_eq "$(cast_targets awaiting | sed 's/.*-//')" 000000000006
  assert_eq "$(cast_targets idle | sed 's/.*-//')" 000000000001
  assert_eq "$(cast_targets Sakuya)" aaaaaaaa-0000-4000-8000-000000000003
  assert_eq "$(cast_targets 2)" aaaaaaaa-0000-4000-8000-000000000002   # named, so it is the caster's to report
  assert_eq "$(cast_targets Youmu)" aaaaaaaa-0000-4000-8000-000000000004
  assert_fails cast_targets Nobody
  assert_match "$(cmd_broadcast status-report Marisa 2>&1)" 'Marisa has a dialog waiting for you; not cast at'
  assert_match "$(cmd_broadcast status-report Nitori 2>&1)" 'Nitori is asking you a question; not cast at'
  assert_match "$(cmd_broadcast second-opinion Reimu --with Cirno 2>&1)" 'Cirno is still starting'
  assert_match "$(cmd_broadcast status-report Cirno 2>&1)" 'Cirno is still starting up; not cast at'
  # A group used to drop a starting resident inside cast_targets, which left it out in silence
  # and hid the fact that cast_blocked could not see the dialog underneath. It is named now.
  assert_match "$(cmd_broadcast status-report all 2>&1)" 'Cirno is still starting up; left out'

  t "other_names: {residents} is everyone else who could be written to, and 'nobody' when alone"
  assert_eq "$(other_names aaaaaaaa-0000-4000-8000-000000000001)" 'Marisa, Sakuya, Alice, Nitori'
  assert_eq "$(other_names aaaaaaaa-0000-4000-8000-000000000003)" 'Reimu, Marisa, Alice, Nitori'
  assert_eq "$(other_names aaaaaaaa-0000-4000-8000-000000000004)" 'Reimu, Marisa, Sakuya, Alice, Nitori'
  fresh
  for f in 1 2 3 4 5 6 7; do drop_side_files "aaaaaaaa-0000-4000-8000-00000000000$f"; done
  rec aaaaaaaa-0000-4000-8000-000000000001 slot=1 name=Reimu cwd=/a pane=%1 window=@1
  assert_eq "$(other_names aaaaaaaa-0000-4000-8000-000000000001)" nobody

  t "cast_tail: the last line of a card, because the first has scrolled off by then"
  assert_eq "$(cast_tail 'abcdefghijklmnopqrstuvwxyz')" 'abcdefghijklmnopqrst'
  assert_eq "$(cast_tail short)" short
  assert_eq "$(cast_tail "$(printf 'one\ntwo')")" two
  assert_eq "$(cast_tail "$(printf 'one\ntwo\n\n  \n')")" two

  t "cast_type submits when the card reaches the input line, and only then"
  ttl2=$CAST_WAIT; CAST_WAIT=2
  out=$(
    FAKE_PANE='> '
    tmux_() {
      case $1 in
        (capture-pane) printf '%s\n' "$FAKE_PANE" ;;
        (load-buffer)  cat >/dev/null ;;
        (paste-buffer) FAKE_PANE='> [Pasted text #1 +3 lines]' ;;   # what Claude Code shows
        (send-keys)    printf 'ENTER\n' ;;
      esac
      return 0
    }
    cast_type %9 "$(printf 'one\ntwo\nthree\nfour')"; printf 'rc=%s\n' "$?"
  )
  assert_match "$out" ENTER
  assert_match "$out" rc=0

  t "cast_type sends no Enter when something else ate the paste: the pane changed, the card is gone"
  # The defect this replaces: the wait was for the pane to *differ*, and a dialog swallowing the
  # paste redraws the pane doing it. So the wait passed, the Enter answered the dialog and
  # broadcast reported a card it had lost. Measured 2026-09-07 against the workspace trust
  # dialog, where the Enter granted the directory trust.
  out=$(
    FAKE_PANE='> '
    tmux_() {
      case $1 in
        (capture-pane) printf '%s\n' "$FAKE_PANE" ;;
        (load-buffer)  cat >/dev/null ;;
        (paste-buffer) FAKE_PANE='Quick safety check: is this a project you trust?
  > No, exit
    Yes, I trust this folder' ;;
        (send-keys)    printf 'ENTER\n' ;;
      esac
      return 0
    }
    cast_type %9 "$(printf 'one\ntwo\nthree')"; printf 'rc=%s\n' "$?"
  )
  assert_nomatch "$out" ENTER
  assert_match "$out" rc=2

  t "cast_type also takes the plain echo, for anything that never asked for bracketed paste"
  # The stub claude does not enable bracketed-paste mode, so tmux sends the card as keystrokes
  # and the pane shows the first line rather than a paste marker. Real residents show the
  # marker. Either is proof the card arrived; a dialog shows neither.
  out=$(
    FAKE_PANE='> '
    tmux_() {
      case $1 in
        (capture-pane) printf '%s\n' "$FAKE_PANE" ;;
        (load-buffer)  cat >/dev/null ;;
        (paste-buffer) FAKE_PANE='> three' ;;
        (send-keys)    printf 'ENTER\n' ;;
      esac
      return 0
    }
    cast_type %9 "$(printf 'one\ntwo\nthree')"; printf 'rc=%s\n' "$?"
  )
  assert_match "$out" ENTER
  assert_match "$out" rc=0

  t "cast_type is not answered by the echo of its own last cast: the pane has to have changed"
  out=$(
    FAKE_PANE='three'
    tmux_() {
      case $1 in
        (capture-pane) printf '%s\n' "$FAKE_PANE" ;;
        (load-buffer)  cat >/dev/null ;;
        (paste-buffer) : ;;   # swallowed, and the last cast's echo is still on the screen
        (send-keys)    printf 'ENTER\n' ;;
      esac
      return 0
    }
    cast_type %9 "$(printf 'one\ntwo\nthree')"; printf 'rc=%s\n' "$?"
  )
  assert_nomatch "$out" ENTER
  assert_match "$out" rc=2
  CAST_WAIT=$ttl2

  t "broadcast: it says which card, who, and what it will not do, rather than guessing"
  assert_match "$(cmd_broadcast nosuchcard all 2>&1)" "no spell card 'nosuchcard'"
  assert_match "$(cmd_broadcast status-report 2>&1)" 'name who gets it'
  assert_match "$(cmd_broadcast status-report nobody-here 2>&1)" "no resident 'nobody-here'"
  assert_match "$(cmd_broadcast status-report all --with Reimu 2>&1)" 'names no peer'
  assert_match "$(cmd_broadcast second-opinion Reimu 2>&1)" 'needs a peer: --with'
  assert_match "$(cmd_broadcast second-opinion Reimu --with Nobody 2>&1)" "no resident 'Nobody' to be the peer"
  assert_match "$(cmd_broadcast second-opinion Reimu --with Reimu 2>&1)" 'cannot be its own peer'
  assert_match "$(cmd_broadcast status-report --whatever all 2>&1)" 'unknown option --whatever'

  t "broadcast: a pair card goes to one resident, because two of them cannot share one peer"
  cat > "$REGISTRY" <<'EOF'
[{"pid": 1, "cwd": "/a", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000001", "name": "Reimu", "status": "idle"},
 {"pid": 2, "cwd": "/b", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000002", "name": "Marisa", "status": "idle"}]
EOF
  REG=$'\n'$(registry_filter)
  rec aaaaaaaa-0000-4000-8000-000000000002 slot=2 name=Marisa cwd=/b pane=%2 window=@2
  assert_match "$(cmd_broadcast second-opinion all --with Marisa 2>&1)" 'is cast at one resident'
  # A departed resident is nobody's peer, and a pane gensokyo cannot type into is reported, not
  # counted: these records point at panes on no tmux server at all.
  rec aaaaaaaa-0000-4000-8000-000000000003 slot=3 name=Youmu cwd=/c pane=%3 window=@3 departed=1
  assert_match "$(cmd_broadcast second-opinion Reimu --with Youmu 2>&1)" 'Youmu has departed'
  out=$(cmd_broadcast status-report Reimu 2>&1)
  assert_match "$out" 'could not type into'
  assert_match "$out" 'reached nobody'

  t "shrine: the cast picker lists the cards, and a digit or a click picks one"
  fresh
  rec aaaaaaaa-0000-4000-8000-000000000001 slot=1 name=Reimu cwd=/a pane=%1 window=@1
  rec aaaaaaaa-0000-4000-8000-000000000002 slot=2 name=Marisa cwd=/b pane=%2 window=@2
  SHRINE_VIEW=main; SHRINE_SAID=''
  assert_eq "$(shrine_letter s)" cast
  shrine_do cast
  assert_eq "$SHRINE_VIEW|$SHRINE_ARG|$SHRINE_ARG2" 'cast||'
  shrine_render 200 24   # wide enough for the hint's whole path
  assert_match "$SHRINE_TEXT" "your own cards go in $(tilde "$CONFIG_DIR/spellcards")"
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  cast which spell card?'
  assert_match "$(shrine_at "$(shrine_map_find pick-card status-report)")" 'Spirit Sign "Status Report"'
  assert_match "$(shrine_at "$(shrine_map_find pick-card second-opinion)")" 'asks a peer'
  assert_eq "$(shrine_digit 1)" 'pick-card|sync-up'
  assert_eq "$(shrine_at "$(shrine_map_find cancel)")" '[ cancel q ]'

  t "shrine: then who gets it - the groups that have anybody in them, then one resident each"
  shrine_do pick-card status-report
  assert_eq "$SHRINE_VIEW|$SHRINE_ARG" 'cast-target|status-report'
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  cast Spirit Sign "Status Report" on?'
  assert_match "$(shrine_at "$(shrine_map_find pick-target all)")" 'everyone (2)'
  assert_eq "$(shrine_map_find pick-target awaiting)" ''   # nobody needs anybody
  assert_match "$(shrine_at "$(shrine_map_find pick-target Marisa)")" '2 ○ Marisa'
  assert_eq "$(shrine_digit 1)" 'pick-target|all'
  # ✦ from a finished turn nobody has read: castable, and its own group in the picker. A ✦ that
  # is a dialog would be left out of the list altogether, which is what cast_targets tests.
  status_write aaaaaaaa-0000-4000-8000-000000000001 stopped 'done here'
  rm -f "$REGISTRY"; cat > "$REGISTRY" <<'EOF'
[{"pid": 1, "cwd": "/a", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000001", "name": "Reimu", "status": "idle"},
 {"pid": 2, "cwd": "/b", "kind": "interactive", "startedAt": 1, "sessionId": "aaaaaaaa-0000-4000-8000-000000000002", "name": "Marisa", "status": "idle"}]
EOF
  REG=$'\n'$(registry_filter)
  shrine_render 100 24
  assert_match "$(shrine_at "$(shrine_map_find pick-target awaiting)")" 'everyone who needs you (1)'
  assert_match "$(shrine_at "$(shrine_map_find pick-target idle)")" 'everyone who is resting (1)'
  status_write aaaaaaaa-0000-4000-8000-000000000001 '' ''

  t "shrine: a pair card asks for the peer as well, and never offers the resident itself"
  shrine_do cancel
  shrine_do pick-card second-opinion
  shrine_do pick-target Reimu
  assert_eq "$SHRINE_VIEW|$SHRINE_ARG|$SHRINE_ARG2" 'cast-peer|second-opinion|Reimu'
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  who should Reimu ask?'
  assert_match "$(shrine_at "$(shrine_map_find pick-peer Marisa)")" '2 ○ Marisa'
  assert_eq "$(shrine_map_find pick-peer Reimu)" ''
  assert_eq "$(shrine_digit 1)" 'pick-peer|Marisa'
  assert_eq "$(shrine_letter q)" cancel
  shrine_do cancel
  assert_eq "$SHRINE_VIEW|$SHRINE_ARG|$SHRINE_ARG2" 'main||'

  t "the plain client's three menus: a card, then who, then the peer - all running broadcast"
  # tmux is replaced inside a subshell, so what would have been a display-menu is readable and
  # nothing leaks into the tests after this one.
  out=$( tmux_() { printf '%s\n' "$*"; }; cmd__menu-spell c0 )
  assert_match "$out" 'display-menu -c c0 -T  cast a spell card '
  assert_match "$out" "run-shell '$SELF _menu-cast-target c0 status-report'"
  assert_match "$out" 'Review Sign "Second Opinion"  (pair)'
  out=$( tmux_() { printf '%s\n' "$*"; }; cmd__menu-cast-target c0 status-report )
  assert_match "$out" 'display-menu -c c0 -T  Spirit Sign "Status Report" on '
  assert_match "$out" 'broadcast status-report all >/dev/null'
  assert_match "$out" 'broadcast status-report awaiting >/dev/null'
  assert_match "$out" 'broadcast status-report Marisa >/dev/null'
  out=$( tmux_() { printf '%s\n' "$*"; }; cmd__menu-cast-target c0 second-opinion )
  assert_nomatch "$out" ' all >/dev/null'          # a pair card has one target, never a group
  assert_match "$out" "run-shell '$SELF _menu-cast-peer c0 second-opinion Reimu'"
  out=$( tmux_() { printf '%s\n' "$*"; }; cmd__menu-cast-peer c0 second-opinion Reimu )
  assert_match "$out" 'broadcast second-opinion Reimu --with Marisa >/dev/null'
  assert_nomatch "$out" '--with Reimu'
  out=$( tmux_() { printf '%s\n' "$*"; }; cmd__menu-cast-target c0 nosuchcard )
  assert_match "$out" 'display-message -c c0 no spell card nosuchcard'

  t "shrine: a card with nobody to cast it at says so rather than drawing an empty list"
  fresh; SHRINE_VIEW=cast-target; SHRINE_ARG=status-report
  shrine_render 100 24
  assert_match "$SHRINE_TEXT" '  (nobody)'
  SHRINE_VIEW=main; SHRINE_ARG='' SHRINE_ARG2='' SHRINE_SAID=''
  rm -f "$REGISTRY"
  for f in 1 2 3 4 5 6 7; do drop_side_files "aaaaaaaa-0000-4000-8000-00000000000$f"; done
  REGISTRY_TTL=$ttl
}
