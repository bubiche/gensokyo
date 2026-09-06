# lib/iterm.sh - the one file gensokyo may write into iTerm2's configuration, and only when
# asked for it: the "gensokyo" dynamic profile. iTerm2 watches its DynamicProfiles folder and
# picks up a new file at once, without a restart and without touching the preferences plist,
# so an additive profile is the only way to set anything up for the user that they would
# otherwise have to click through Settings for. Nothing else in iTerm2 is ever written: the
# application's own defaults (where tmux windows open, where the status bar sits) stay the
# user's, and `doctor` only reports them.

# The profile's identity, the same on every machine, so that installing over an older copy
# keeps the profile a window or a session may already be pointing at. It doubles as the mark
# of gensokyo's own file: a gensokyo.json whose Guid is not this one was written by someone
# else and is never overwritten or deleted.
ITERM_GUID=1AB52449-8D89-4E6A-97B2-800C66B98CF6

cmd_iterm() {
  case ${1:-} in
    setup)  iterm_setup ;;
    remove) iterm_remove ;;
    *)      die "usage: gensokyo iterm setup | remove" ;;
  esac
}

# iterm_render <parent profile>: the profile file as it is installed. The template ships with
# everything but the parent, which is the user's own default profile and so cannot be shipped:
# a dynamic profile inherits every key it does not name from `Dynamic Profile Parent Name`, and
# naming a profile that does not exist would leave gensokyo's looking nothing like the user's
# other tabs. A profile can be named anything at all, so the name is escaped twice on its way
# in - first for the JSON string it becomes, then for the sed replacement that puts it there -
# and a name with a quote or a backslash in it comes out of iTerm2 again exactly as it went in.
iterm_render() {
  local repl
  repl=$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/[\\&|]/\\&/g')
  sed "s|PARENT_PLACEHOLDER|$repl|" "$SHARE/iterm2/gensokyo.json"
}

# iterm_default_profile: the name of the profile iTerm2 opens new tabs with, which is what the
# gensokyo profile inherits from - font, colours, keys and the rest. `defaults` is asked for
# the default profile's id (iTerm2 keeps its preferences in memory while it runs, and cfprefsd
# answers for it), then the profile list is walked for the name that goes with it. Anything
# unreadable, or a name that could not be put in a JSON string, falls back to "Default", which
# is what iTerm2 ships with.
iterm_default_profile() {
  local plist=$HOME/Library/Preferences/com.googlecode.iterm2.plist guid name i=0
  guid=$(defaults read com.googlecode.iterm2 'Default Bookmark Guid' 2>/dev/null) || guid=
  if [ -n "$guid" ] && [ -f "$plist" ]; then
    while [ "$i" -lt 200 ] && name=$(plutil -extract "New Bookmarks.$i.Name" raw -o - "$plist" 2>/dev/null); do
      if [ "$(plutil -extract "New Bookmarks.$i.Guid" raw -o - "$plist" 2>/dev/null)" = "$guid" ]; then
        case $name in ''|*[\"\\]*) ;; *) printf '%s\n' "$name"; return 0 ;; esac
        break
      fi
      i=$((i + 1))
    done
  fi
  printf 'Default\n'
}

# The Guid inside a profile file, and whether that file is gensokyo's to write. A file that is
# not there yet is gensokyo's to write; one that cannot be read, or reads as another profile,
# is not. `plutil -extract` reads JSON as happily as a plist; `-lint` does not, so the check
# that the generated file parses is a conversion to nowhere.
iterm_guid_of()  { plutil -extract 'Profiles.0.Guid' raw -o - "$1" 2>/dev/null; }
iterm_parses()   { plutil -convert xml1 -o /dev/null "$1" 2>/dev/null; }
iterm_is_ours()  { [ -e "$1" ] || return 0; [ "$(iterm_guid_of "$1")" = "$ITERM_GUID" ]; }

iterm_setup() {
  local dest=$ITERM_DIR/gensokyo.json parent tmp
  command -v plutil >/dev/null 2>&1 || die "plutil not found: the iTerm2 profile is a macOS thing"
  [ -f "$SHARE/iterm2/gensokyo.json" ] || die "missing $SHARE/iterm2/gensokyo.json"
  iterm_is_ours "$dest" \
    || die "$(tilde "$dest") is not the profile gensokyo wrote (another Guid); move it aside first"
  parent=$(iterm_default_profile)
  mkdir -p "$ITERM_DIR" || die "cannot create $(tilde "$ITERM_DIR")"
  # Written beside the file it becomes, never in $TMPDIR: iTerm2 is watching this folder and a
  # rename within it is atomic, where a copy across volumes can be read half-written.
  tmp=$dest.new.$$
  iterm_render "$parent" > "$tmp" || { rm -f "$tmp"; die "cannot write $(tilde "$dest")"; }
  iterm_parses "$tmp" || { rm -f "$tmp"; die "the generated profile is not valid JSON: $(tilde "$tmp")"; }
  mv "$tmp" "$dest" || { rm -f "$tmp"; die "cannot write $(tilde "$dest")"; }
  say "wrote $(tilde "$dest"): the \"gensokyo\" profile, inheriting \"$parent\""
  say "iTerm2 has it already. Start the cockpit from a tab that uses it: open one from the"
  say "Profiles menu, or Settings > Profiles > gensokyo > Other Actions > Set as Default."
}

iterm_remove() {
  local dest=$ITERM_DIR/gensokyo.json
  if [ ! -e "$dest" ]; then say "nothing to remove: $(tilde "$dest") is not there"; return 0; fi
  iterm_is_ours "$dest" \
    || die "$(tilde "$dest") is not the profile gensokyo wrote (another Guid); leaving it alone"
  rm -f "$dest" || die "cannot remove $(tilde "$dest")"
  say "removed $(tilde "$dest"); iTerm2 drops the profile at once, open tabs keep their settings"
}
