# tests/cases/iterm.sh - the iTerm2 dynamic profile: what is rendered, and what gensokyo
# refuses to write over. Sourced by tests/run.sh, which holds the harness, the scratch state
# dir and the sourced bin/gensokyo these tests call. ITERM_DIR points into the scratch dir
# (GENSOKYO_ITERM_DIR), so nothing here goes near the real iTerm2 configuration.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

iterm_tests() {
  local out file=$ITERM_DIR/gensokyo.json
  t "the profile template renders into a file iTerm2 can read, and carries nobody's home directory"
  if ! command -v plutil >/dev/null 2>&1; then skip "no plutil (macOS only)"; return; fi
  rm -rf "$ITERM_DIR"; mkdir -p "$ITERM_DIR"
  # The shipped template itself, since that is the artifact: both components and the one Guid.
  out=$(cat "$SHARE/iterm2/gensokyo.json")
  assert_match "$out" '\(session.tmuxStatusLeft)'
  assert_match "$out" '\(session.tmuxStatusRight)'
  assert_match "$out" "$ITERM_GUID"
  iterm_render 'Grand & Old "profile"' > "$scratch/rendered.json"
  assert_ok iterm_parses "$scratch/rendered.json"
  out=$(cat "$scratch/rendered.json")
  assert_match "$out" '\(session.tmuxStatusLeft)'
  assert_match "$out" '\(session.tmuxStatusRight)'
  assert_nomatch "$out" PARENT_PLACEHOLDER
  assert_nomatch "$out" /Users/
  assert_nomatch "$out" "$HOME"
  # The parent is the one thing filled in, and an & or a quote in a profile name must survive
  # both sed's replacement and JSON's string; plutil reads it back as it was written.
  assert_eq "$(plutil -extract 'Profiles.0.Dynamic Profile Parent Name' raw -o - "$scratch/rendered.json")" 'Grand & Old "profile"'
  assert_eq "$(plutil -extract 'Profiles.0.Name' raw -o - "$scratch/rendered.json")" gensokyo
  assert_eq "$(plutil -extract 'Profiles.0.Show Status Bar' raw -o - "$scratch/rendered.json")" true
  assert_eq "$(iterm_guid_of "$scratch/rendered.json")" "$ITERM_GUID"

  t "iterm setup writes the profile, says which profile it inherits, and runs again unchanged"
  out=$(cmd_iterm setup 2>&1)
  assert_match "$out" "wrote $file"
  assert_re "$out" 'inheriting "[^"]+"'
  assert_eq "$(iterm_guid_of "$file")" "$ITERM_GUID"
  assert_ok iterm_parses "$file"
  assert_eq "$(cmd_iterm setup >/dev/null 2>&1; iterm_guid_of "$file")" "$ITERM_GUID"
  assert_eq "$(ls "$ITERM_DIR")" gensokyo.json   # no half-written temp file left behind
  assert_match "$(cmd_doctor)" "profile    installed:"

  t "a profile file gensokyo did not write is never overwritten or removed"
  printf '{"Profiles":[{"Name":"mine","Guid":"someone-else"}]}\n' > "$file"
  assert_fails iterm_is_ours "$file"
  # These say no by dying, so each is run in a subshell of its own and read for its reason.
  if out=$(cmd_iterm setup 2>&1); then bad "setup should refuse"; else assert_match "$out" "another Guid"; fi
  if out=$(cmd_iterm remove 2>&1); then bad "remove should refuse"; else assert_match "$out" "leaving it alone"; fi
  assert_eq "$(iterm_guid_of "$file")" someone-else
  assert_match "$(cmd_doctor)" "has another Guid"
  printf 'not a profile at all\n' > "$file"       # unreadable counts as somebody else's
  assert_fails iterm_is_ours "$file"

  t "iterm remove takes gensokyo's own file back out, and says so when there is none"
  rm -f "$file"; cmd_iterm setup >/dev/null 2>&1
  assert_match "$(cmd_iterm remove)" "removed $file"
  assert_eq "$(ls "$ITERM_DIR")" ''
  assert_match "$(cmd_iterm remove)" "nothing to remove"
  if out=$(cmd_iterm 2>&1); then bad "iterm alone should refuse"; else assert_match "$out" "usage: gensokyo iterm"; fi
  if out=$(cmd_iterm frobnicate 2>&1); then bad "an unknown subcommand should refuse"; else assert_match "$out" "usage: gensokyo iterm"; fi
  assert_match "$(cmd_help)" "gensokyo iterm"

  t "doctor reports iTerm2's tmux window setting without ever writing it"
  assert_match "$(iterm_tabs_line 2)" 'tabs in one window'
  assert_nomatch "$(iterm_tabs_line 2)" 'Settings >'
  assert_match "$(iterm_tabs_line '')" 'not set: iTerm2 gives every resident a macOS window of its own'
  assert_match "$(iterm_tabs_line '')" 'When attaching, restore window as'
  assert_match "$(iterm_tabs_line 1)" 'OpenTmuxWindowsIn=1'
  assert_match "$(iterm_tabs_line 1)" 'When attaching, restore window as'
  assert_re "$(cmd_doctor)" 'tmux tabs'
  # Whatever this machine is set to, reading it says one line and changes nothing.
  assert_re "$(iterm_tmux_windows)" '^[0-9]*$'
}
