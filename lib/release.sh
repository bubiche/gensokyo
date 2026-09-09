# lib/release.sh - the two commands that touch gensokyo's own install: update and uninstall.
#
# Where the releases live: the same defaults as install.sh, and the same two overrides, so a
# test can point at a directory of tarballs (curl speaks file://) and a fork at its own builds.
REL_REPO=${GENSOKYO_REPO:-bubiche/gensokyo}
REL_BASE=${GENSOKYO_RELEASE_BASE:-https://github.com/$REL_REPO/releases}

# A checkout updates with git and is nobody's to delete but its owner's; a release install is
# a tarball we unpacked and may replace or remove.
is_checkout() { [ -e "$GENSOKYO_HOME/.git" ]; }

sha256_of() {   # the same three-way fallback as scripts/vendor.sh and install.sh
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | sed 's/.* //'
  else die "need shasum, sha256sum or openssl to verify a download"
  fi
}

# our_links: the symlinks that point at this very copy, in the places install.sh puts them.
# Only a link that resolves to this script is ours; a `gensokyo` that is a real file, or one
# pointing at another copy, is left alone.
our_links() {
  local c seen=''
  for c in "${GENSOKYO_BIN_DIR:-$HOME/.local/bin}/gensokyo" "$HOME/.local/bin/gensokyo" \
           "$HOME/bin/gensokyo" /usr/local/bin/gensokyo "$(command -v gensokyo 2>/dev/null)"; do
    [ -n "$c" ] && [ -L "$c" ] || continue
    [ "$(real_path "$c")" = "$SELF" ] || continue
    case " $seen " in *" $c "*) continue ;; esac
    seen="$seen $c"
    printf '%s\n' "$c"
  done
}

# confirm <question>: yes only if the user says so out loud. With no terminal to ask - a pipe,
# a hook, the test suite - the answer is no, so nothing is ever deleted for want of an answer.
confirm() {
  local ans
  if [ ! -t 0 ]; then say "  $1 - no terminal to ask, so no (--yes deletes without asking)"; return 1; fi
  printf '  %s [y/N] ' "$1"
  IFS= read -r ans || { say ''; return 1; }
  case $ans in y|Y|yes|YES|Yes) return 0 ;; *) return 1 ;; esac
}

# A path we are allowed to delete: ours, named, and not something whose loss would be a story.
deletable() {
  case $1 in
    ''|/|"$HOME"|"$HOME"/) return 1 ;;
    /*) [ -e "$1" ] ;;   # -e, not -d: `rm -rf` takes a stray file just as well
    *) return 1 ;;
  esac
}

# ---------------------------------------------------------------- update
# The version of the newest release comes from a one-line VERSION asset, which GitHub serves
# from releases/latest/download/ for whatever release is newest. The API would answer this too
# and rate-limits by IP address, so it is not asked.
rel_latest() {
  curl -fsSL --retry 2 "$REL_BASE/latest/download/VERSION" 2>/dev/null | head -n 1 | tr -d ' \r'
}

# rel_fetch <version> <workdir>: download this platform's tarball and the release's SHA256SUMS,
# verify one against the other and unpack it; REL_TREE is where it landed. Nothing that did not
# verify is ever unpacked - the same rule as scripts/vendor.sh. The result comes back in a global
# because a `die` inside a command substitution would only kill the subshell.
REL_TREE=''
rel_fetch() {
  local ver=$1 work=$2 name want got
  name="gensokyo-$ver-$PLATFORM.tar.gz"
  say "downloading $name"
  curl -fsSL --retry 3 -o "$work/$name" "$REL_BASE/download/v$ver/$name" ||
    die "cannot download $REL_BASE/download/v$ver/$name (no such release, or no build for $PLATFORM)"
  curl -fsSL --retry 3 -o "$work/SHA256SUMS" "$REL_BASE/download/v$ver/SHA256SUMS" ||
    die "cannot download that release's SHA256SUMS; refusing to install an unverified tarball"
  want=$(awk -v f="$name" '$2 == f { print $1 }' "$work/SHA256SUMS")
  [ -n "$want" ] || die "that release's SHA256SUMS has no line for $name"
  got=$(sha256_of "$work/$name")
  [ "$want" = "$got" ] || die "checksum mismatch for $name
  expected $want
  got      $got
Refusing to install. Do not bypass this."
  tar -xzf "$work/$name" -C "$work" || die "cannot unpack $name"
  [ -x "$work/gensokyo-$ver/bin/gensokyo" ] || die "$name does not hold gensokyo-$ver/bin/gensokyo"
  REL_TREE="$work/gensokyo-$ver"
}

cmd_update() {
  local want='' check=0 latest parent work old
  while [ $# -gt 0 ]; do
    case $1 in
      --check) check=1 ;;
      --version) [ $# -ge 2 ] || die "--version needs a release version"; want=$2; shift ;;
      --version=*) want=${1#--version=} ;;
      *) die "usage: gensokyo update [--check] [--version VERSION]" ;;
    esac
    shift
  done
  ! is_checkout ||
    die "$(tilde "$GENSOKYO_HOME") is a git checkout, not a release install: 'git pull' updates it"
  command -v curl >/dev/null 2>&1 || die "curl is required to fetch a release"
  latest=$want
  if [ -z "$latest" ]; then
    latest=$(rel_latest)
    [ -n "$latest" ] ||
      die "cannot tell which release is the latest ($REL_BASE/latest/download/VERSION did not answer); try again, or 'gensokyo update --version X'"
  fi
  if [ "$latest" = "$VERSION" ]; then
    say "gensokyo $VERSION is already the release you asked for"
    return 0
  fi
  if [ "$check" = 1 ]; then
    say "gensokyo $VERSION is installed; $latest is out ('gensokyo update' fetches it)"
    return 0
  fi
  # The new tree is unpacked next to the old one so that putting it in place is a rename on the
  # same filesystem: the swap is one instant, not a copy that could be interrupted half-way.
  parent=$(dirname "$GENSOKYO_HOME")
  [ -w "$parent" ] || die "cannot write in $(tilde "$parent"): nothing to do here but reinstall"
  work=$(mktemp -d "$parent/.gensokyo-update.XXXXXX") || die "cannot make a directory in $(tilde "$parent")"
  # shellcheck disable=SC2064  # $work is wanted as it is now, not as it may be later
  trap "rm -rf '$work'" EXIT INT TERM
  rel_fetch "$latest" "$work"
  old="$GENSOKYO_HOME.old.$$"
  mv "$GENSOKYO_HOME" "$old" || die "cannot move $(tilde "$GENSOKYO_HOME") aside"
  if ! mv "$REL_TREE" "$GENSOKYO_HOME"; then
    mv "$old" "$GENSOKYO_HOME" || warn "and the old copy is still at $(tilde "$old")"
    die "cannot move the new tree into $(tilde "$GENSOKYO_HOME"); nothing was changed"
  fi
  rm -rf "$old"
  say "gensokyo $VERSION -> $latest in $(tilde "$GENSOKYO_HOME")"
  # The symlink points into the tree by path, not by inode, so it needs nothing done to it, and
  # config and state live outside the tree and are untouched. What does need saying is that a
  # running cockpit is still running the code it started with.
  if [ -n "$TMUX_BIN" ] && server_running; then
    say "the cockpit is running the old code: 'gensokyo reload' runs this one instead"
  fi
}

# ---------------------------------------------------------------- uninstall
cmd_uninstall() {
  local yes=0 keep=0 link links
  while [ $# -gt 0 ]; do
    case $1 in
      -y|--yes) yes=1 ;;
      --keep-data) keep=1 ;;
      *) die "usage: gensokyo uninstall [--yes] [--keep-data]" ;;
    esac
    shift
  done
  if [ -n "$TMUX_BIN" ] && server_running; then
    die "the cockpit is running: 'gensokyo quit' first (it asks every resident to /exit), then uninstall"
  fi
  say "gensokyo $VERSION in $(tilde "$GENSOKYO_HOME")"

  # The symlinks: only the ones that resolve to this copy.
  links=$(our_links)
  if [ -n "$links" ]; then
    while IFS= read -r link; do
      if rm -f "$link"; then say "  removed the link $(tilde "$link")"; else warn "cannot remove $(tilde "$link")"; fi
    done <<EOF
$links
EOF
  else
    say "  no link to this copy in the usual places"
  fi

  # The iTerm2 dynamic profile, if it is the one gensokyo wrote. iterm_remove says what it did
  # and refuses a file with another Guid, which is what we want here too.
  if [ -e "$ITERM_DIR/gensokyo.json" ]; then
    if iterm_is_ours "$ITERM_DIR/gensokyo.json"; then
      if rm -f "$ITERM_DIR/gensokyo.json"; then say "  removed the iTerm2 profile $(tilde "$ITERM_DIR/gensokyo.json")"
      else warn "cannot remove $(tilde "$ITERM_DIR/gensokyo.json")"; fi
    else
      say "  left $(tilde "$ITERM_DIR/gensokyo.json") alone: another Guid, so not the profile gensokyo wrote"
    fi
  fi
  say "  iTerm2's own settings are yours and were never written to; the same for ~/.tmux.conf and ~/.claude"

  # Your rituals, your config, the records of who has been summoned: offered, never assumed.
  if [ "$keep" = 1 ]; then
    say "  kept $(tilde "$CONFIG_DIR") and $(tilde "$STATE_DIR") (--keep-data)"
  elif ! deletable "$CONFIG_DIR" && ! deletable "$STATE_DIR"; then
    say "  no config or state to delete"
  else
    say "  your config and state: $(tilde "$CONFIG_DIR"), $(tilde "$STATE_DIR")"
    if [ "$yes" = 1 ] || confirm "delete them? your rituals and config are in there"; then
      for link in "$CONFIG_DIR" "$STATE_DIR"; do
        deletable "$link" || continue
        if rm -rf "$link"; then say "  removed $(tilde "$link")"; else warn "cannot remove $(tilde "$link")"; fi
      done
    else
      say "  kept them"
    fi
  fi

  # This copy of gensokyo. A checkout is the owner's, whatever they say.
  if is_checkout; then
    say "  $(tilde "$GENSOKYO_HOME") is a git checkout: left alone (delete it yourself if you want to)"
  elif ! deletable "$GENSOKYO_HOME"; then
    say "  $(tilde "$GENSOKYO_HOME") is not a directory gensokyo may delete: left alone"
  elif [ "$yes" = 1 ] || confirm "delete this copy of gensokyo, $(tilde "$GENSOKYO_HOME")?"; then
    # Safe to delete the tree we are running from: this script and the lib files it sourced are
    # read and parsed already, and Unix keeps an unlinked file alive for whoever holds it open.
    if rm -rf "$GENSOKYO_HOME"; then say "  removed $(tilde "$GENSOKYO_HOME")"; else warn "cannot remove $(tilde "$GENSOKYO_HOME")"; fi
  else
    say "  kept $(tilde "$GENSOKYO_HOME")"
  fi
  say "done. Claude Code, its sessions and its settings are untouched."
  return 0
}
