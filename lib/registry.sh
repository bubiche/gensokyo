# lib/registry.sh - the Claude Code session registry (`claude agents --json`), cached, and the
# per-resident state rows the bar, list and menus read. Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

# ---------------------------------------------------------------- registry
# `claude agents --json` (Claude Code 2.1.260) prints an array of
#   {pid, cwd, kind, startedAt, sessionId, name, status}
# with status idle|busy|waiting|null; `waiting` means a permission or plan dialog is open,
# null shows briefly for headless -p runs. registry_filter is the ONLY place that parses it,
# emitting one "sessionId|status|name|cwd|pid" line per session.
REGISTRY=$STATE_DIR/registry.json
REGISTRY_TTL=3
REG=$'\n'   # newline-prefixed registry_filter output, loaded by load_registry

registry_filter() {
  # shellcheck disable=SC2016
  jq_ -r '.[] | [.sessionId, (.status // "null"), (.name // ""), (.cwd // ""), (.pid | tostring)] | join("|")' \
    "$REGISTRY" 2>/dev/null
}

# Rewrite the cache when it is older than REGISTRY_TTL seconds (the CLI takes ~0.13 s).
# A failing `claude` keeps the previous cache; an invalid file is replaced by an empty array.
load_registry() {
  local now
  now=$(date +%s)
  if [ ! -f "$REGISTRY" ] || [ $((now - $(mtime_of "$REGISTRY"))) -ge "$REGISTRY_TTL" ]; then
    if claude_ agents --json > "$REGISTRY.tmp.$$" 2>/dev/null \
       && jq_ -e 'type == "array"' "$REGISTRY.tmp.$$" >/dev/null 2>&1; then
      mv "$REGISTRY.tmp.$$" "$REGISTRY"
    else
      rm -f "$REGISTRY.tmp.$$"
      [ -f "$REGISTRY" ] || printf '[]\n' > "$REGISTRY"
    fi
  fi
  REG=$'\n'$(registry_filter)
}

# registry_row <session-id>: "status|name|cwd|pid" for that session, or nothing.
registry_row() {
  local row
  case $REG in
    *$'\n'"$1|"*) row=${REG#*$'\n'"$1|"}; printf '%s\n' "${row%%$'\n'*}" ;;
  esac
}

# resident_rows: "slot|id|name|state|cwd|pane|window" per record, sorted by slot.
# state: busy|waiting|idle|departed|starting ("starting": launched, not in the registry yet).
# Precedence: departed record > registry status. The registry name wins over the recorded one
# (/rename inside the resident) and is written back so `close <name>` follows the rename.
resident_rows() {
  local f id row state rname
  for f in "$RES_DIR"/*; do
    [ -f "$f" ] || continue
    id=${f##*/}
    rec_load "$f"
    row=$(registry_row "$id")
    rname=$R_name
    if [ -n "$R_departed" ]; then
      state=departed
    elif [ -n "$row" ]; then
      state=${row%%|*}; case $state in busy|waiting|idle) ;; *) state=idle ;; esac
      rname=${row#*|}; rname=${rname%%|*}
      if [ -z "$rname" ]; then rname=$R_name; elif [ "$rname" != "$R_name" ]; then rec_set "$f" name "$rname"; fi
    else
      state=starting
    fi
    printf '%s|%s|%s|%s|%s|%s|%s\n' "$R_slot" "$id" "$rname" "$state" "$R_cwd" "${R_pane:--}" "$R_window"
  done | sort -n
}

# outsider_count: registry sessions gensokyo did not launch. They have no hooks, so their state
# would often be wrong; they get a count, not a chip.
outsider_count() {
  local line id n=0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    id=${line%%|*}
    [ -f "$RES_DIR/$id" ] || n=$((n + 1))
  done <<EOF
$REG
EOF
  printf '%s\n' "$n"
}

glyph_for() {
  case $1 in
    busy) printf '●' ;; waiting) printf '✦' ;; departed) printf '·' ;; *) printf '○' ;;
  esac
}
