# lib/spellcards.sh - spell cards: the prompts the shrine's `cast` button types into residents,
# and `gensokyo broadcast` behind it. A card is a markdown file with a little frontmatter; its
# body is the prompt, with placeholders filled in per resident. Sourced by bin/gensokyo;
# bash 3.2.
# shellcheck shell=bash

# Cards ship in share/spellcards/ and the user's own live in ~/.config/gensokyo/spellcards/,
# where a file of the same name shadows the shipped one. Writing a card is writing a file:
# nothing registers it and no resident is taught anything about it.
CARD_title='' CARD_summary='' CARD_peer='' CARD_body='' CARD_slug=''

# A card is addressed by its slug (its filename without .md) from the CLI and from a tmux menu,
# where it travels through a run-shell command string; anything outside these characters would
# have to survive two layers of quoting to get there, so such a file is left out and named by
# `gensokyo broadcast` instead of being cast with a mangled name.
card_name_ok() { case $1 in [A-Za-z0-9]*) case $1 in *[!A-Za-z0-9._-]*) return 1 ;; esac ;; *) return 1 ;; esac; return 0; }

# card_files: every usable card file, the user's before the shipped ones so the first hit for a
# given filename wins.
card_files() {
  local d f base seen=$'\n'
  for d in "$CONFIG_DIR/spellcards" "$SHARE/spellcards"; do
    [ -d "$d" ] || continue
    for f in "$d"/*.md; do
      [ -f "$f" ] || continue
      base=${f##*/}
      card_name_ok "${base%.md}" || continue
      case $seen in *$'\n'"$base"$'\n'*) continue ;; esac
      seen="$seen$base"$'\n'
      printf '%s\n' "$f"
    done
  done
  return 0
}

# card_unusable: card files left out by the rule above, so nothing is skipped in silence.
card_unusable() {
  local d f base
  for d in "$CONFIG_DIR/spellcards" "$SHARE/spellcards"; do
    [ -d "$d" ] || continue
    for f in "$d"/*.md; do
      [ -f "$f" ] || continue
      base=${f##*/}
      card_name_ok "${base%.md}" || printf '%s\n' "$f"
    done
  done
  return 0
}

# card_load <file>: the card into CARD_slug, CARD_title, CARD_summary, CARD_peer and CARD_body.
# The frontmatter is the `---` fenced block at the top, `key: value` a line, and it is read by
# hand rather than evalled: a card is a text file the user writes, and nothing in it should be
# able to run. A file with no fence is all body, so a bare prompt in a file is a valid card.
card_load() {
  local f=$1 line key val state=start
  CARD_slug='' CARD_title='' CARD_summary='' CARD_peer='' CARD_body=''
  [ -f "$f" ] || return 1
  CARD_slug=${f##*/}; CARD_slug=${CARD_slug%.md}
  while IFS= read -r line || [ -n "$line" ]; do
    case $state in
      start)
        case $line in
          '---') state=front; continue ;;
          *) state=body ;;
        esac ;;
      front)
        case $line in '---') state=body; continue ;; esac
        key=${line%%:*}
        if [ "$key" != "$line" ]; then
          val=${line#*:}; val=${val# }
          case $key in
            title)   CARD_title=$val ;;
            summary) CARD_summary=$val ;;
            peer)    CARD_peer=$val ;;
          esac
        fi
        continue ;;
    esac
    # Leading blank lines belong to the fence, not to the prompt.
    [ -n "$CARD_body" ] || [ -n "$line" ] || continue
    if [ -z "$CARD_body" ]; then CARD_body=$line; else CARD_body="$CARD_body"$'\n'"$line"; fi
  done < "$f"
  [ -n "$CARD_title" ] || CARD_title=$CARD_slug
  return 0
}

# card_rows: "slug|title|peer|summary|path" per card, by title, which is the order the picker
# and the listing both show.
card_rows() {
  local f
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    card_load "$f" || continue
    printf '%s|%s|%s|%s|%s\n' "$CARD_slug" "$CARD_title" "$CARD_peer" "$CARD_summary" "$f"
  done <<EOF
$(card_files)
EOF
  return 0
}
# card_rows, sorted; a separate function so the walk above stays readable.
card_rows_sorted() { card_rows | sort -t '|' -k2,2; }

# find_card <slug|title|part of either>: the card's path, or nothing. An exact slug or title
# wins; otherwise a substring, and only when it picks out exactly one card.
find_card() {
  local want slug title peer summary path part='' n=0
  want=$(lower "$1")
  while IFS='|' read -r slug title peer summary path; do
    [ -n "$slug" ] || continue
    if [ "$(lower "$slug")" = "$want" ] || [ "$(lower "$title")" = "$want" ]; then
      printf '%s\n' "$path"; return 0
    fi
    case $(lower "$slug $title") in *"$want"*) n=$((n + 1)); part=$path ;; esac
  done <<EOF
$(card_rows_sorted)
EOF
  [ "$n" -eq 1 ] || return 1
  printf '%s\n' "$part"
}

# card_expand <text> <self> <peer> <cwd> <residents>: the placeholders every card may use. Done
# once per target, because {self} and {residents} are different for each of them.
card_expand() {
  local s=$1
  s=${s//\{self\}/$2}
  s=${s//\{peer\}/$3}
  s=${s//\{cwd\}/$4}
  s=${s//\{residents\}/$5}
  printf '%s' "$s"
}

# ---------------------------------------------------------------- who a card goes to
# cast_blocked <session-id>: why gensokyo will not type into this resident, or nothing.
#
# The one that matters is a resident with a dialog open. Typing there does not submit a prompt:
# the card goes into the dialog and the Enter after it *answers* the dialog - approving or
# refusing whatever permission was pending - and the card is gone. That is an action the user
# never asked for, so it is refused rather than risked. The registry reports `waiting` for as
# long as a permission or plan dialog is up; the hooks record `question` for an AskUserQuestion
# one, which the registry cannot see. A resident whose ✦ is only a finished turn nobody has
# read yet is not blocked - typing into that is exactly the right thing.
#
# The third case is a resident with no registry row at all, and it is the one with teeth. A
# session summoned into a directory Claude Code has not seen before comes up on the workspace
# trust dialog, which no detector here can read: no hook fires and the registry has yet to hear
# of the session. Measured 2026-09-07: the Enter after a swallowed paste answers that dialog,
# and depending on the folder it either exits the resident or *grants the directory trust* -
# gensokyo deciding a security question on the user's behalf. No row also means SendMessage
# cannot reach it, which is why `--with` already refuses the same state. So: no row, no typing.
cast_blocked() {
  local row
  status_load "$1"
  [ "$S_pending" = question ] && { printf 'is asking you a question'; return 0; }
  row=$(registry_row "$1")
  [ -z "$row" ] && { printf 'is still starting up'; return 0; }
  [ "${row%%|*}" = waiting ] && { printf 'has a dialog waiting for you'; return 0; }
  return 0
}

# cast_targets <all|awaiting|idle|name|slot|session-id>: the session ids it names, one per line.
# A departed resident has no prompt to type into, so it is never part of a group. Everything
# else a group must leave out - a dialog held open, a session not yet in the registry - is
# `cast_blocked`'s to say, and is left to it deliberately: this filter used to drop `starting`
# on its own, which meant the group path was safe from the trust dialog for a reason nothing in
# the code stated, and a resident left out in silence. Named on its own, each is reported by
# the caster instead of being dropped in silence.
cast_targets() {
  local spec=$1 slot id name state rest f
  case $spec in
    all|awaiting|idle)
      while IFS='|' read -r slot id name state rest; do
        [ -n "$id" ] || continue
        case $state in departed) continue ;; esac
        [ -n "$(cast_blocked "$id")" ] && continue
        case $spec in
          awaiting) [ "$state" = waiting ] || continue ;;
          idle)     [ "$state" = idle ] || continue ;;
        esac
        printf '%s\n' "$id"
      done <<EOF
$(resident_rows)
EOF
      ;;
    *)
      f=$(find_resident "$spec") || return 1
      printf '%s\n' "${f##*/}"
      ;;
  esac
  return 0
}

# other_names <session-id>: the live residents that are not this one, as {residents} reads it.
# `nobody` rather than an empty line, so a card can say what to do when it is alone.
other_names() {
  local slot id name state rest out=''
  while IFS='|' read -r slot id name state rest; do
    [ -n "$id" ] || continue
    [ "$id" = "$1" ] && continue
    case $state in departed|starting) continue ;; esac
    out="${out:+$out, }$name"
  done <<EOF
$(resident_rows)
EOF
  printf '%s' "${out:-nobody}"
}

# ---------------------------------------------------------------- typing it in
# cast_tail <text>: the last line of a card, short enough to stay on one row of an input line.
# The last line and not the first: a card's echo runs to twice the card's height, so by the time
# the end of a ten-line card is on screen its opening line has already scrolled off.
cast_tail() {
  local last
  last=$(printf '%s\n' "$1" | sed '/^[[:space:]]*$/d' | sed -n '$p')
  printf '%s' "$(printf '%s' "$last" | cut -c1-20)"
}

# cast_type <pane> <text>: put the prompt into a resident's input line and submit it.
# 0 it went in and was submitted, 1 tmux refused, 2 it never reached the input line.
#
# `load-buffer` then `paste-buffer -p` is what holds a multi-line card together: bracketed
# paste arrives in Claude Code as one block that submits only when Enter comes, so a card with
# newlines in it cannot go in as several half prompts the way `send-keys -l` would send it.
#
# Enter waits for the card to appear rather than for a number of seconds - a fixed pause is a
# bug, the same lesson as `pane_settled` - and it waits for the *card*, not merely for the pane
# to differ. Waiting only on "the pane changed" is what let a swallowed paste read as a success:
# anything that eats the paste redraws the pane doing it, so the wait was satisfied, the Enter
# went into a dialog and `broadcast` reported a card it had lost (measured 2026-09-07 against
# the workspace trust dialog, where that Enter granted the directory trust). If the card never
# shows, no Enter is sent: an unread card left in an input line is visible and recoverable, and
# answering an unknown dialog is neither.
#
# Either needle will do, because what the pane shows depends on who is reading it. Claude Code
# collapses a bracketed multi-line paste to "[Pasted text #4 +3 lines]", so none of the card's
# own words are on screen; a one-line card arrives literally; and anything that never asked for
# bracketed paste - the test stub, a plain shell - is sent the text as keystrokes and echoes it.
# The pane must also have changed, so that a card cast twice cannot be answered by the echo of
# its own last cast sitting on the screen. What that pair cannot rule out is a pane that redrew
# for an unrelated reason while an old echo happened to be visible; the states that would do it
# are the ones `cast_blocked` refuses outright.
# A resident that took the paste shows it within a tenth of a second and this loop leaves at
# once, so the only cast that waits the whole time is one that is about to be reported as
# undelivered. That is what the budget is for: long enough that a busy machine redrawing slowly
# is not called a failure, short enough that being told is not worth the wait.
CAST_WAIT=50   # tenths of a second to wait for the card to reach the input line
cast_type() {
  local pane=$1 text=$2 tail before after n=0
  tail=$(cast_tail "$text")
  [ -n "$tail" ] || return 1
  before=$(tmux_ capture-pane -p -t "$pane" 2>/dev/null)
  printf '%s' "$text" | tmux_ load-buffer -b gensokyo-cast - 2>/dev/null || return 1
  tmux_ paste-buffer -b gensokyo-cast -d -p -t "$pane" 2>/dev/null || return 1
  while [ "$n" -lt "$CAST_WAIT" ]; do
    after=$(tmux_ capture-pane -p -t "$pane" 2>/dev/null)
    if [ "$after" != "$before" ]; then
      case $after in
        *'Pasted text'*|*"$tail"*)
          tmux_ send-keys -t "$pane" Enter 2>/dev/null || return 1
          return 0 ;;
      esac
    fi
    nap 0.1; n=$((n + 1))
  done
  return 2
}

# ---------------------------------------------------------------- the command
# broadcast (spell) <card> <all|awaiting|idle|name...> [--with peer]: type one card into every
# resident named. Alone it lists the cards. There is no way to broadcast free text: telling one
# resident something is typing into its pane, and a prompt worth sending to everybody is worth
# a file in ~/.config/gensokyo/spellcards/.
cmd_broadcast() {
  local card='' with='' specs=() spec more state ids='' ids2='' id f peer='' peername='' one='' why n=0 sent=0 text rc=0
  while [ $# -gt 0 ]; do
    case $1 in
      --with) with=${2:-}; shift ;;
      -*) die "broadcast: unknown option $1 (usage: gensokyo broadcast <card> <all|awaiting|idle|name> [--with peer])" ;;
      *) if [ -z "$card" ]; then card=$1; else specs[${#specs[@]}]=$1; fi ;;
    esac
    shift
  done
  ensure_dirs
  [ -n "$card" ] || { card_list; return 0; }
  f=$(find_card "$card") || die "broadcast: no spell card '$card' (gensokyo broadcast lists them)"
  card_load "$f"
  [ -n "$CARD_body" ] || die "broadcast: $CARD_title has no prompt in it, only frontmatter ($f)"
  [ "${#specs[@]}" -gt 0 ] || die "broadcast: name who gets it: all, awaiting, idle, or a resident (gensokyo list)"
  load_registry
  for spec in "${specs[@]}"; do
    more=$(cast_targets "$spec") || die "broadcast: no resident '$spec' (gensokyo list)"
    ids="$ids$more"$'\n'
  done
  # One resident named twice - by name and in `all` - is cast at once.
  ids=$(printf '%s\n' "$ids" | sed '/^$/d' | awk '!seen[$0]++')
  # A group leaves out anybody holding a dialog, and a resident quietly missed is worse than
  # one that was never asked for: say who was left out and why, before anything is typed.
  case " ${specs[*]} " in
    *' all '*|*' awaiting '*|*' idle '*)
      while IFS='|' read -r spec id more state ids2; do
        [ -n "$id" ] || continue
        case $state in departed) continue ;; esac
        why=$(cast_blocked "$id")
        [ -n "$why" ] && warn "broadcast: $more $why; left out"
      done <<EOF
$(resident_rows)
EOF
      ;;
  esac
  [ -n "$ids" ] || die "broadcast: nobody to cast $CARD_title at (gensokyo list)"
  n=$(printf '%s\n' "$ids" | grep -c '^')

  if [ "$CARD_peer" = required ]; then
    [ "$n" -eq 1 ] || die "broadcast: $CARD_title is cast at one resident, with a peer to talk to (--with); $n were named"
    [ -n "$with" ] || die "broadcast: $CARD_title needs a peer: --with <name>"
  fi
  if [ -n "$with" ]; then
    case $CARD_body in *'{peer}'*) ;; *) die "broadcast: $CARD_title names no peer, so --with has nowhere to go" ;; esac
    peer=$(find_resident "$with") || die "broadcast: no resident '$with' to be the peer (gensokyo list)"
    peer=${peer##*/}
    rec_load "$RES_DIR/$peer"
    peername=$R_name
    [ -n "$R_departed" ] && die "broadcast: $peername has departed and cannot be the peer (gensokyo resume $peername)"
    # A resident that is not in the registry yet cannot be reached by name: SendMessage would
    # fail on it exactly as it fails on a name that never existed, and the card would send the
    # target after a dead end.
    [ -n "$(registry_row "$peer")" ] || die "broadcast: $peername is still starting, so nothing can message it yet"
    case $ids in "$peer"|"$peer"$'\n'*|*$'\n'"$peer"$'\n'*|*$'\n'"$peer") die "broadcast: $peername cannot be its own peer" ;; esac
    # Said rather than refused: the message will reach it, but it will not be read until the
    # user has dealt with what it is holding, and the target should not be left wondering.
    why=$(cast_blocked "$peer")
    [ -n "$why" ] && warn "broadcast: $peername $why, so it may not answer until you have seen to that"
  fi

  while IFS= read -r id; do
    [ -n "$id" ] || continue
    rec_load "$RES_DIR/$id"
    if [ -n "$R_departed" ]; then warn "broadcast: $R_name has departed; not cast at"; continue; fi
    if [ -z "$R_pane" ]; then warn "broadcast: $R_name has no pane yet; not cast at"; continue; fi
    # The last gate before anything is typed, and the one that has to be here rather than only
    # in cast_targets: a resident named by hand comes straight past the groups.
    why=$(cast_blocked "$id")
    if [ -n "$why" ]; then warn "broadcast: $R_name $why; not cast at, or the card would answer it"; continue; fi
    text=$(card_expand "$CARD_body" "$R_name" "$peername" "$R_cwd" "$(other_names "$id")")
    cast_type "$R_pane" "$text"; rc=$?
    case $rc in
      0) # We typed for the user, so whatever this resident was waiting to be told, it has been.
         status_write "$id" '' ''
         sent=$((sent + 1)); one=$R_name ;;
      2) warn "broadcast: the card never reached $R_name's input line; nothing was submitted - look at its pane" ;;
      *) warn "broadcast: tmux could not type into $R_name's pane" ;;
    esac
  done <<EOF
$ids
EOF
  [ "$sent" -gt 0 ] || die "broadcast: $CARD_title reached nobody"
  refresh_bar
  if [ -n "$peername" ]; then
    say "cast $CARD_title on $one, with $peername as peer"
  elif [ "$sent" -eq 1 ]; then
    say "cast $CARD_title on $one"
  else
    say "cast $CARD_title on $sent residents"
  fi
  return 0
}

card_list() {
  local slug title peer summary path n=0 f
  while IFS='|' read -r slug title peer summary path; do
    [ -n "$slug" ] || continue
    [ "$n" -eq 0 ] && printf '  %-18s %-34s %s\n' card title 'what it asks for'
    n=$((n + 1))
    printf '  %-18s %-34s %s%s\n' "$slug" "$title" "$summary" \
      "$([ "$peer" = required ] && printf ' (needs --with <peer>)')"
  done <<EOF
$(card_rows_sorted)
EOF
  if [ "$n" -eq 0 ]; then
    say "no spell cards: put one in $(tilde "$CONFIG_DIR/spellcards")"
  else
    say
    say "  gensokyo broadcast <card> all|awaiting|idle|<name> [--with <name>]   (the shrine's [ cast ] button)"
    say "  your own cards go in $(tilde "$CONFIG_DIR/spellcards")/<name>.md; {self} {peer} {cwd} {residents} are filled in"
  fi
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    warn "not a usable card name (letters, digits, . _ - and .md): $(tilde "$f")"
  done <<EOF
$(card_unusable)
EOF
  return 0
}

# ---------------------------------------------------------------- the --tty menus
# `g s` under the plain client: card, then who gets it, then the peer when the card wants one.
# Each step is a menu of its own rather than one long list, which is what the shrine's pickers
# do as well.
cmd__menu-spell() {
  local client=$1 s i=0 args=() slug title peer summary path label
  s=$(sq "$SELF")
  while IFS='|' read -r slug title peer summary path; do
    [ -n "$slug" ] || continue
    i=$((i + 1)); [ "$i" -le 9 ] || break
    label=$title; [ "$peer" = required ] && label="$title  (pair)"
    args=(${args[@]+"${args[@]}"} "$label" "$i" "run-shell '$s _menu-cast-target $(sq "$client") $(sq "$slug")'")
  done <<EOF
$(card_rows_sorted)
EOF
  [ "$i" -gt 0 ] || { tmux_ display-message -c "$client" "no spell cards"; return 0; }
  args=(${args[@]+"${args[@]}"} "" "cancel" q "")
  tmux_ display-menu -c "$client" -T " cast a spell card " -x C -y C "${args[@]}"
}

cmd__menu-cast-target() {
  local client=$1 slug=$2 s i=0 args=() f slot id name state cwd rest label next
  s=$(sq "$SELF")
  f=$(find_card "$slug") || { tmux_ display-message -c "$client" "no spell card $slug"; return 0; }
  card_load "$f"
  load_registry
  # Chained with `run-shell` and not `run-shell -b`: the next step is another display-menu,
  # which blocks the process it is drawn from until it closes, and run-shell tolerates that
  # (the same reason `_menu-banish` is bound that way). The client is passed by name rather
  # than as `#{client_name}`, so what the menu runs is settled here and not by tmux later.
  if [ "$CARD_peer" = required ]; then
    next="run-shell '$s _menu-cast-peer $(sq "$client") $(sq "$slug")"
  else
    next=''
    args=(${args[@]+"${args[@]}"} \
      "everyone" a "run-shell -b '$s broadcast $(sq "$slug") all >/dev/null 2>&1'" \
      "everyone who needs you" w "run-shell -b '$s broadcast $(sq "$slug") awaiting >/dev/null 2>&1'" \
      "everyone who is resting" i "run-shell -b '$s broadcast $(sq "$slug") idle >/dev/null 2>&1'" "")
  fi
  while IFS='|' read -r slot id name state cwd rest; do
    [ -n "$id" ] || continue
    case $state in departed|starting) continue ;; esac
    i=$((i + 1)); [ "$i" -le 9 ] || break
    label="$slot $(glyph_for "$state") $name · $(tilde "$cwd" 30)"
    if [ -n "$next" ]; then
      args=(${args[@]+"${args[@]}"} "$label" "$i" "$next $(sq "$name")'")
    else
      args=(${args[@]+"${args[@]}"} "$label" "$i" "run-shell -b '$s broadcast $(sq "$slug") $(sq "$name") >/dev/null 2>&1'")
    fi
  done <<EOF
$(resident_rows)
EOF
  [ "$i" -gt 0 ] || { tmux_ display-message -c "$client" "nobody is here to cast $CARD_title at"; return 0; }
  args=(${args[@]+"${args[@]}"} "" "cancel" q "")
  tmux_ display-menu -c "$client" -T " $CARD_title on " -x C -y C "${args[@]}"
}

cmd__menu-cast-peer() {
  local client=$1 slug=$2 target=$3 s i=0 args=() f slot id name state cwd rest label
  s=$(sq "$SELF")
  f=$(find_card "$slug") || { tmux_ display-message -c "$client" "no spell card $slug"; return 0; }
  card_load "$f"
  load_registry
  while IFS='|' read -r slot id name state cwd rest; do
    [ -n "$id" ] || continue
    case $state in departed|starting) continue ;; esac
    [ "$(lower "$name")" = "$(lower "$target")" ] && continue
    i=$((i + 1)); [ "$i" -le 9 ] || break
    label="$slot $(glyph_for "$state") $name · $(tilde "$cwd" 30)"
    args=(${args[@]+"${args[@]}"} "$label" "$i" \
      "run-shell -b '$s broadcast $(sq "$slug") $(sq "$target") --with $(sq "$name") >/dev/null 2>&1'")
  done <<EOF
$(resident_rows)
EOF
  [ "$i" -gt 0 ] || { tmux_ display-message -c "$client" "nobody else is here to review for $target"; return 0; }
  args=(${args[@]+"${args[@]}"} "" "cancel" q "")
  tmux_ display-menu -c "$client" -T " $target asks whom? " -x C -y C "${args[@]}"
}
