# lib/shrine.sh - the shrine tab: the first window of the session, so iTerm2's first tab, where
# gensokyo draws who is here and what you can do about it. A bash loop redraws the whole pane
# every few seconds and whenever a hook has news, and every redraw also records what is
# clickable where, so a click can be turned back into an action. Sourced by bin/gensokyo;
# bash 3.2.
# shellcheck shell=bash

SHRINE_TICK=3      # seconds between redraws when nothing else wakes the loop
SHRINE_TEXT=''     # the frame shrine_render drew: one line per screen row, no escapes
SHRINE_MAP=''      # what is clickable in it, one "row|first column|last column|action|argument"
                   # line per target; rows and columns are 1-based from the top left of the
                   # pane, which is the origin an SGR mouse report uses. tmux's own `mouse`
                   # option stays off: the reports are meant for this program, not for tmux
SHRINE_COLS=80     # the width shrine_render is drawing for; lines are cut to it, because a
                   # line that wraps would put every row below it out of step with the map
SHRINE_ROW=0       # rows drawn so far, so the next line knows its own row number
SHRINE_BUSY=''     # a redraw is in flight, or a question is up; do not draw over either
SHRINE_VIEW=main   # which screen is up: main, or the picker a button opened
SHRINE_ARG=''      # what that screen is about (the resident a confirmation names)
SHRINE_SAID=''     # one line under the buttons: what the last action said
SHRINE_KEY=''      # what shrine_event read: one character, or `esc`
SHRINE_CLICK=''    # ... or "row column", when it was a click
SHRINE_ANSWER=''   # the line shrine_ask read
SHRINE_CANCEL=''   # the user backed out of the question shrine_ask is holding

# One table drives the buttons row, the letters that do the same, and the help screen, so a
# button cannot appear without a letter or drift from what it runs. Fields:
# action|letter|the text on the button|what it does. The text is literal, and pure ASCII,
# because the buttons row is the one place where the column arithmetic has to be exact.
shrine_buttons() {
  cat <<'EOF'
summon|n|[ summon n ]|start a resident in a directory of your choosing
banish|x|[ banish x ]|ask a resident to /exit; its tab then shows the departed screen
recall|r|[ recall r ]|bring a departed resident back, with its name and transcript
cast|s|[ cast s ]|send one prompt to every resident at once
timetable|t|[ timetable t ]|the rituals: prompts on a schedule
help|?|[ ? ]|this screen
EOF
}

# The buttons the other screens carry instead. Same fields, so the same walk draws them.
shrine_cancel_buttons() { printf 'cancel|q|[ cancel q ]|back to the shrine\n'; }
shrine_confirm_buttons() {
  cat <<'EOF'
banish-yes|y|[ yes y ]|go ahead
cancel|n|[ no n ]|leave it alone
EOF
}
# The departed screen's, drawn by lib/residents.sh through the same walk in its own pane.
departed_buttons() {
  cat <<'EOF'
recall|r|[ recall r ]|start this session again where it left off
close|x|[ close x ]|let the pane go, and its tab with it
EOF
}

# ---------------------------------------------------------------- drawing
# shrine_render <columns> <rows>: draw one frame into SHRINE_TEXT and its click targets into
# SHRINE_MAP. Nothing is printed and no terminal state is touched, so this is the whole of the
# shrine that can be tested without a tmux server.
shrine_render() {
  local cols=$1 rows=$2
  SHRINE_TEXT='' SHRINE_MAP='' SHRINE_ROW=0
  SHRINE_COLS=$cols
  case $SHRINE_VIEW in
    summon)  shrine_view_pick "$rows" 'summon a resident into' pick-dir shrine_dir_rows \
               'click a directory, or press its number' ;;
    banish)  shrine_view_pick "$rows" 'banish which resident?' pick-banish shrine_here_rows \
               'it is asked to /exit, and can be recalled afterwards' ;;
    recall)  shrine_view_pick "$rows" 'recall which resident?' pick-recall shrine_gone_rows \
               'it comes back with its name and its transcript' ;;
    confirm) shrine_view_confirm ;;
    help)    shrine_view_help ;;
    *)       shrine_view_main "$rows" ;;
  esac
  shrine_fit "$rows"
  return 0
}

# shrine_fit <rows>: the last word on the frame's height. Every screen budgets its own list,
# but this is where the invariant actually holds: a frame taller than the pane scrolls, and a
# scrolled pane puts every row of the map one or more lines away from what the user clicked.
# Cutting the bottom loses a button; the letters still work, and nothing lies about where it is.
shrine_fit() {
  local rows=$1 line n=0 text='' map=''
  [ "$SHRINE_ROW" -gt "$rows" ] || return 0
  while IFS= read -r line; do
    n=$((n + 1)); [ "$n" -le "$rows" ] || break
    if [ "$n" -eq 1 ]; then text=$line; else text="$text"$'\n'"$line"; fi
  done <<EOF
$SHRINE_TEXT
EOF
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    [ "${line%%|*}" -le "$rows" ] && map="$map$line"$'\n'
  done <<EOF
$SHRINE_MAP
EOF
  SHRINE_TEXT=$text SHRINE_MAP=$map SHRINE_ROW=$rows
  return 0
}

# ---------------------------------------------------------------- the main screen
# shrine_view_main <rows>: who is here, a line each, and the buttons under them.
shrine_view_main() {
  local rows=$1 data n budget limit shown line w tail_rows banner_rows banner_cols
  local slot id name state cwd pane win mode detail model ctx effort cache tcache cost branch advisor
  local five freset week wreset at tail tele

  prune_records
  load_registry
  data=$(resident_rows)
  n=0; [ -n "$data" ] && n=$(printf '%s\n' "$data" | grep -c '^')
  # What is below the residents: blank, ritual, blank, the buttons, the hint, and the note when
  # there is one. The buttons take two rows in a pane too narrow for one, so their height is
  # measured by the same walk that lays them out rather than assumed here, where it would drift
  # the moment a button is added and leave the whole map one row off.
  tail_rows=$((4 + $(shrine_draw_buttons shrine_buttons --measure)))
  [ -n "$SHRINE_SAID" ] && tail_rows=$((tail_rows + 1))

  shrine_line ''
  if [ "$n" -eq 0 ]; then
    # The torii is all or nothing: half a gate is worse than no gate, and drawing the half that
    # fits would push the buttons off the bottom.
    banner_rows=0 banner_cols=0
    while IFS= read -r line; do
      banner_rows=$((banner_rows + 1))
      shrine_measure "$line"; [ "$SHRINE_W" -gt "$banner_cols" ] && banner_cols=$SHRINE_W
    done < "$SHARE/banner.txt"
    if [ "$banner_rows" -le $((rows - SHRINE_ROW - tail_rows - 2)) ] && [ "$banner_cols" -le "$SHRINE_COLS" ]; then
      while IFS= read -r line; do shrine_line "$line"; done < "$SHARE/banner.txt"
    fi
    shrine_line ''
    shrine_line '  Nobody is here yet.'
  else
    shrine_line "  $SHRINE_NAME"
    shrine_line ''
    budget=$((rows - SHRINE_ROW - tail_rows))
    [ "$budget" -ge 1 ] || budget=1
    # The "… N more" line takes one of those rows itself, so it costs a block; with room for
    # everyone it is not drawn at all and nothing is given up.
    limit=$n
    [ "$n" -gt "$budget" ] && limit=$((budget - 1))
    shown=0
    while IFS='|' read -r slot id name state cwd pane win mode detail model ctx effort cache tcache cost branch advisor five freset week wreset at; do
      [ -n "$slot" ] || continue
      shown=$((shown + 1))
      if [ "$shown" -gt "$limit" ]; then
        shrine_line "  … $((n - limit)) more (gensokyo list)"
        break
      fi
      if [ "$state" = departed ]; then
        tail=departed
      else
        tail=''
        [ -n "$branch" ] && tail="⎇ $branch"
        tele=$(tele_fields "$model" "$ctx" "$effort" "$cache" "$tcache" "$cost" "$branch" "$advisor" "$mode")
        [ -n "$tele" ] && tail="${tail:+$tail · }$tele"
        # Until the first status line report there is nothing to show but the state itself.
        [ -n "$tail" ] || tail=$state
        case $state in
          waiting|question) tail="$tail · needs you${detail:+: $detail}" ;;
        esac
      fi
      shrine_map_add "$((SHRINE_ROW + 1))" 1 "$SHRINE_COLS" focus "$slot"
      shrine_line "$(printf '  %-2s%s %-14s %-16s %s' \
        "$slot" "$(glyph_for "$state")" "${name:0:14}" "$(basename "$cwd")" "$tail")"
    done <<EOF
$data
EOF
  fi

  shrine_line ''
  shrine_line '  ⏲ no rituals yet'
  shrine_line ''
  shrine_draw_buttons shrine_buttons
  if [ "$n" -eq 0 ]; then
    shrine_line '  click a button, or press its letter'
  else
    shrine_line '  click a resident above to open its tab'
  fi
  [ -n "$SHRINE_SAID" ] && shrine_line "  $SHRINE_SAID"
  return 0
}

# ---------------------------------------------------------------- the other screens
# shrine_view_pick <rows> <title> <action> <rows function> <hint>: a numbered list to click, and
# a way back. The rows function prints "argument|label" lines, nearest first. Nine entries at
# most, because each one carries a digit key as well as its line.
shrine_view_pick() {
  local rows=$1 title=$2 action=$3 src=$4 hint=$5 list n budget limit i=0 arg label
  list=$($src)
  n=0; [ -n "$list" ] && n=$(printf '%s\n' "$list" | grep -c '^')
  # Blank, the title and a blank above; a blank, the cancel button and the hint below.
  budget=$((rows - 6))
  [ "$budget" -gt 9 ] && budget=9
  [ "$budget" -ge 1 ] || budget=1
  limit=$n
  [ "$n" -gt "$budget" ] && limit=$((budget - 1))
  shrine_line ''
  shrine_line "  $title"
  shrine_line ''
  [ "$n" -gt 0 ] || shrine_line '  (nobody)'
  while IFS='|' read -r arg label; do
    [ -n "$label" ] || continue
    i=$((i + 1))
    if [ "$i" -gt "$limit" ]; then shrine_line "  … $((n - limit)) more"; break; fi
    shrine_map_add "$((SHRINE_ROW + 1))" 1 "$SHRINE_COLS" "$action" "$arg"
    shrine_line "$(printf '  %s  %s' "$i" "$label")"
  done <<EOF
$list
EOF
  shrine_line ''
  shrine_draw_buttons shrine_cancel_buttons
  shrine_line "  $hint"
  return 0
}

# Where a summon can start: the directories gensokyo has been used in, newest first, and always
# the way to name another one.
shrine_dir_rows() {
  local d i=0
  if [ -f "$STATE_DIR/recent-dirs" ]; then
    while IFS= read -r d; do
      [ -d "$d" ] || continue
      i=$((i + 1)); [ "$i" -le 8 ] || break
      printf '%s|%s\n' "$d" "$(tilde "$d" 60)"
    done < "$STATE_DIR/recent-dirs"
  fi
  printf '|other directory…\n'
  return 0
}

# Who is here, for the banish picker, from the same rows the blocks are drawn from.
shrine_here_rows() {
  local slot id name state cwd rest
  prune_records
  load_registry
  while IFS='|' read -r slot id name state cwd rest; do
    [ -n "$slot" ] || continue
    printf '%s|%s %s %-14s %s\n' "$id" "$slot" "$(glyph_for "$state")" "${name:0:14}" "$(tilde "$cwd" 40)"
  done <<EOF
$(resident_rows)
EOF
  return 0
}

# Who can be recalled: departed panes still open and records of earlier runs, newest first.
shrine_gone_rows() {
  local when id name cwd slot tr now
  now=$(date +%s)
  while IFS='|' read -r when id name cwd slot tr; do
    [ -n "$id" ] || continue
    printf '%s|%-14s %s  %s ago%s\n' "$id" "${name:0:14}" "$(tilde "$cwd" 40)" \
      "$(fmt_age $((now - when)))" "${slot:+ · still in pane $slot}"
  done <<EOF
$(recall_rows)
EOF
  return 0
}

# The one action worth a second click: a banished resident stops mid-thought.
shrine_view_confirm() {
  local name
  name=$(rec_get "$RES_DIR/$SHRINE_ARG" name)
  shrine_line ''
  shrine_line "  banish ${name:-that resident}?"
  shrine_line ''
  shrine_line '  It is asked to /exit, so it stops wherever it has got to. Its tab then'
  shrine_line '  shows the departed screen, and it can be recalled from there or from'
  shrine_line '  the shrine, with its name and its transcript.'
  shrine_line ''
  shrine_draw_buttons shrine_confirm_buttons
  shrine_line '  click, or press y or n'
  return 0
}

shrine_view_help() {
  local action letter text what
  shrine_line ''
  shrine_line "  $SHRINE_NAME"
  shrine_line ''
  while IFS='|' read -r action letter text what; do
    [ -n "$action" ] || continue
    shrine_line "$(printf '  %-3s %s' "$letter" "$what")"
  done <<EOF
$(shrine_buttons)
EOF
  shrine_line ''
  shrine_line '  Every resident has a tab of its own; this one stays when the last of'
  shrine_line '  them leaves. Click a resident here to bring its tab to the front, or'
  shrine_line '  press its slot number.'
  shrine_line ''
  shrine_draw_buttons shrine_cancel_buttons
  shrine_line '  click, or press q'
  return 0
}

# ---------------------------------------------------------------- lines, buttons, the map
# shrine_draw_buttons <table function> [--measure]: one buttons row (rows, when the pane is too
# narrow for one), recording the columns each button covers as it goes. The pieces are ASCII, so
# their width is their length; nothing else is on these rows. --measure prints how many rows they
# would take and draws nothing, which is how the screen above knows what to leave room for.
shrine_draw_buttons() {
  local src=$1 measure='' action letter text what line='  ' col=3 row count=1 pending=''
  [ "${2:-}" = --measure ] && measure=1
  row=$((SHRINE_ROW + 1))
  while IFS='|' read -r action letter text what; do
    [ -n "$action" ] || continue
    if [ -n "$pending" ] && [ $((col + ${#text} - 1)) -gt "$SHRINE_COLS" ]; then
      count=$((count + 1))
      [ -n "$measure" ] || { shrine_line "${line%  }"; row=$((SHRINE_ROW + 1)); }
      line='  '; col=3; pending=''
    fi
    [ -n "$measure" ] || shrine_map_add "$row" "$col" "$((col + ${#text} - 1))" "$action" ''
    line="$line$text  "
    col=$((col + ${#text} + 2))
    pending=1
  done <<EOF
$($src)
EOF
  if [ -n "$measure" ]; then
    printf '%s' "$count"
  elif [ -n "$pending" ]; then
    shrine_line "${line%  }"
  fi
  return 0
}

# The characters gensokyo draws that a terminal gives two columns rather than one: the three on
# the torii's tablet, and the four it takes from the emoji presentation set. Everything else it
# draws - the state glyphs ● ✦ ✧ ○ ·, ⎇, → - is one column wide. Counting one of these as
# narrow costs a wrapped line, which puts every row below it out of step with the map; counting a
# narrow one as wide costs a column at the right edge. An uncertain character belongs on the list.
SHRINE_WIDE='幻 想 郷 ⛩ ⏲ ⚡ ⚖'

# shrine_measure <text>: how many columns it takes on screen, into SHRINE_W. Every line of every
# frame is measured, so this leaves the answer in a variable rather than printing it: a command
# substitution here is a fork per line, and a frame that takes that much longer is a frame that a
# signal is that much more likely to land in the middle of.
shrine_measure() {
  local s=$1 c rest n
  n=${#s}   # not on the `local` line above: its words are expanded before `s` is assigned
  for c in $SHRINE_WIDE; do
    rest=${s//"$c"/}
    n=$((n + ${#s} - ${#rest}))
    s=$rest
  done
  SHRINE_W=$n
  return 0
}
SHRINE_W=0
# The same, for a caller that wants to read it as a value.
shrine_text_width() { shrine_measure "$1"; printf '%s' "$SHRINE_W"; }

# shrine_line <text>: one more row of the frame, cut to the pane's width - cut by columns, not by
# characters, or one wide character in a full line would wrap it.
shrine_line() {
  local s=$1 w
  shrine_measure "$s"; w=$SHRINE_W
  while [ "$w" -gt "$SHRINE_COLS" ]; do
    case " $SHRINE_WIDE " in
      *" ${s:$((${#s} - 1))} "*) w=$((w - 2)) ;;
      *) w=$((w - 1)) ;;
    esac
    s=${s:0:$((${#s} - 1))}
  done
  if [ "$SHRINE_ROW" -eq 0 ]; then SHRINE_TEXT=$s; else SHRINE_TEXT="$SHRINE_TEXT"$'\n'"$s"; fi
  SHRINE_ROW=$((SHRINE_ROW + 1))
  return 0
}

# shrine_map_add <row> <first column> <last column> <action> <argument>
shrine_map_add() { SHRINE_MAP="$SHRINE_MAP$1|$2|$3|$4|$5"$'\n'; }

# shrine_hit <row> <column>: "action|argument" for what was drawn there, or nothing.
shrine_hit() {
  local r c1 c2 action arg
  while IFS='|' read -r r c1 c2 action arg; do
    [ -n "$action" ] || continue
    [ "$r" = "$1" ] && [ "$2" -ge "$c1" ] && [ "$2" -le "$c2" ] && { printf '%s|%s' "$action" "$arg"; return 0; }
  done <<EOF
$SHRINE_MAP
EOF
  return 0
}

# shrine_digit <1-9>: what that number picks on a picker screen - the nth line of the map that
# is a list entry rather than a button, which is the number the line itself carries.
shrine_digit() {
  local r c1 c2 action arg i=0
  while IFS='|' read -r r c1 c2 action arg; do
    case $action in pick-dir|pick-banish|pick-recall) ;; *) continue ;; esac
    i=$((i + 1))
    [ "$i" = "$1" ] && { printf '%s|%s' "$action" "$arg"; return 0; }
  done <<EOF
$SHRINE_MAP
EOF
  return 0
}

# shrine_view_table: the button table the screen that is up is drawn from. A letter is looked up
# there and nowhere else, so `n` in the banish picker cannot open a summon behind it.
shrine_view_table() {
  case $SHRINE_VIEW in
    main)    shrine_buttons ;;
    confirm) shrine_confirm_buttons ;;
    *)       shrine_cancel_buttons ;;
  esac
}

# shrine_letter <character>: the action that letter runs on the screen that is up, or nothing.
shrine_letter() {
  local action letter rest
  while IFS='|' read -r action letter rest; do
    [ -n "$action" ] && [ "$letter" = "$1" ] && { printf '%s' "$action"; return 0; }
  done <<EOF
$(shrine_view_table)
EOF
  return 0
}

# ---------------------------------------------------------------- input
# shrine_event: one thing the user did, into SHRINE_KEY (one character, or `esc`) or into
# SHRINE_CLICK ("row column"); both empty when the tick ran out or the terminal said something
# there is nothing to do about. Mouse reports arrive because the shrine asks the terminal for
# them (ESC[?1000h ESC[?1006h) - not because of tmux, whose own `mouse` option stays off - and
# read ESC[<button;column;rowM for a press, ...m for the release. The release is the one acted
# on: by then the press is already out of the stream, so an action that hands the pane over to
# `read` does not find half a click waiting in it. bash 3.2 refuses a fractional -t, so the bytes
# after an ESC get a whole second; they arrive in one write, so that is a ceiling, not a wait.
shrine_event() {
  local c seq btn col row
  SHRINE_KEY='' SHRINE_CLICK=''
  IFS= read -r -s -n 1 -t "$SHRINE_TICK" c 2>/dev/null || return 0
  if [ "$c" != $'\033' ]; then SHRINE_KEY=$c; return 0; fi
  IFS= read -r -s -n 1 -t 1 c 2>/dev/null || { SHRINE_KEY=esc; return 0; }
  [ "$c" = '[' ] || { SHRINE_KEY=esc; return 0; }
  seq=''
  while :; do
    IFS= read -r -s -n 1 -t 1 c 2>/dev/null || return 0
    case $c in [A-Za-z~]) break ;; esac
    seq="$seq$c"
  done
  # Anything else on this stream - a focus event (ESC[I, ESC[O), an arrow key - is not ours.
  case $seq in '<'*) ;; *) return 0 ;; esac
  [ "$c" = m ] || return 0
  IFS=';' read -r btn col row <<EOF
${seq#<}
EOF
  [ "$btn" = 0 ] || return 0   # the left button; scrolling is 64 and 65, the others 1 and 2
  case $row$col in ''|*[!0-9]*) return 0 ;; esac
  SHRINE_CLICK="$row $col"
  return 0
}

# shrine_ask <heading> <prompt> [readline]: the pane becomes a question, and the answer lands in
# SHRINE_ANSWER with SHRINE_CANCEL empty. Redraws are held off for the duration - a tick landing
# here would paint the frame over the question - and the mouse goes back to the terminal, so a
# click cannot leave its report in the middle of the answer. With `readline` bash reads the whole
# line, which is what completes a directory with Tab, and Ctrl-C then Enter is the way back;
# otherwise the characters are read one at a time, which is what lets Esc take the question back.
shrine_ask() {
  local heading=$1 prompt=$2 mode=${3:-} line='' c
  SHRINE_ANSWER='' SHRINE_CANCEL=''
  SHRINE_BUSY=1
  trap '' USR1 WINCH   # a hook's signal here would cut the read short and read as backing out
  printf '\033[?1006l\033[?1000l\033[?25h\033[H\033[2J\n  %s\n\n  %s' "$heading" "$prompt"
  if [ -n "$mode" ]; then
    IFS= read -e -r line 2>/dev/null || SHRINE_CANCEL=1
  else
    while :; do
      IFS= read -r -s -n 1 c 2>/dev/null || { SHRINE_CANCEL=1; break; }
      case $c in
        '')      break ;;                                    # Enter
        $'\033') SHRINE_CANCEL=1; break ;;                   # Esc; the mouse is off, so it is one
        $'\177'|$'\b') [ -n "$line" ] && { line=${line%?}; printf '\b \b'; } ;;
        [[:print:]]) line="$line$c"; printf '%s' "$c" ;;
      esac
    done
  fi
  printf '\033[?25l\033[?1000h\033[?1006h'
  SHRINE_ANSWER=$line
  SHRINE_BUSY=''
  trap shrine_frame USR1 WINCH
  return 0
}

# ---------------------------------------------------------------- what a click does
# shrine_do <action> [argument]: what a click or a letter asked for. Everything that changes who
# is here runs `gensokyo` as a command of its own rather than calling the function in this
# process: those end in `die` when they are unhappy, and an exit here would take the shrine's
# window with it. What the command said goes under the buttons.
shrine_do() {
  local action=$1 arg=${2:-} f
  SHRINE_SAID=''
  case $action in
    focus)
      f=$(find_resident "$arg") || { SHRINE_SAID="no resident $arg"; return 0; }
      rec_load "$f"
      [ -n "${R_window:-$R_pane}" ] || { SHRINE_SAID="$R_name is still starting"; return 0; }
      focus_window "${R_window:-$R_pane}" ;;
    summon|banish|recall|help) SHRINE_VIEW=$action ;;
    cast)      SHRINE_SAID='casting a spell card is not available yet' ;;
    timetable) SHRINE_SAID='the timetable is not available yet' ;;
    cancel)    SHRINE_VIEW=main; SHRINE_ARG='' ;;
    pick-dir)     shrine_summon "$arg" ;;
    pick-banish)  SHRINE_VIEW=confirm; SHRINE_ARG=$arg ;;
    banish-yes)
      SHRINE_SAID=$(shrine_run close "$SHRINE_ARG")
      SHRINE_VIEW=main; SHRINE_ARG='' ;;
    pick-recall)
      SHRINE_SAID=$(shrine_run resume "$arg" --focus)
      SHRINE_VIEW=main ;;
  esac
  return 0
}

# shrine_run <command> [args]: gensokyo in a process of its own; prints the last thing it said,
# which is the summon's "summoned X (slot 2) in ..." or, when it went wrong, the reason.
shrine_run() {
  local out
  out=$("$SELF" "$@" 2>&1)
  printf '%s' "$(printf '%s\n' "$out" | grep -v '^[[:space:]]*$' | tail -n 1)"
  return 0
}

# The one action that needs typing: which directory, when it is not one of the recent ones, and
# what to call the resident.
shrine_summon() {
  local dir=$1 name
  if [ -z "$dir" ]; then
    shrine_ask 'summon a resident into' \
      "directory (Tab completes, Enter for $(tilde "$PWD"), Ctrl-C then Enter to go back): " readline
    [ -n "$SHRINE_CANCEL" ] && { SHRINE_VIEW=main; return 0; }
    dir=${SHRINE_ANSWER:-$PWD}
  fi
  shrine_ask "summon a resident into $(tilde "$dir")" 'name (Enter for a random one, Esc to go back): '
  [ -n "$SHRINE_CANCEL" ] && { SHRINE_VIEW=main; return 0; }
  name=$SHRINE_ANSWER
  SHRINE_VIEW=main
  set -- "$dir" --focus
  [ -n "$name" ] && set -- "$@" -n "$name"
  SHRINE_SAID=$(shrine_run new "$@")
  return 0
}

# ---------------------------------------------------------------- the pane
# shrine_paint: put the frame on screen. Home the cursor and erase each line as it is written
# rather than clearing the pane first, so a redraw does not flash; then erase whatever a taller
# frame left below.
shrine_paint() {
  local line first=1
  printf '\033[H'
  while IFS= read -r line; do
    [ -n "$first" ] || printf '\n'
    first=''
    printf '%s\033[K' "$line"
  done <<EOF
$SHRINE_TEXT
EOF
  printf '\033[J'
  return 0
}

# shrine_frame: one redraw, from the pane's current size. Also the signal handler, so it can
# arrive in the middle of itself: bash 3.2 runs a trap the moment the signal lands, even while
# `read` is still blocked (which is what makes the signal worth sending at all), so a second
# frame could otherwise overwrite SHRINE_TEXT and SHRINE_MAP halfway through the first one and
# leave a map pointing at rows that are no longer there. The frame in flight is about to show
# the same news, so the signal is dropped rather than queued.
shrine_frame() {
  local size cols rows ttl=$REGISTRY_TTL
  [ -n "$SHRINE_BUSY" ] && return 0
  SHRINE_BUSY=1
  # Ignored rather than merely guarded against: a signal that lands while a frame is reading a
  # record file cuts that `read` short, and a record that reads back empty looks to
  # prune_records like a resident whose pane is gone. Ignoring drops the signal outright, which
  # is what the guard wanted anyway - the frame being drawn is about to show the same news.
  trap '' USR1 WINCH
  size=$(tmux_ display -p -t "${TMUX_PANE:-}" '#{pane_width} #{pane_height}' 2>/dev/null)
  cols=${size%% *}; rows=${size##* }
  case $cols in ''|*[!0-9]*) cols=80 ;; esac
  case $rows in ''|*[!0-9]*) rows=24 ;; esac
  # Nobody attached, nobody looking: draw from the cached registry instead of asking
  # `claude agents --json` for a fresh one every few seconds for as long as the cockpit runs.
  # The clock skips its render entirely for the same reason (lib/server.sh); the shrine still
  # draws, because it costs nothing and the pane is then correct the moment a client attaches.
  [ -n "$(tmux_ list-clients -F 1 2>/dev/null)" ] || ttl=86400
  local REGISTRY_TTL=$ttl   # dynamic scope: it reaches load_registry, and only for this frame
  shrine_render "$cols" "$rows"
  shrine_paint
  SHRINE_BUSY=''
  trap shrine_frame USR1 WINCH
  return 0
}

# Ctrl-C is the reflex when a question is up, and its default disposition would end this loop -
# which is the pane's command, so tmux would take the window, and that window is the one tab that
# must never go. It takes the user back to the shrine instead.
shrine_interrupt() {
  SHRINE_CANCEL=1
  SHRINE_VIEW=main
  SHRINE_ARG='' SHRINE_SAID=''
  return 0
}

# The shrine has a window of its own - the first tab, and the one still there when the last
# resident leaves. Residents never take it over.
cmd__shrine() {
  local hit
  [ -n "${TMUX_PANE:-}" ] && tmux_ set -p -t "$TMUX_PANE" @shrine 1 2>/dev/null
  printf '\033]2;gensokyo\007\033[2J\033[?25l\033[?1000h\033[?1006h'   # own pane title, no cursor, clicks
  trap 'printf "\033[?1006l\033[?1000l\033[?25h"' EXIT
  trap shrine_interrupt INT
  trap shrine_frame USR1 WINCH
  # Without a terminal there is nothing to read and every read returns at once; wait to be
  # closed rather than spinning. The same guard as the departed screen's, for the same reason.
  [ -t 0 ] || while :; do sleep 3600; done
  while :; do
    # Nothing can be in flight at the top of the loop, and a frame or a question that died
    # holding the guard would leave the pane frozen until it was respawned.
    SHRINE_BUSY=''
    shrine_frame
    # The tick is the floor, not the rate: a hook signals the loop and the trap above draws at
    # once.
    shrine_event
    if [ -n "$SHRINE_CLICK" ]; then
      hit=$(shrine_hit "${SHRINE_CLICK%% *}" "${SHRINE_CLICK##* }")
      [ -n "$hit" ] && shrine_do "${hit%%|*}" "${hit#*|}"
    elif [ -n "$SHRINE_KEY" ]; then
      shrine_press "$SHRINE_KEY"
    fi
  done
}

# shrine_press <character>: Esc and q go back, a digit picks the numbered line - in the main
# screen a digit is a slot, which is the number the block itself carries - and a letter runs the
# button that carries it on the screen that is up.
shrine_press() {
  local action hit
  case $1 in
    esc) [ "$SHRINE_VIEW" = main ] || shrine_do cancel; return 0 ;;
    [1-9])
      if [ "$SHRINE_VIEW" = main ]; then
        shrine_do focus "$1"
      else
        hit=$(shrine_digit "$1")
        [ -n "$hit" ] && shrine_do "${hit%%|*}" "${hit#*|}"
      fi
      return 0 ;;
  esac
  action=$(shrine_letter "$1")
  [ -n "$action" ] && shrine_do "$action"
  return 0
}

# shrine_signal: put a hook's news on the shrine now instead of at its next tick. The loop
# marks its own pane with @shrine, and a pane's #{pane_pid} is the process running in it.
shrine_signal() {
  local pid mark
  while read -r pid mark; do
    [ "$mark" = 1 ] && [ -n "$pid" ] && kill -USR1 "$pid" 2>/dev/null
  done <<EOF
$(tmux_ list-panes -s -t "=$SESSION" -F '#{pane_pid} #{@shrine}' 2>/dev/null)
EOF
  return 0
}
