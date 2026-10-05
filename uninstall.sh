#!/bin/sh
# uninstall.sh - remove what gensokyo put on this Mac.
#
#   ./uninstall.sh                what is there; nothing is deleted
#   ./uninstall.sh --yes          delete all of it
#   ./uninstall.sh --yes --keep-data   ... but keep your config, your rituals and the records
#   ./uninstall.sh --dir DIR      the install tree is DIR, not ~/.gensokyo (also: GENSOKYO_DIR)
#   curl -fsSL https://github.com/bubiche/gensokyo/releases/latest/download/uninstall.sh | sh
#
# `gensokyo uninstall` runs this and asks before deleting. Run by hand it asks nothing: without
# --yes it only says what it found, so a `curl | sh` of it can never delete anything by surprise.
#
# gensokyo writes in its own install tree, ~/.config/gensokyo, ~/.local/state/gensokyo and the
# one login agent `gensokyo login setup` adds. Claude Code's settings and sessions are read at
# most, never written.

set -eu

say() { printf '%s\n' "$*"; }
die() { printf 'uninstall.sh: %s\n' "$*" >&2; exit 1; }
usage() { sed -n 's/^#   //p' "$0"; }

yes=0 keep=0
dir=${GENSOKYO_DIR:-$HOME/.gensokyo}
config=${GENSOKYO_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/gensokyo}
state=${GENSOKYO_STATE_DIR:-$HOME/.local/state/gensokyo}
launch=${GENSOKYO_LAUNCH_DIR:-$HOME/Library/LaunchAgents}
label=${GENSOKYO_LAUNCH_LABEL:-io.github.bubiche.gensokyo}
launchctl=${GENSOKYO_LAUNCHCTL:-launchctl}
while [ $# -gt 0 ]; do
  case $1 in
    -y|--yes) yes=1 ;;
    --keep-data) keep=1 ;;
    --dir) [ $# -ge 2 ] || die "--dir needs a directory"; dir=$2; shift ;;
    --dir=*) dir=${1#--dir=} ;;
    -h|--help)
      if [ -f "$0" ] && grep -q '^# uninstall.sh' "$0" 2>/dev/null; then usage
      else say "uninstall.sh: ... | sh -s -- [--yes] [--keep-data] [--dir DIR]"
      fi
      exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

tilde() { case $1 in "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;; *) printf '%s' "$1" ;; esac; }

# Somewhere we may delete: named, absolute, and not something whose loss would be a story.
deletable() { case $1 in ''|/|"$HOME"|"$HOME"/) return 1 ;; /*) [ -e "$1" ] || [ -L "$1" ] ;; *) return 1 ;; esac; }

found=0
# gone <what> <path>: count it, and remove it if we were told to.
gone() {
  found=$((found + 1))
  if [ "$yes" != 1 ]; then say "  found  $1: $(tilde "$2")"; return 0; fi
  if rm -rf "$2"; then say "  removed  $1: $(tilde "$2")"
  else say "  COULD NOT REMOVE  $1: $(tilde "$2")"
  fi
}

# ---------------------------------------------------------------- is anything running?
# A running daemon means live Claude Code sessions, and deleting the records under one would
# orphan them. The daemon holds run/daemon.lock for as long as it lives.
lock="$state/run/daemon.lock"
if [ -f "$lock" ] && command -v lsof >/dev/null 2>&1; then
  pids=$(lsof -t "$lock" 2>/dev/null | tr '\n' ' ') || pids=''
  if [ -n "$pids" ]; then
    say "the gensokyo daemon is running (pid $pids), with residents in it."
    say "Stop it first: 'gensokyo quit' asks every resident to /exit."
    die "nothing was removed"
  fi
fi
# ---------------------------------------------------------------- what is there
if [ "$yes" = 1 ]; then say "gensokyo: removing what is left"; else say "gensokyo: what is here (nothing is deleted without --yes)"; fi

# The login agent, only if what it runs is a gensokyo. launchd lets go of it first, or it would
# keep the job until the next logout.
plist="$launch/$label.plist"
if [ -f "$plist" ]; then
  prog=$(plutil -extract 'ProgramArguments.0' raw -o - "$plist" 2>/dev/null) || prog=''
  case $prog in
    */bin/gensokyo)
      [ "$yes" = 1 ] && { "$launchctl" bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || :; }
      gone "login agent" "$plist" ;;
    *) say "  left alone  $(tilde "$plist") was not written by gensokyo (it runs ${prog:-something else})" ;;
  esac
fi

# The symlinks into this tree, dangling or not. A `gensokyo` that is a real file is somebody's
# own build, and a link to another copy is that copy's; both are left alone.
real_dir=$(cd "$dir" 2>/dev/null && pwd -P) || real_dir=$dir
seen=''
for c in "${GENSOKYO_BIN_DIR:-$HOME/.local/bin}/gensokyo" "$HOME/.local/bin/gensokyo" \
         "$HOME/bin/gensokyo" /usr/local/bin/gensokyo "$(command -v gensokyo 2>/dev/null || :)"; do
  [ -n "$c" ] && [ -L "$c" ] || continue
  case " $seen " in *" $c "*) continue ;; esac
  seen="$seen $c"
  to=$(readlink "$c")
  at=$(cd "$(dirname "$c")" && cd "$(dirname "$to")" 2>/dev/null && pwd -P) || at=''
  if [ "$to" = "$dir/bin/gensokyo" ] || [ "$at/$(basename "$to")" = "$real_dir/bin/gensokyo" ]; then
    gone "link" "$c"
  else
    case $to in */bin/gensokyo) say "  left alone  $(tilde "$c") points at another copy ($to)" ;; esac
  fi
done

# The install tree, if it is one: a release's VERSION beside its binary and share/.
if [ -f "$dir/bin/gensokyo" ] && [ -f "$dir/VERSION" ] && [ -f "$dir/share/names.txt" ]; then
  deletable "$dir" && gone "install tree" "$dir"
elif [ -d "$dir" ]; then
  say "  left alone  $(tilde "$dir") does not look like a gensokyo install (--dir names another)"
fi

# Your config and your rituals; the records, journals and run logs.
if [ "$keep" = 1 ]; then
  for p in "$config" "$state"; do deletable "$p" && say "  kept  $(tilde "$p") (--keep-data)"; done
else
  deletable "$config" && gone "config and rituals" "$config"
  deletable "$state" && gone "records, journal and run logs" "$state"
fi

# ---------------------------------------------------------------- what that came to
if [ "$found" = 0 ]; then
  say "nothing of gensokyo's is here."
elif [ "$yes" != 1 ]; then
  say "nothing was deleted. Add --yes to remove all of that (piped into sh: sh -s -- --yes)."
else
  say "done."
fi
say "Claude Code, its settings and its sessions are untouched."
