# lib/recall.sh - bringing departed residents back: `resume` (recall), the list of who can be
# recalled and the `g r` menu. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

DEPARTED_DIR=$STATE_DIR/departed   # records of earlier runs, moved here by archive_records

# transcript_of <session-id>: the transcript `claude --resume` reads, kept by Claude Code as
# $CLAUDE_DIR/projects/<directory slug>/<session-id>.jsonl. Empty when there is none.
transcript_of() {
  find "$CLAUDE_DIR/projects" -maxdepth 2 -name "$1.jsonl" 2>/dev/null | head -n 1
}

# recall_rows: who can be recalled, newest first, as "when|id|name|cwd|slot|transcript" lines:
# residents still showing the departed screen in a pane (slot set) and records of earlier
# runs (slot empty). `when` is the transcript's mtime, else the moment the resident departed.
recall_rows() {
  local f id when tr
  for f in "$RES_DIR"/* "$DEPARTED_DIR"/*; do
    [ -f "$f" ] || continue
    rec_load "$f"; id=${f##*/}
    case $f in "$RES_DIR"/*) [ -n "$R_departed" ] || continue ;; *) R_slot= ;; esac
    tr=$(transcript_of "$id")
    if [ -n "$tr" ]; then when=$(mtime_of "$tr"); else when=${R_departed:-${R_launched:-0}}; fi
    printf '%s|%s|%s|%s|%s|%s\n' "$when" "$id" "$R_name" "$R_cwd" "$R_slot" "$tr"
  done | sort -t '|' -k1,1nr
}

# find_departed <name|session-id prefix>: the newest archived record with that name (any case),
# else the newest whose id starts with the argument (4+ characters); prints its path.
find_departed() {
  local want by_name='' by_id='' when id name rest
  want=$(lower "$1")
  while IFS='|' read -r when id name rest; do
    [ -n "$id" ] && [ -f "$DEPARTED_DIR/$id" ] || continue
    [ -z "$by_name" ] && [ "$(lower "$name")" = "$want" ] && by_name=$id
    if [ -z "$by_id" ] && [ "${#want}" -ge 4 ]; then case $id in "$want"*) by_id=$id ;; esac; fi
  done <<EOF
$(recall_rows)
EOF
  id=${by_name:-$by_id}
  [ -n "$id" ] && printf '%s\n' "$DEPARTED_DIR/$id"
}

# resume [name|slot|session-id] [--focus] [--json]: alone, who can be recalled; with a name, a
# departed resident still in its pane is recalled there (the screen's own `r` key), one from an
# earlier run gets a fresh slot and pane. Claude Code resumes the same session id and keeps
# the name and transcript; the launch flags chosen at summon time apply again, the first
# prompt is not replayed.
cmd_resume() {
  local who='' focus='' json='' f id rec slot stage pane
  while [ $# -gt 0 ]; do
    case $1 in
      --focus) focus=1 ;;
      --json) json=1 ;;
      -*) die "resume: unknown option $1 (usage: gensokyo resume [name|slot|session-id] [--json])" ;;
      *) [ -z "$who" ] || die "resume: one resident at a time"; who=$1 ;;
    esac
    shift
  done
  ensure_dirs
  if [ -z "$who" ]; then recall_list "$json"; return 0; fi
  if f=$(find_resident "$who"); then
    rec_load "$f"
    [ -n "$R_departed" ] || die "resume: $R_name is still here (slot $R_slot)"
    tmux_ send-keys -t "$R_pane" r
    [ -n "$focus" ] && focus_pane "$R_pane"
    say "recalled $R_name into its pane (slot $R_slot)"
    return 0
  fi
  f=$(find_departed "$who") || die "resume: nobody called '$who' has departed (gensokyo resume lists them)"
  rec_load "$f"; id=${f##*/}
  [ -d "$R_cwd" ] || die "resume: $R_name's directory is gone: $R_cwd"
  find_resident "$R_name" >/dev/null && die "resume: another $R_name is here already (gensokyo list); close it first"
  [ -n "$(transcript_of "$id")" ] || warn "no transcript for $R_name under $CLAUDE_DIR/projects; Claude Code may not find the session"
  start_server
  slot=$(next_slot); stage=$(stage_of_slot "$slot"); rec=$RES_DIR/$id
  mv "$f" "$rec"
  rec_set "$rec" slot "$slot"; rec_set "$rec" window "$stage"; rec_set "$rec" resume 1
  rec_del "$rec" pane; rec_del "$rec" departed; rec_del "$rec" exit
  pane=$(open_pane "$id" "$stage") || { mv "$rec" "$f"; die "resume: tmux could not open a pane"; }
  rec_set "$rec" pane "$pane"
  remember_dir "$R_cwd"
  [ -n "$focus" ] && focus_pane "$pane"
  say "recalled $R_name (slot $slot) in $R_cwd"
}

recall_list() {
  local json=$1 rows now when id name cwd slot tr note n=0
  rows=$(recall_rows)
  if [ -n "$json" ]; then
    # shellcheck disable=SC2016
    printf '%s\n' "$rows" | jq_ -R -s '
      split("\n") | map(select(length > 0) | split("|"))
      | map({session_id: .[1], name: .[2], cwd: .[3], slot: (if .[4] == "" then null else (.[4] | tonumber) end),
             in_pane: (.[4] != ""), departed_at: (.[0] | tonumber), transcript: (.[5] != "")})'
    return 0
  fi
  now=$(date +%s)
  while IFS='|' read -r when id name cwd slot tr; do
    [ -n "$id" ] || continue
    [ "$n" -eq 0 ] && printf '  %-16s %-36s %-9s %s\n' name directory session departed
    n=$((n + 1)); note=''
    [ -n "$slot" ] && note=" · still in its pane (slot $slot)"
    [ -z "$tr" ] && note="$note · no transcript"
    printf '  %-16s %-36s %-9s %s ago%s\n' "${name:0:16}" "$(tilde "$cwd" 36)" "${id:0:8}" "$(fmt_age $((now - when)))" "$note"
  done <<EOF
$rows
EOF
  if [ "$n" -eq 0 ]; then
    say "nobody has departed; residents that leave (gensokyo close, /exit) can be recalled from here"
  else
    say
    say "  gensokyo resume <name|session id>   ($(prefix_label) then g r in the cockpit)"
  fi
}

# `g r`: a menu of the nine most recent departed residents.
cmd__menu-recall() {
  local client=$1 s i=0 args=() when id name cwd slot tr now label
  s=$(sq "$SELF"); now=$(date +%s)
  while IFS='|' read -r when id name cwd slot tr; do
    [ -n "$id" ] || continue
    i=$((i + 1)); [ "$i" -le 9 ] || break
    label="$name · $(tilde "$cwd" 40) · $(fmt_age $((now - when))) ago${slot:+ · still in pane $slot}"
    args=(${args[@]+"${args[@]}"} "$label" "$i" "run-shell -b '$s resume $id --focus >/dev/null 2>&1'")
  done <<EOF
$(recall_rows)
EOF
  [ "$i" -gt 0 ] || { tmux_ display-message -c "$client" "nobody has departed"; return 0; }
  args=(${args[@]+"${args[@]}"} "" "cancel" q "")
  tmux_ display-menu -c "$client" -T " recall a departed resident " -x C -y C "${args[@]}"
}
