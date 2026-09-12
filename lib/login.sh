# lib/login.sh - the launchd agent that starts the cockpit when you log in, so that rituals
# fire on a day you never opened a terminal. Sourced by bin/gensokyo; bash 3.2.
#
# A ritual only fires while the tmux server is running: that is what `gensokyo --detach` is
# for, and this is how it gets run without anybody remembering to. launchd runs the agent at
# login, the server comes up, the sweep starts, and `catch_up` covers whatever was missed.
#
# This and lib/iterm.sh are the only two places gensokyo writes outside its own three
# directories, and both only when asked to by name. What goes in is one plist of gensokyo's
# own under ~/Library/LaunchAgents; launchd's own configuration is never touched.

# One label for every copy of gensokyo on the Mac, on purpose. Two agents would both run
# `--detach` against the same tmux socket at login and race for it, so setting up from a
# second copy replaces the first rather than joining it - which is why `status` and `remove`
# always print the path in the plist, and not the one they were run from.
LOGIN_LABEL=io.github.bubiche.gensokyo
LOGIN_DIR=${GENSOKYO_LAUNCH_DIR:-$HOME/Library/LaunchAgents}
# The tests hand this a stand-in that writes its arguments down instead of loading anything:
# a suite that ran the real one would leave an agent behind on the machine it ran on.
launchctl_() { "${GENSOKYO_LAUNCHCTL:-launchctl}" "$@"; }
login_plist() { printf '%s/%s.plist' "$LOGIN_DIR" "$LOGIN_LABEL"; }
login_target() { printf 'gui/%s/%s' "$(id -u)" "$LOGIN_LABEL"; }

# ---------------------------------------------------------------- the plist
# XML has five characters that cannot be written as themselves, and a path may hold three of
# them: `&` and `<` are legal in a directory name, and a home directory is the user's to name.
xml_escape() {
  local s=$1
  s=${s//&/&amp;}; s=${s//</&lt;}; s=${s//>/&gt;}
  printf '%s' "$s"
}
login_kv() { printf '    <key>%s</key><string>%s</string>\n' "$(xml_escape "$1")" "$(xml_escape "$2")"; }

# login_render: the agent as it is installed.
#
# RunAtLoad and nothing else. `gensokyo --detach` starts the server and returns straight away
# (lib/cockpit.sh), so KeepAlive would read that exit as a crash and respawn it until launchd
# throttled the job - the server it started is the long-running thing, not this.
#
# PATH is baked in because launchd gives an agent /usr/bin:/bin:/usr/sbin:/sbin and nothing
# else. tmux and jq are vendored and resolve from the install tree, but `claude` does not: it
# lives under Homebrew or ~/.claude/local, neither of which is on that list. Without this the
# agent would start at every login, fail to find claude, and fire nothing.
#
# The GENSOKYO_* variables carry over only when they are set in the environment setting this
# up, so an install that runs against its own socket or state directory keeps doing so.
login_render() {
  local v
  printf '<?xml version="1.0" encoding="UTF-8"?>\n'
  printf '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
  printf '<plist version="1.0">\n<dict>\n'
  login_kv Label "$LOGIN_LABEL"
  printf '    <key>ProgramArguments</key>\n    <array>\n'
  printf '      <string>%s</string>\n' "$(xml_escape "$SELF")" '--detach'
  printf '    </array>\n'
  printf '    <key>RunAtLoad</key><true/>\n'
  # A login agent starts in / with TERM=dumb. The directory it runs in reaches a ritual only
  # through the ritual's own cwd, but / is nobody's idea of a working directory.
  login_kv WorkingDirectory "$HOME"
  # An agent that failed at login and said so nowhere is the worst way for this to break: the
  # user finds out by noticing that nothing has run for a week.
  login_kv StandardOutPath "$STATE_DIR/login.log"
  login_kv StandardErrorPath "$STATE_DIR/login.log"
  printf '    <key>EnvironmentVariables</key>\n    <dict>\n'
  login_kv PATH "$PATH"
  for v in GENSOKYO_TMUX GENSOKYO_JQ GENSOKYO_CLAUDE GENSOKYO_STATE_DIR GENSOKYO_CONFIG_DIR \
           GENSOKYO_ITERM_DIR GENSOKYO_SOCKET CLAUDE_CONFIG_DIR; do
    eval "[ -n \"\${$v:-}\" ]" && login_kv "$v" "$(eval "printf '%s' \"\$$v\"")"
  done
  printf '    </dict>\n</dict>\n</plist>\n'
}

# ---------------------------------------------------------------- what is there now
# The gensokyo the installed agent runs, or nothing. A plist at our own label that does not run
# a gensokyo was written by somebody else and is never loaded, replaced or deleted - the same
# rule the iTerm2 profile follows, for the same reason.
login_program() {
  local p
  p=$(login_plist)
  [ -f "$p" ] || return 1
  command -v plutil >/dev/null 2>&1 || return 1
  p=$(plutil -extract 'ProgramArguments.0' raw -o - "$p" 2>/dev/null) || return 1
  case $p in */bin/gensokyo) printf '%s\n' "$p" ;; *) return 1 ;; esac
}
login_is_ours() { [ -e "$(login_plist)" ] || return 0; login_program >/dev/null; }
# Loaded means launchd is holding it: `print` answers 0 for a service in the domain and 113
# for one that is not there.
login_loaded() { launchctl_ print "$(login_target)" >/dev/null 2>&1; }

# login_status_line: the one line doctor and `ritual login` both say. Four states worth
# telling apart, and the third is the one that fails silently: a plist left pointing at a tree
# that has been deleted or moved loads at every login and dies before it reaches main().
login_status_line() {
  local prog
  if [ ! -e "$(login_plist)" ]; then
    printf 'not installed: %s\n' "'gensokyo ritual login setup' starts the cockpit at login, so rituals fire on a day you never opened a terminal"
  elif ! prog=$(login_program); then
    printf '%s holds an agent gensokyo did not write: left alone\n' "$(tilde "$(login_plist)")"
  elif [ ! -x "$prog" ]; then
    printf 'STALE: names %s, which is not there any more - it fails at every login ("gensokyo ritual login setup" repoints it)\n' "$(tilde "$prog")"
  elif ! login_loaded; then
    printf 'installed, not loaded: %s (it loads at your next login; "gensokyo ritual login setup" loads it now)\n' "$(tilde "$prog")"
  else
    printf 'on: %s runs at login\n' "$(tilde "$prog")"
  fi
}

# ---------------------------------------------------------------- the command
LOGIN_USAGE='usage: gensokyo ritual login [setup | remove]
       alone: whether the cockpit starts at login, and which copy of gensokyo it starts'

cmd_ritual_login() {
  case ${1:-} in
    ''|status) say "the cockpit at login: $(login_status_line)"; login_log_tail ;;
    setup)     login_setup ;;
    remove)    login_remove ;;
    -h|--help) say "$LOGIN_USAGE" ;;
    *)         die "ritual login: nothing called '$1' to do"$'\n'"$LOGIN_USAGE" ;;
  esac
}

# login_log_tail: what the agent said the last time it ran. Only ever a few lines, and only
# when there are any: a working agent writes nothing at all.
login_log_tail() {
  local f=$STATE_DIR/login.log
  [ -s "$f" ] || return 0
  say "  it last said, in $(tilde "$f"):"
  tail -n 5 "$f" | sed 's/^/    /'
  return 0
}

login_setup() {
  local dest tmp
  dest=$(login_plist)
  [ "$(uname -s)" = Darwin ] || die "the login agent is a macOS thing (launchd); this is $(uname -s)"
  command -v plutil >/dev/null 2>&1 || die "plutil not found: it is what checks the agent parses before launchd is given it"
  login_is_ours || die "$(tilde "$dest") was not written by gensokyo (it runs $(plutil -extract 'ProgramArguments.0' raw -o - "$dest" 2>/dev/null || echo 'something else')); move it aside first"
  # The agent runs `--detach`, which needs all three binaries; refusing here with the reason is
  # better than installing something that will fail at login into a log nobody reads.
  need_bins
  ensure_dirs
  mkdir -p "$LOGIN_DIR" || die "cannot create $(tilde "$LOGIN_DIR")"
  tmp=$dest.new.$$
  login_render > "$tmp" || { rm -f "$tmp"; die "cannot write $(tilde "$dest")"; }
  plutil -lint "$tmp" >/dev/null 2>&1 || { rm -f "$tmp"; die "the agent gensokyo generated is not a valid plist: $(tilde "$tmp")"; }
  mv "$tmp" "$dest" || { rm -f "$tmp"; die "cannot write $(tilde "$dest")"; }
  # `bootstrap` refuses a label the domain already holds (5: Input/output error), so the old
  # one comes out first whether or not there was one - a `bootout` with nothing to boot out
  # says "3: No such process" and has done nothing, which is the state we wanted anyway.
  launchctl_ bootout "$(login_target)" >/dev/null 2>&1
  if ! launchctl_ bootstrap "gui/$(id -u)" "$dest" 2>/dev/null; then
    warn "launchd would not take $(tilde "$dest"); it is written, and loads at your next login"
  fi
  say "wrote $(tilde "$dest"): launchd runs '$(tilde "$SELF") --detach' when you log in."
  say "It has just run it, as well, so the cockpit is starting now and 'gensokyo' attaches to it."
  say "Anything it says goes to $(tilde "$STATE_DIR/login.log"); 'gensokyo ritual login' reads it back."
}

login_remove() {
  local dest prog
  dest=$(login_plist)
  if [ ! -e "$dest" ]; then say "nothing to remove: $(tilde "$dest") is not there"; return 0; fi
  login_is_ours || die "$(tilde "$dest") was not written by gensokyo: leaving it alone"
  prog=$(login_program) || prog=$dest
  launchctl_ bootout "$(login_target)" >/dev/null 2>&1
  rm -f "$dest" || die "cannot remove $(tilde "$dest")"
  say "removed $(tilde "$dest"): $(tilde "$prog") no longer starts at login."
  say "A cockpit that is already running keeps running; 'gensokyo quit' stops it."
}
