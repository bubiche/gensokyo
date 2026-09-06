# lib/records.sh - resident records: state/residents/<session-id> files and the helpers that
# read, write, find, archive and prune them. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# ---------------------------------------------------------------- resident records
# state/residents/<session-id> holds KEY=value lines: slot, name, cwd, pane (%N), window,
# launched (epoch), args (shell-quoted launch flags), prompt, mode (the --permission-mode
# passed at launch, if any), departed (epoch), exit, resume.
rec_get() { sed -n "s/^$2=//p" "$1" 2>/dev/null | head -n 1; }
rec_set() {
  local tmp=$1.tmp.$$
  { [ -f "$1" ] && grep -v "^$2=" "$1"; printf '%s=%s\n' "$2" "$3"; } > "$tmp" && mv "$tmp" "$1"
}
rec_del() { [ -f "$1" ] && { grep -v "^$2=" "$1" > "$1.tmp.$$"; mv "$1.tmp.$$" "$1"; }; return 0; }

# load_kv <file> <PREFIX> <key>...: read KEY=value lines into PREFIX_<key> for the listed keys
# without spawning a process (the bar and border do this per resident on every redraw). The
# caller pre-sets the variables to '' (so a missing file or key leaves them empty and
# the linter sees the assignment). Values are expanded from $line after eval parses, so a
# prompt containing quotes or $ is safe.
load_kv() {
  local file=$1 prefix=$2 line key
  shift 2
  [ -f "$file" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    key=${line%%=*}
    case " $* " in *" $key "*) eval "${prefix}_$key=\${line#*=}" ;; esac
  done < "$file"
}

# rec_load <file>: every record key into R_<key>.
rec_load() {
  # shellcheck disable=SC2034  # every key is loaded; not every caller reads all of them
  R_slot='' R_name='' R_cwd='' R_pane='' R_window='' R_launched='' R_args='' R_prompt='' R_mode='' R_departed='' R_exit='' R_resume=''
  load_kv "$1" R slot name cwd pane window launched args prompt mode departed exit resume
}

ensure_dirs() { mkdir -p "$RES_DIR" "$STATE_DIR/status" "$STATE_DIR/statusline" "$CONFIG_DIR"; }
count_records() { find "$RES_DIR" -type f 2>/dev/null | wc -l | tr -d ' '; }
lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }
# tilde <path> [max]: ~ for $HOME; longer than max characters -> "…" plus the tail.
tilde() {
  local p=$1 max=${2:-0}
  case $p in "$HOME") p='~' ;; "$HOME"/*) p="~${p#"$HOME"}" ;; esac
  if [ "$max" -gt 0 ] && [ "${#p}" -gt "$max" ]; then p="…${p:$((${#p} - max + 1))}"; fi
  printf '%s' "$p"
}
mtime_of() { stat -f %m "$1" 2>/dev/null || stat -c %Y "$1" 2>/dev/null || echo 0; }

new_uuid() {
  local u
  u=$(uuidgen 2>/dev/null) || u=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n' \
    | sed 's/^\(.\{8\}\)\(.\{4\}\)\(.\{4\}\)\(.\{4\}\)\(.\{12\}\)$/\1-\2-\3-\4-\5/')
  lower "$u"
}

# find_resident <name|slot|session-id prefix>: prints the record path, fails when unknown.
find_resident() {
  local f want
  want=$(lower "$1")
  # Slot or name first; a session-id prefix only when it is at least 4 characters, so that
  # `close 7` can never hit a session id that happens to start with 7.
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    rec_load "$f"
    if [ "$R_slot" = "$1" ] || [ "$(lower "$R_name")" = "$want" ]; then printf '%s\n' "$f"; return 0; fi
  done
  [ "${#want}" -ge 4 ] || return 1
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    case ${f##*/} in "$want"*) printf '%s\n' "$f"; return 0 ;; esac
  done
  return 1
}
record_by_pane() { grep -l "^pane=$1\$" "$RES_DIR"/* 2>/dev/null | head -n 1; }

# A fresh tmux server starts with the shrine alone; records of the previous run point at panes
# that no longer exist, so they move to state/departed/ (the recall menu reads them later).
archive_records() {
  local f
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    mkdir -p "$STATE_DIR/departed" && mv "$f" "$STATE_DIR/departed/"
    drop_side_files "${f##*/}"
  done
}

# prune_records: drop records whose pane was killed behind our back (tmux kill-pane, a
# closed window). A record without a pane yet is a summon in progress; keep it for 30 s.
prune_records() {
  local live f now
  live=$'\n'$(tmux_ list-panes -s -t "=$SESSION" -F '#{pane_id}' 2>/dev/null)$'\n'
  [ "$live" != $'\n\n' ] || return 0
  now=$(date +%s)
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    rec_load "$f"
    # A record that read back as nothing is not a record that says nothing: `read` gives up when
    # a signal lands in the middle of it, and the shrine's loop is signalled by every hook. The
    # cost of skipping a record here is one more tick before a dead pane is noticed; the cost of
    # trusting the empty read is deleting a resident who is sitting right there.
    [ -n "$R_slot" ] || continue
    if [ -n "$R_pane" ]; then
      case $live in *$'\n'"$R_pane"$'\n'*) ;; *) drop_record "$f" ;; esac
    elif [ $((now - ${R_launched:-0})) -gt 30 ]; then
      drop_record "$f"
    fi
  done
}
# drop_record <record>: the record and everything the hooks and status line kept for it.
drop_record() { rm -f "$1"; drop_side_files "${1##*/}"; }
drop_side_files() { rm -f "$STATE_DIR/status/$1" "$STATE_DIR/statusline/$1.json" "$STATE_DIR/statusline/$1.kv"; }

pick_name() {
  local file=$SHARE/names.txt used='' f n free
  [ -f "$CONFIG_DIR/names.txt" ] && file=$CONFIG_DIR/names.txt
  for f in "$RES_DIR"/*; do [ -f "$f" ] && used="$used"$'\n'"$(lower "$(rec_get "$f" name)")"; done
  used="$used"$'\n'
  free=()
  while IFS= read -r n || [ -n "$n" ]; do
    case $n in ''|'#'*) continue ;; esac
    case $used in *$'\n'"$(lower "$n")"$'\n'*) continue ;; esac
    free[${#free[@]}]=$n
  done < "$file"
  if [ "${#free[@]}" -gt 0 ]; then
    printf '%s\n' "${free[$((RANDOM % ${#free[@]}))]}"
  else
    printf 'Resident%s\n' "$((RANDOM % 900 + 100))"
  fi
}

# Slots are the smallest free number from 1; they are what `prefix N` and chips show.
next_slot() {
  local used='' f n=1
  for f in "$RES_DIR"/*; do [ -f "$f" ] && used="$used $(rec_get "$f" slot) "; done
  while :; do
    case $used in *" $n "*) n=$((n + 1)) ;; *) printf '%s\n' "$n"; return ;; esac
  done
}

remember_dir() {
  local f=$STATE_DIR/recent-dirs
  { printf '%s\n' "$1"; grep -v -x -F -e "$1" "$f" 2>/dev/null; } | head -n 20 > "$f.tmp.$$" && mv "$f.tmp.$$" "$f"
}
