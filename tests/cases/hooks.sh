# tests/cases/hooks.sh - the hooks Claude Code runs inside a resident, and the notifications they fire.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

# The hooks Claude Code runs inside a resident: payloads piped to `gensokyo _hook`, no tmux
# server (the toast and bell need one; the fake osascript logs the desktop alerts).
payload() { printf '{"session_id":"%s","hook_event_name":"%s"%s}\n' "$1" "$2" "${3:-}"; }
hook() { payload "$@" | "$root/bin/gensokyo" _hook; }
notifications() { cat "$scratch/notify.log" 2>/dev/null; : > "$scratch/notify.log"; }
hook_tests() {
  local id=cafecafe-0000-4000-8000-000000000001 st=$STATE_DIR/status/cafecafe-0000-4000-8000-000000000001 out
  if [ -z "$JQ_BIN" ]; then t "hook tests"; skip "no jq"; return; fi

  t "status_write/status_load: since moves only when pending changes; mode is kept when not given"
  fresh; rm -rf "$STATE_DIR/status"
  status_write "$id" awaits 'permission: Bash' plan
  status_load "$id"; out=$S_since
  assert_eq "$S_pending|$S_detail|$S_mode" 'awaits|permission: Bash|plan'
  assert_re "$out" '^[0-9]+$'
  status_write "$id" awaits 'permission: Edit'
  status_load "$id"
  assert_eq "$S_since|$S_mode|$S_detail" "$out|plan|permission: Edit"
  status_write "$id" '' '' ; status_load "$id"
  assert_eq "$S_pending|$S_mode" '|plan'

  t "launch_settings: the hooks Claude Code merges into the resident, all calling _hook; the status line wrapper with the user's padding"
  out=$(launch_settings "$id" /tmp)
  assert_eq "$(printf '%s' "$out" | jq_ -r '.hooks | keys | join(",")')" 'Notification,PostToolUse,PreToolUse,Stop,UserPromptSubmit'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.hooks.PreToolUse[0].matcher, .hooks.PostToolUse[0].matcher')" 'AskUserQuestion
AskUserQuestion'
  assert_eq "$(printf '%s' "$out" | jq_ -r '[.hooks[][].hooks[].command] | unique | .[]')" "'$SELF' _hook"
  assert_eq "$(printf '%s' "$out" | jq_ -r '.statusLine | "\(.type) \(.command) \(.padding)"')" "command '$SELF' _statusline '$id' 1"
  out=$(CLAUDE_DIR=$scratch/cc-empty launch_settings "$id" /tmp)
  assert_eq "$(printf '%s' "$out" | jq_ -c '.statusLine | keys')" '["command","type"]'

  t "_hook: Stop marks the turn as awaiting you and notifies once; UserPromptSubmit clears and records the mode"
  fresh; rm -rf "$STATE_DIR/status"; : > "$scratch/notify.log"
  rec "$id" slot=1 name=Reimu cwd=/tmp pane=%9 window=@1
  hook "$id" Stop ',"permission_mode":"default","last_assistant_message":"pong\nsecond line"'
  assert_eq "$(cut -d= -f1,2 "$st" | grep -v since | tr '\n' ' ')" 'pending=stopped detail=pong mode=default '
  assert_eq "$(notifications)" 'Reimu|Reimu is done: pong'
  hook "$id" Stop ',"permission_mode":"default","last_assistant_message":"pong"'
  assert_eq "$(notifications)" ''             # same waiting period: no second alert
  hook "$id" UserPromptSubmit ',"permission_mode":"plan","prompt":"go on"'
  assert_eq "$(cut -d= -f1,2 "$st" | grep -v since | tr '\n' ' ')" 'pending= detail= mode=plan '
  assert_eq "$(notifications)" ''

  t "_hook: permission prompt, question dialog open/close, idle_prompt as a late backup"
  hook "$id" Notification ',"notification_type":"permission_prompt","message":"Claude needs your permission","tool_name":"Bash"'
  assert_eq "$(rec_get "$st" pending)|$(rec_get "$st" detail)|$(rec_get "$st" mode)" 'awaits|permission: Bash|plan'
  assert_eq "$(notifications)" 'Reimu|Reimu needs your permission (Bash)'
  hook "$id" Notification ',"notification_type":"permission_prompt","message":"Claude needs your permission"'
  assert_eq "$(rec_get "$st" detail)" ''
  assert_eq "$(notifications)" ''
  hook "$id" PreToolUse ',"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Delete the branch | yes/no?\nreally","header":"Branch"}]}'
  assert_eq "$(rec_get "$st" pending)|$(rec_get "$st" detail)" 'question|Delete the branch   yes/no? really'
  assert_eq "$(notifications)" 'Reimu|Reimu asks: Delete the branch   yes/no? really'
  hook "$id" PostToolUse ',"tool_name":"AskUserQuestion","tool_response":{}'
  assert_eq "$(rec_get "$st" pending)" ''
  hook "$id" Notification ',"notification_type":"idle_prompt","message":"Claude is waiting for your input"'
  assert_eq "$(rec_get "$st" pending)" stopped
  assert_eq "$(notifications)" 'Reimu|Reimu is done'
  hook "$id" Notification ',"notification_type":"idle_prompt","message":"Claude is waiting for your input"'
  assert_eq "$(notifications)" ''
  hook "$id" PreToolUse ',"tool_name":"Bash","tool_input":{"command":"ls"}'
  assert_eq "$(rec_get "$st" pending)" stopped   # only AskUserQuestion matters

  t "_hook: unknown sessions and garbage are ignored, stdout stays empty, exit 0"
  out=$(hook dead-beef Stop ',"last_assistant_message":"x"' 2>&1; echo "rc=$?")
  assert_eq "$out" 'rc=0'
  assert_ok test ! -f "$STATE_DIR/status/dead-beef"
  out=$(printf 'not json' | "$root/bin/gensokyo" _hook 2>&1; echo "rc=$?")
  assert_eq "$out" 'rc=0'

  t "_hook: NOTIFY_DESKTOP=off silences the desktop alert; the state still changes"
  printf 'NOTIFY_DESKTOP=off\n' > "$CONFIG_DIR/config"
  hook "$id" UserPromptSubmit ',"permission_mode":"default"'
  hook "$id" Stop ',"permission_mode":"default","last_assistant_message":"quiet"'
  assert_eq "$(rec_get "$st" pending)" stopped
  assert_eq "$(notifications)" ''
  rm -f "$CONFIG_DIR/config"

  t "the two suppression rules, decided without a client attached"
  assert_ok    watched_cc yes %3 %3 '✳ Reimu' '✳ Reimu'    # in front, that tab, that pane
  assert_fails watched_cc no  %3 %3 '✳ Reimu' '✳ Reimu'    # iTerm2 is behind another application
  assert_fails watched_cc yes %4 %3 '✳ Reimu' '✳ Reimu'    # in front, but on another resident
  assert_fails watched_cc yes %3 %3 '◑ notes.md' '✳ Reimu' # an iTerm2 tab that is not the cockpit
  assert_fails watched_cc yes %3 %3 '' '✳ Reimu'           # iTerm2 would not say which tab
  assert_fails watched_cc yes %3 %3 '✳ Reimu' ''           # the pane has no title to match
  assert_fails watched_cc yes '' '' '✳ Reimu' '✳ Reimu'    # no pane to be on
  assert_ok    watched_tty 1000 1005
  assert_ok    watched_tty 1000 1010
  assert_fails watched_tty 1000 1011    # touched too long ago
  assert_fails watched_tty '' 1000      # a client that never reported
  assert_fails watched_tty 1005 1000    # the arguments the wrong way round, not "always watched"

  t "iterm_frontmost: yes or no and nothing else, whatever the machine answers"
  assert_re "$(iterm_frontmost)" '^(yes|no)$'

  fresh; rm -rf "$STATE_DIR/status"
}
