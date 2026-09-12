# tests/cases/login.sh - the launchd agent that starts the cockpit at login: what goes into the
# plist, what gensokyo refuses to write over, and what it tells launchd to do. Sourced by
# tests/run.sh, which holds the harness. GENSOKYO_LAUNCH_DIR points into the scratch dir and
# GENSOKYO_LAUNCHCTL at a stand-in that only writes down its arguments, so nothing here loads
# an agent onto the machine running the suite.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

login_tests() {
  local out plist log
  plist=$(login_plist)
  log=$scratch/launchctl.log
  t "the agent names this copy of gensokyo, runs it at login, and carries a PATH that finds claude"
  if ! command -v plutil >/dev/null 2>&1; then skip "no plutil (macOS only)"; return; fi
  rm -rf "$GENSOKYO_LAUNCH_DIR"; rm -f "$log" "$scratch/launchctl.loaded"
  login_render > "$scratch/agent.plist"
  assert_ok plutil -lint "$scratch/agent.plist"
  assert_eq "$(plutil -extract 'Label' raw -o - "$scratch/agent.plist")" "$LOGIN_LABEL"
  assert_eq "$(plutil -extract 'ProgramArguments.0' raw -o - "$scratch/agent.plist")" "$SELF"
  assert_eq "$(plutil -extract 'ProgramArguments.1' raw -o - "$scratch/agent.plist")" --detach
  assert_eq "$(plutil -extract 'RunAtLoad' raw -o - "$scratch/agent.plist")" true
  # launchd hands an agent /usr/bin:/bin:/usr/sbin:/sbin and nothing else, and `claude` is on
  # neither of those; a PATH that is not carried over is the whole failure mode.
  assert_eq "$(plutil -extract 'EnvironmentVariables.PATH' raw -o - "$scratch/agent.plist")" "$PATH"
  # KeepAlive would respawn `--detach`, which returns the moment the server is up.
  assert_fails plutil -extract 'KeepAlive' raw -o - "$scratch/agent.plist"
  # A login that goes wrong has to say so somewhere a person can find it afterwards.
  assert_eq "$(plutil -extract 'StandardOutPath' raw -o - "$scratch/agent.plist")" "$STATE_DIR/login.log"
  assert_eq "$(plutil -extract 'StandardErrorPath' raw -o - "$scratch/agent.plist")" "$STATE_DIR/login.log"
  # The environment this was set up with is the environment it runs in: the suite's own socket
  # and directories are in there, which is exactly what a second cockpit would need too.
  assert_eq "$(plutil -extract 'EnvironmentVariables.GENSOKYO_SOCKET' raw -o - "$scratch/agent.plist")" "$SOCKET"
  assert_eq "$(plutil -extract 'EnvironmentVariables.GENSOKYO_STATE_DIR' raw -o - "$scratch/agent.plist")" "$STATE_DIR"

  t "a home directory with XML in its name still produces a plist launchd can read"
  out=$(HOME='/tmp/a & b <c>' login_render)
  printf '%s' "$out" > "$scratch/amp.plist"
  assert_ok plutil -lint "$scratch/amp.plist"
  assert_eq "$(plutil -extract 'WorkingDirectory' raw -o - "$scratch/amp.plist")" '/tmp/a & b <c>'
  assert_nomatch "$out" '<string>/tmp/a & b <c></string>'

  t "ritual login setup writes the agent, tells launchd to take it, and runs again unchanged"
  out=$(cmd_ritual_login setup 2>&1)
  assert_match "$out" "wrote $plist"
  assert_match "$out" 'when you log in'
  assert_ok plutil -lint "$plist"
  assert_eq "$(ls "$GENSOKYO_LAUNCH_DIR")" "$LOGIN_LABEL.plist"   # no half-written temp file
  # `bootstrap` refuses a label the domain already holds, so the old one always comes out first.
  assert_match "$(cat "$log")" "bootout gui/$(id -u)/$LOGIN_LABEL"
  assert_match "$(cat "$log")" "bootstrap gui/$(id -u) $plist"
  assert_eq "$(grep -c '^bootout' "$log")" 1
  assert_ok cmd_ritual_login setup
  assert_eq "$(grep -c '^bootout' "$log")" 2   # ... and again on the second run, before the second bootstrap
  assert_eq "$(ls "$GENSOKYO_LAUNCH_DIR")" "$LOGIN_LABEL.plist"

  t "what it reports: loaded, not loaded, and stale when the gensokyo it names has gone"
  rm -f "$scratch/launchctl.loaded"
  assert_match "$(login_status_line)" 'installed, not loaded'
  : > "$scratch/launchctl.loaded"
  assert_match "$(login_status_line)" "on: $(tilde "$SELF") runs at login"
  assert_match "$(cmd_ritual_login)" 'the cockpit at login: on:'
  assert_match "$(cmd_ritual_login status)" 'runs at login'
  assert_match "$(cmd_doctor)" 'at login   on:'
  # The silent failure this exists to catch: a plist left pointing at a tree that is gone.
  sed "s|<string>$SELF</string>|<string>/nowhere/bin/gensokyo</string>|" "$plist" > "$plist.x" && mv "$plist.x" "$plist"
  assert_eq "$(login_program)" /nowhere/bin/gensokyo
  assert_match "$(login_status_line)" 'STALE: names /nowhere/bin/gensokyo'
  assert_match "$(cmd_doctor)" 'at login   STALE'
  # Stale is still gensokyo's own, so setup repoints it rather than refusing.
  assert_ok cmd_ritual_login setup
  assert_eq "$(login_program)" "$SELF"

  t "the agent's log is read back, and nothing is said when it is empty"
  rm -f "$STATE_DIR/login.log"
  assert_nomatch "$(cmd_ritual_login)" 'it last said'
  : > "$STATE_DIR/login.log"
  assert_nomatch "$(cmd_ritual_login)" 'it last said'   # empty is not something to report
  printf 'gensokyo: claude not found on PATH\n' > "$STATE_DIR/login.log"
  out=$(cmd_ritual_login)
  assert_match "$out" "it last said, in $(tilde "$STATE_DIR/login.log")"
  assert_match "$out" '    gensokyo: claude not found on PATH'
  rm -f "$STATE_DIR/login.log"

  t "an agent at that label gensokyo did not write is never loaded, replaced or deleted"
  printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>' \
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
    '<plist version="1.0"><dict><key>Label</key><string>someone.else</string>' \
    '<key>ProgramArguments</key><array><string>/bin/echo</string></array></dict></plist>' > "$plist"
  assert_fails login_is_ours
  assert_fails login_program
  if out=$(cmd_ritual_login setup 2>&1); then bad "setup should refuse"; else assert_match "$out" 'was not written by gensokyo'; fi
  if out=$(cmd_ritual_login remove 2>&1); then bad "remove should refuse"; else assert_match "$out" 'leaving it alone'; fi
  assert_eq "$(plutil -extract 'ProgramArguments.0' raw -o - "$plist")" /bin/echo   # still theirs
  assert_match "$(login_status_line)" 'holds an agent gensokyo did not write'
  printf 'not a plist at all\n' > "$plist"    # unreadable counts as somebody else's
  assert_fails login_is_ours

  t "ritual login remove takes the agent out of launchd and off the disk, and says when there is none"
  rm -f "$plist" "$log"; cmd_ritual_login setup >/dev/null 2>&1; : > "$log"
  out=$(cmd_ritual_login remove)
  assert_match "$out" "removed $plist"
  assert_match "$out" "$(tilde "$SELF") no longer starts at login"
  assert_match "$(cat "$log")" "bootout gui/$(id -u)/$LOGIN_LABEL"
  assert_eq "$(ls "$GENSOKYO_LAUNCH_DIR")" ''
  assert_match "$(cmd_ritual_login remove)" 'nothing to remove'
  assert_match "$(login_status_line)" 'not installed'
  assert_match "$(cmd_doctor)" 'at login   not installed'

  t "ritual login is reachable by that name, and refuses a verb it does not have"
  assert_match "$(cmd_ritual login)" 'the cockpit at login'
  if out=$(cmd_ritual login frobnicate 2>&1); then bad "an unknown verb should refuse"; else assert_match "$out" "nothing called 'frobnicate'"; fi
  assert_match "$(cmd_ritual_login --help)" 'usage: gensokyo ritual login'
  assert_match "$(cmd_ritual nonsense 2>&1 || true)" 'ritual login [setup | remove]'
  assert_match "$(cmd_help)" 'login'
}
