#!/bin/sh
# uninstall.sh - remove what gensokyo put on this Mac, without needing gensokyo itself.
#
#   ./uninstall.sh                what is there; nothing is deleted
#   ./uninstall.sh --yes          delete all of it
#   ./uninstall.sh --yes --keep-data   ... but keep your config, your rituals and the records
#   ./uninstall.sh --dir DIR      the install tree is DIR, not ~/.gensokyo (also: GENSOKYO_DIR)
#   curl -fsSL https://github.com/bubiche/gensokyo/releases/latest/download/uninstall.sh | sh
#
# `gensokyo uninstall` does the same thing and asks as it goes. This is for when that is not
# there any more: somebody deleted ~/.gensokyo, or the symlink, and these files are what is
# left. It therefore assumes nothing about the tree, and asks no questions - without --yes it
# only says what it found, so a `curl | sh` of it can never delete anything by surprise.
#
# gensokyo writes in four places and nowhere else: its own install tree, ~/.config/gensokyo,
# ~/.local/state/gensokyo and one iTerm2 dynamic profile of its own. Claude Code's settings and
# sessions, ~/.tmux.conf and iTerm2's own preferences are read at most, never written.

set -eu

say() { printf '%s\n' "$*"; }
die() { printf 'uninstall.sh: %s\n' "$*" >&2; exit 1; }
usage() { sed -n 's/^#   //p' "$0"; }

# Must match ITERM_GUID in lib/iterm.sh: it is how a gensokyo.json is told from a file of the
# same name that somebody else wrote, which is never touched.
GUID=1AB52449-8D89-4E6A-97B2-800C66B98CF6

yes=0 keep=0
dir=${GENSOKYO_DIR:-$HOME/.gensokyo}
config=${GENSOKYO_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/gensokyo}
state=${GENSOKYO_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/gensokyo}
iterm=${GENSOKYO_ITERM_DIR:-$HOME/Library/Application Support/iTerm2/DynamicProfiles}
socket=${GENSOKYO_SOCKET:-gensokyo}
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
deletable() { case $1 in ''|/|"$HOME"|"$HOME"/) return 1 ;; /*) [ -e "$1" ] ;; *) return 1 ;; esac; }

found=0
# gone <what> <path>: count it, and remove it if we were told to.
gone() {
  found=$((found + 1))
  if [ "$yes" != 1 ]; then say "  found  $1: $(tilde "$2")"; return 0; fi
  if rm -rf "$2"; then say "  removed  $1: $(tilde "$2")"
  else say "  COULD NOT REMOVE  $1: $(tilde "$2")"
  fi
}

# ---------------------------------------------------------------- is the cockpit running?
# A running cockpit means live Claude Code sessions, and deleting the records under one would
# orphan them. gensokyo's tmux server has a socket of its own, so this asks that server - with
# the vendored tmux if the tree is still there, otherwise with whatever tmux is on PATH. If
# there is no tmux to ask with, the socket is reported and the rest goes ahead: say so rather
# than pretend either way.
sockdir=${TMUX_TMPDIR:-/tmp}/tmux-$(id -u 2>/dev/null || echo 0)
sock="$sockdir/$socket"
tmux_bin=''
for c in "$dir/vendor/macos-arm64/tmux" "$dir/vendor/macos-x86_64/tmux" "$(command -v tmux 2>/dev/null)"; do
  [ -n "$c" ] && [ -x "$c" ] && { tmux_bin=$c; break; }
done
if [ -S "$sock" ] && [ -n "$tmux_bin" ] && "$tmux_bin" -L "$socket" has-session 2>/dev/null; then
  say "the cockpit is still running on tmux socket '$socket', with residents in it."
  say "Stop it first - 'gensokyo quit' if you still have the command, or ask tmux yourself:"
  say "  $tmux_bin -L $socket kill-server      (this does not ask the residents to /exit)"
  die "nothing was removed"
fi

# ---------------------------------------------------------------- what is there
if [ "$yes" = 1 ]; then say "gensokyo: removing what is left"; else say "gensokyo: what is here (nothing is deleted without --yes)"; fi

# The symlinks. A `gensokyo` that is a real file is somebody's own build and is left alone; a
# symlink into a tree's bin/gensokyo is ours, dangling or not.
seen=''
for c in "${GENSOKYO_BIN_DIR:-$HOME/.local/bin}/gensokyo" "$HOME/.local/bin/gensokyo" \
         "$HOME/bin/gensokyo" /usr/local/bin/gensokyo "$(command -v gensokyo 2>/dev/null)"; do
  [ -n "$c" ] && [ -L "$c" ] || continue
  case " $seen " in *" $c "*) continue ;; esac
  seen="$seen $c"
  case $(readlink "$c") in */bin/gensokyo) gone "link" "$c" ;; esac
done

# The install tree, if it is one: three files no other directory has together.
if [ -f "$dir/bin/gensokyo" ] && [ -f "$dir/lib/rituals.sh" ] && [ -f "$dir/share/tmux.conf" ]; then
  deletable "$dir" && gone "install tree" "$dir"
elif [ -d "$dir" ]; then
  say "  left alone  $(tilde "$dir") does not look like a gensokyo install (--dir names another)"
fi

# The iTerm2 dynamic profile, only the one carrying gensokyo's own Guid.
prof="$iterm/gensokyo.json"
if [ -f "$prof" ]; then
  if command -v plutil >/dev/null 2>&1 &&
     [ "$(plutil -extract 'Profiles.0.Guid' raw -o - "$prof" 2>/dev/null)" = "$GUID" ]; then
    gone "iTerm2 profile" "$prof"
  else
    say "  left alone  $(tilde "$prof") was not written by gensokyo (another Guid)"
  fi
fi

# Your config and your rituals; the records, journals and run logs.
if [ "$keep" = 1 ]; then
  for p in "$config" "$state"; do deletable "$p" && say "  kept  $(tilde "$p") (--keep-data)"; done
else
  deletable "$config" && gone "config and rituals" "$config"
  deletable "$state" && gone "records, journal and run logs" "$state"
fi

# tmux leaves its socket behind when its server goes; by here we know nothing is listening.
if [ -S "$sock" ] && [ -n "$tmux_bin" ]; then
  gone "dead tmux socket" "$sock"
elif [ -S "$sock" ]; then
  say "  left alone  $sock: no tmux here to ask whether a server is using it"
fi

# ---------------------------------------------------------------- what that came to
if [ "$found" = 0 ]; then
  say "nothing of gensokyo's is here."
elif [ "$yes" != 1 ]; then
  say "nothing was deleted. Add --yes to remove all of that (piped into sh: sh -s -- --yes)."
else
  say "done."
fi
say "Claude Code, its settings and its sessions are untouched, and so are ~/.tmux.conf and iTerm2's own settings."
