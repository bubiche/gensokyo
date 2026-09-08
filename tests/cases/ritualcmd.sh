# tests/cases/ritualcmd.sh - the `gensokyo ritual` commands (lib/ritualcmd.sh): the file `add`
# writes, the line `enable` changes, the run `run` starts by hand, and what the listing says.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call. The times are fixed epochs in the runner's fixed TZ.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

ritual_cmd_tests() {
  local mine=$CONFIG_DIR/rituals out rc mon before id f
  local was_share was_share_all was_now was_start was_pane was_visual was_srv was_live was_tmux
  mkdir -p "$mine"; rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"

  t "the rituals gensokyo ships: each one loads, each is paused, and no example is a surprise"
  assert_eq "$(cd "$SHARE/rituals" && ls -- *.md | tr '\n' ' ')" 'inbox-zero.md nightly-checks.md slack-morning.md '
  out=''
  for f in "$SHARE"/rituals/*.md; do
    ritual_load "$f" || { out="$out cannot-read:${f##*/}"; continue; }
    # Paused is the one that matters: an example that fired on a fresh install would run
    # somebody's morning into a directory they never named.
    ritual_enabled && out="$out fires-on-a-fresh-install:$RIT_slug"
    [ "$RIT_slug.md" = "${f##*/}" ] || out="$out named-something-else:${f##*/}"
    # Its `cwd` is a directory on nobody's machine on purpose, and that is the only thing an
    # example is allowed to be wrong about: everything else would be shipped broken.
    case $(ritual_problem) in
      ''|'cwd: '*) ;;
      *) out="$out $RIT_slug:$(ritual_problem)" ;;
    esac
  done
  assert_eq "$out" ''

  t "and a shipped one's problem line is what to do about it, not a complaint it cannot act on"
  assert_eq "$(rit_problem_line 'cwd: ~/dev/yourproject is not a directory' "$SHARE/rituals/slack-morning.md")" \
    'an example: enable or edit it, and gensokyo copies it to your own rituals first'
  # Anything else wrong with a shipped file is still said: the line is about the cwd, not about
  # where the file lives.
  assert_eq "$(rit_problem_line 'schedule: 9 5 * *' "$SHARE/rituals/slack-morning.md")" '! schedule: 9 5 * *'
  assert_eq "$(rit_problem_line 'cwd: gone' "$mine/whatever.md")" '! cwd: gone'

  # From here on the examples are out of the way: these tests are about the user's own rituals,
  # and a fourth example added later must not fail an assertion about what is scheduled.
  was_share_all=$SHARE
  SHARE=$scratch/share-none; mkdir -p "$SHARE/rituals"
  # No .claude.json at all, which is the answer "nothing can be said about trust" - so the tests
  # that are not about the trust dialog are not about the trust dialog (lib/hooks.sh).
  rm -f "$CLAUDE_JSON"
  was_now=${GENSOKYO_NOW:-}
  mon=$(rt_at '2026-09-07 09:05:00')   # a Monday morning, the Slack example's own hour
  GENSOKYO_NOW=$mon

  t "ritual add: the file it writes is the file the reader gets back"
  out=$(cmd_ritual add --name slack-morning --schedule '3 9 * * 1-5' --cwd "$scratch" \
    --description 'Morning Slack triage (before 9:30 #urgent)' --model haiku --effort low \
    --mode acceptEdits --allowed-tools 'Read, Grep' --allowed-tools 'Bash(npm run test:*)' \
    --allowed-tools 'mcp__claude_ai_Slack__*' --prompt 'Check Slack and summarize.' 2>&1)
  assert_match "$out" "wrote $(tilde "$mine/slack-morning.md")"
  assert_match "$out" 'next fire  2026-09-08 09:03 (in 23h)'
  # The clock the five fields are read on, said out loud. A resident that has been told to convert
  # a local time to UTC writes `3 1 * * 1-5` and reports back 09:03; this line is the only thing in
  # the exchange that contradicts it, so it prints whether or not the ritual is on.
  assert_match "$out" "3 9 * * 1-5 is this machine's clock, now "
  assert_match "$out" 'a ritual is never in UTC'
  ritual_load "$mine/slack-morning.md"
  assert_eq "$RIT_slug|$RIT_name|$RIT_schedule|$RIT_cwd" "slack-morning|slack-morning|3 9 * * 1-5|$scratch"
  # A description with a colon and a `#` in it: written quoted, because a bare value loses
  # everything from the ` #` on (lib/rituals.sh).
  assert_eq "$RIT_description" 'Morning Slack triage (before 9:30 #urgent)'
  assert_eq "$RIT_model|$RIT_effort|$RIT_mode" 'haiku|low|acceptEdits'
  # Said three times, once with a comma in the middle, and one of them a pattern with spaces and
  # a comma of its own - which is why the file gets the block form and not an inline list.
  assert_eq "$RIT_allowed" 'Read
Grep
Bash(npm run test:*)
mcp__claude_ai_Slack__*'
  assert_eq "$RIT_prompt" 'Check Slack and summarize.'
  assert_eq "$(ritual_problem)" ''
  # Through ritual_args as well, since that is the form the record carries and the pane eval's
  # back apart: the pattern with spaces in it is one argument on the far side (lib/ritualrun.sh).
  assert_eq "$(rr_argc "$(ritual_args)")" 13
  assert_eq "$(rr_argv 12 "$(ritual_args)")" 'Bash(npm run test:*)'
  assert_eq "$(rr_argv 13 "$(ritual_args)")" 'mcp__claude_ai_Slack__*'

  t "ritual add: --headless writes the line, and says there will be no tab to watch"
  out=$(cmd_ritual add --name quiet-add --schedule '@daily' --cwd "$scratch" --headless \
    --allowed-tools Read --prompt 'summarize and stop' 2>&1)
  assert_match "$out" 'headless: no pane to watch and nobody to answer a prompt'
  assert_match "$out" "$(tilde "$STATE_DIR/rituals/quiet-add/runs")"
  ritual_load "$mine/quiet-add.md"
  assert_eq "$RIT_headless" true
  assert_eq "$(ritual_problem)" ''
  assert_eq "$(ritual_cmd_rows | grep '^quiet-add|' | cut -d'|' -f8)" true
  # And the plain listing says it too, since that ritual's fire is the one that shows nothing.
  assert_match "$(cmd_ritual list 2>&1)" 'headless: no pane, and its own log of what each run said'
  rm -f "$mine/quiet-add.md"

  t "ritual add: the prompt comes whole, from a file or from a pipe"
  printf 'first line\n\nthird line\n' > "$scratch/rit-prompt.txt"
  cmd_ritual add --name from-file --schedule '@daily' --cwd "$scratch" \
    --prompt-file "$scratch/rit-prompt.txt" >/dev/null 2>&1
  ritual_load "$mine/from-file.md"
  assert_eq "$RIT_prompt" 'first line

third line'
  printf 'from a pipe\nand a second line\n' | cmd_ritual add --name from-stdin --schedule '@daily' \
    --cwd "$scratch" --prompt-file - >/dev/null 2>&1
  ritual_load "$mine/from-stdin.md"
  assert_eq "$RIT_prompt" 'from a pipe
and a second line'
  assert_match "$("$root/bin/gensokyo" ritual add --name no-prompt --schedule '@daily' 2>&1)" '--prompt-file <file>'
  assert_match "$("$root/bin/gensokyo" ritual add --name no-file --schedule '@daily' --prompt-file /no/such/file 2>&1)" 'no such prompt file'
  assert_fails test -f "$mine/no-prompt.md"

  t "ritual add: a relative directory is resolved where it was typed, since a run starts nowhere"
  ( cd "$scratch" && cmd_ritual add --name relative-cwd --schedule '@daily' --cwd . --prompt x ) >/dev/null 2>&1
  ritual_load "$mine/relative-cwd.md"
  assert_eq "$RIT_cwd" "$scratch"

  t "ritual add: a name that is taken is refused, and the file it would have replaced is untouched"
  before=$(cksum < "$mine/slack-morning.md")
  out=$("$root/bin/gensokyo" ritual add --name slack-morning --schedule '@daily' --prompt x 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'slack-morning is already there'
  assert_eq "$(cksum < "$mine/slack-morning.md")" "$before"
  assert_match "$("$root/bin/gensokyo" ritual add --name 'not a name' --schedule '@daily' --prompt x 2>&1)" 'cannot be a ritual name'
  assert_match "$("$root/bin/gensokyo" ritual add --name odd --schedule 'nonsense' --prompt x 2>&1)" 'is not one gensokyo can read'
  assert_match "$("$root/bin/gensokyo" ritual add --name odd --schedule '0 0 30 2 *' --prompt x 2>&1)" 'never comes round'
  assert_match "$("$root/bin/gensokyo" ritual add --name odd --schedule '@daily' --prompt x --bogus 2>&1)" 'unknown option --bogus'
  assert_fails test -f "$mine/odd.md"

  t "ritual add: a line that could never fire takes the file with it; a cwd nobody can fix from here does not"
  # The two halves of what `add` does about a problem. A target that names no resident is a file
  # that will not run on any day and there is no next fire to print, so nothing is left behind.
  out=$("$root/bin/gensokyo" ritual add --name unwired --schedule '@daily' --cwd "$scratch" \
    --target 3 --prompt x 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" "target: 3 is neither new, persistent, nor a resident's name"
  assert_match "$out" 'nothing was written'
  assert_fails test -f "$mine/unwired.md"
  # A directory that is not there yet, or that nobody has answered Claude Code's trust prompt
  # for, is a file that is right about everything it can be right about: the fix is outside it,
  # and the message says what the fix is - so it is written, said, and left with the user.
  out=$("$root/bin/gensokyo" ritual add --name no-dir --schedule '@daily' --cwd /no/such/place --prompt x 2>&1); rc=$?
  assert_eq "$rc" 0
  assert_match "$out" '/no/such/place is not a directory'
  assert_match "$out" 'next fire'
  assert_ok test -f "$mine/no-dir.md"
  mkdir -p "$scratch/untrusted"
  printf '{"projects":{"%s":{"hasTrustDialogAccepted":false}}}\n' "$scratch/untrusted" > "$CLAUDE_JSON"
  out=$("$root/bin/gensokyo" ritual add --name untrusted-one --schedule '@daily' \
    --cwd "$scratch/untrusted" --prompt x 2>&1); rc=$?
  assert_eq "$rc" 0
  assert_match "$out" "trust prompt for $(tilde "$scratch/untrusted")"
  assert_match "$out" 'next fire'
  assert_ok test -f "$mine/untrusted-one.md"
  rm -f "$CLAUDE_JSON"; rm -f "$mine/untrusted-one.md"; rmdir "$scratch/untrusted"
  # A value that would need both kinds of quote around it cannot be written so that the reader
  # gets it back, and that is said before anything is written rather than mangled.
  out=$("$root/bin/gensokyo" ritual add --name quoted --schedule '@daily' \
    --description 'a "quoted" word and an apostrophe'"'" --prompt x 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'cannot hold both kinds of quote'
  assert_fails test -f "$mine/quoted.md"
  # One of each kind on its own, both of which have to survive: the value is written inside the
  # quote it does not contain, and a value that *starts* with the other kind is the case worth
  # checking rather than reasoning about (rit_value strips only a matching pair).
  cmd_ritual add --name quoted-one --schedule '@daily' --cwd "$scratch" \
    --description "it's a morning job" --prompt x >/dev/null 2>&1
  ritual_load "$mine/quoted-one.md"; assert_eq "$RIT_description" "it's a morning job"
  cmd_ritual add --name quoted-two --schedule '@daily' --cwd "$scratch" \
    --description 'say "yes" first' --prompt x >/dev/null 2>&1
  ritual_load "$mine/quoted-two.md"; assert_eq "$RIT_description" 'say "yes" first'
  cmd_ritual add --name quoted-three --schedule '@daily' --cwd "$scratch" \
    --description "'twas already a job" --prompt x >/dev/null 2>&1
  ritual_load "$mine/quoted-three.md"; assert_eq "$RIT_description" "'twas already a job"
  rm -f "$mine/quoted-one.md" "$mine/quoted-two.md" "$mine/quoted-three.md"

  t "ritual enable / disable: the line where it is, and added inside the fence where it is not"
  assert_nomatch "$(cat "$mine/slack-morning.md")" 'enabled:'
  out=$(cmd_ritual disable slack-morning 2>&1)
  assert_match "$out" 'slack-morning is disabled'
  ritual_load "$mine/slack-morning.md"
  assert_fails ritual_enabled                   # read back through the frontmatter, so it is in it
  assert_eq "$(grep -c '^enabled:' "$mine/slack-morning.md")" 1
  assert_eq "$RIT_prompt" 'Check Slack and summarize.'   # and the prompt is still the prompt
  out=$(cmd_ritual enable slack 2>&1)            # a part of the name is enough, as everywhere
  assert_match "$out" 'slack-morning is enabled'
  assert_match "$out" 'next fire  2026-09-08 09:03'
  ritual_load "$mine/slack-morning.md"
  assert_ok ritual_enabled
  assert_eq "$(grep -c '^enabled:' "$mine/slack-morning.md")" 1   # replaced, not written twice
  assert_match "$("$root/bin/gensokyo" ritual disable nothing-of-the-sort 2>&1)" "no ritual called 'nothing-of-the-sort'"
  assert_match "$("$root/bin/gensokyo" ritual enable a b 2>&1)" 'one ritual at a time'

  t "ritual disable: a file with no frontmatter has no schedule to turn off, and is told so"
  printf 'all prompt, no frontmatter\n' > "$mine/bare-cmd.md"
  out=$("$root/bin/gensokyo" ritual disable bare-cmd 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'has no frontmatter'
  assert_eq "$(cat "$mine/bare-cmd.md")" 'all prompt, no frontmatter'
  assert_eq "$(find "$mine" -name '*.tmp.*' | wc -l | tr -d ' ')" 0   # and nothing half-written left behind
  rm -f "$mine/bare-cmd.md"

  t "ritual enable: a shipped example is copied into your own rituals first, and the copy changes"
  was_share=$SHARE
  SHARE=$scratch/share-cmd; mkdir -p "$SHARE/rituals"
  printf -- '---\nschedule: "@daily"\ncwd: %s\nenabled: false\n---\nthe shipped one\n' "$scratch" \
    > "$SHARE/rituals/example-run.md"
  out=$(cmd_ritual enable example-run 2>&1)
  assert_match "$out" "$(tilde "$mine/example-run.md")"
  assert_match "$(cat "$SHARE/rituals/example-run.md")" 'enabled: false'   # the install tree, untouched
  assert_match "$(cat "$mine/example-run.md")" 'enabled: true'
  assert_eq "$(find_ritual example-run)" "$mine/example-run.md"            # and yours is the one that fires
  assert_eq "$(ritual_dir example-run)" "$STATE_DIR/rituals/example-run"   # keyed by name, so its notes carry over
  SHARE=$was_share; rm -rf "$scratch/share-cmd"; rm -f "$mine/example-run.md"

  t "ritual run: by hand whatever the schedule says, and never spending the minute it is waiting for"
  fresh; rm -rf "$STATE_DIR/rituals" "$STATE_DIR/status"
  was_start=$(declare -f start_server); was_pane=$(declare -f open_pane)
  # A unit test has no tmux server to open a window in; what is being tested is what the command
  # decides, which is the stamp, the log line and the record. The real functions come back below.
  start_server() { :; }
  open_pane() { printf '%s\n' "$2" > "$scratch/rc-title"; printf '%%5\n'; }
  cmd_ritual disable slack-morning >/dev/null 2>&1
  # Yesterday's minute, deliberately not this one: a stamp that already reads as now would let a
  # run that spends the minute look exactly like one that leaves it alone.
  ritual_stamp_set slack-morning "$((mon - 86400))"
  out=$(cmd_ritual run slack 2>&1)
  assert_match "$out" 'slack-morning is disabled: this run is by hand'
  assert_match "$out" "slack-morning is running in $(tilde "$scratch")"
  assert_match "$out" 'nothing is attached'      # nobody could answer a prompt it stops at
  # The one that matters: a hand run does not stand in for the fire the schedule is waiting for.
  assert_eq "$(ritual_stamp slack-morning)" "$((mon - 86400))"
  assert_match "$(cat "$STATE_DIR/rituals/slack-morning/log")" 'ran (by hand)'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  id=$(ls "$RES_DIR"); rec_load "$RES_DIR/$id"
  assert_eq "$R_ritual|$R_name|$R_cwd" "slack-morning|slack-morning|$scratch"
  assert_match "$(cat "$scratch/rc-title")" 'slack-morning'
  # And a second one, while that one is still going, is refused rather than doubled: both runs
  # would be writing the same notes.
  out=$(cmd_ritual run slack-morning 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'is still running from last time'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" 1
  # A ritual that cannot run is refused with the line that is wrong, before anything is opened.
  out=$(cmd_ritual run no-dir 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" '/no/such/place is not a directory'
  assert_match "$("$root/bin/gensokyo" ritual run nothing-of-the-sort 2>&1)" "no ritual called 'nothing-of-the-sort'"

  t "ritual run: a fire into a resident already there says where it is going, not what it opened"
  # Nothing is reset here: the run above left a record and a journal that the log and list tests
  # below read, and this fire summons nobody, so what it must not do is add one.
  before=$(ls "$RES_DIR" | wc -l | tr -d ' ')
  was_tmux=$(declare -f tmux_)
  tmux_() { printf '%s\n' "$*" >> "$scratch/rc-tmux"; return 0; }
  : > "$scratch/rc-tmux"
  printf -- '---\nschedule: "@daily"\ntarget: Reimu\n---\ntell Reimu\n' > "$mine/at-reimu.md"
  out=$(cmd_ritual run at-reimu 2>&1)
  assert_match "$out" "at-reimu's prompt is on its way to Reimu"
  assert_match "$out" 'gensokyo ritual log at-reimu says where it landed'
  assert_match "$(cat "$scratch/rc-tmux")" '_send at-reimu'
  assert_eq "$(ls "$RES_DIR" | wc -l | tr -d ' ')" "$before"   # nothing is summoned for it
  assert_match "$(cat "$STATE_DIR/rituals/at-reimu/log")" 'ran (by hand)'
  # And a second one is not refused: what would be busy is somebody else's session, and a prompt
  # sent into a turn is Claude Code's to queue.
  out=$(cmd_ritual run at-reimu 2>&1)
  assert_match "$out" "on its way to Reimu"
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\ntarget: persistent\n---\nagain\n' "$scratch" \
    > "$mine/kept-one.md"
  out=$(cmd_ritual run kept-one 2>&1)
  assert_match "$out" "kept-one's prompt is on its way to the session it keeps"
  rm -f "$mine/at-reimu.md" "$mine/kept-one.md"
  eval "$was_tmux"

  eval "$was_start"; eval "$was_pane"
  cmd_ritual enable slack-morning >/dev/null 2>&1

  t "ritual log: the journal, oldest first, and -n for the tail of it"
  ritual_note slack-morning 'skipped (by hand): the last run is still going'
  out=$(cmd_ritual log slack-morning 2>&1)
  assert_match "$out" '2026-09-07 09:05  ran (by hand)'
  assert_match "$out" '2026-09-07 09:05  skipped (by hand): the last run is still going'
  assert_match "$out" "$(tilde "$STATE_DIR/rituals/slack-morning/log")"
  assert_eq "$(cmd_ritual log slack-morning -n 1 | sed -n 1p)" '  2026-09-07 09:05  skipped (by hand): the last run is still going'
  assert_match "$(cmd_ritual log from-file 2>&1)" 'has not done anything yet'
  assert_match "$("$root/bin/gensokyo" ritual log slack-morning -n two 2>&1)" '-n takes a number'
  assert_match "$("$root/bin/gensokyo" ritual log 2>&1)" 'which one?'

  t "ritual list: what fires when, with the line that is wrong under the ritual it belongs to"
  out=$(cmd_ritual list 2>&1)
  assert_match "$out" 'slack-morning'
  assert_match "$out" '3 9 * * 1-5'
  assert_match "$out" '2026-09-08 09:03 (in 23h)'
  assert_match "$out" 'last ran 2026-09-07 09:05 (0s ago)'
  assert_match "$out" '! cwd: /no/such/place is not a directory'
  assert_match "$out" 'gensokyo ritual run <name>'
  # A file whose name could never be a ritual name is named rather than skipped in silence.
  printf -- '---\nschedule: "@daily"\n---\nx\n' > "$mine/not a ritual.md"
  assert_match "$(cmd_ritual list 2>&1)" 'not a usable ritual name'
  rm -f "$mine/not a ritual.md"
  # Alone, `gensokyo ritual` is the listing - which is what "what is scheduled?" asks for.
  assert_eq "$(cmd_ritual)" "$(cmd_ritual list)"
  out=$("$root/bin/gensokyo" ritual bogus 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" "nothing called 'bogus' to do"
  assert_match "$out" 'usage: gensokyo ritual'
  assert_match "$("$root/bin/gensokyo" schedule 2>&1)" 'slack-morning'   # the plain spelling, still there
  assert_match "$("$root/bin/gensokyo" ritual list --bogus 2>&1)" 'unknown option --bogus'

  t "ritual list --json: what the skill reads, and a schedule that never comes round has no next fire"
  printf -- '---\nschedule: "0 0 30 2 *"\ncwd: %s\n---\nwork\n' "$scratch" > "$mine/never-again.md"
  out=$(cmd_ritual list --json)
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[] | select(.name == "slack-morning") |
    "\(.enabled) \(.schedule) \(.next_fire) \(.next_fire_local) \(.last_run) \(.cwd) \(.target) \(.headless) \(.problem)"')" \
    "true 3 9 * * 1-5 $(cron_next '3 9 * * 1-5' "$mon") 2026-09-08 09:03 $mon $scratch new false null"
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[] | select(.name == "never-again") | "\(.next_fire) \(.next_fire_local)"')" 'null null'
  assert_match "$(printf '%s' "$out" | jq_ -r '.[] | select(.name == "never-again") | .problem')" 'never comes round'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[] | select(.name == "from-file") | .description')" null
  # "last ran" comes from the log and never from the stamp. The sweep stamps a ritual the first
  # time it sees one, so a ritual that has been seen and has never run has to read as never run;
  # and once it has run, the minute the log says is the one, not the minute the stamp spent.
  ritual_stamp_set from-file "$mon"
  ritual_note from-file 'not run: cwd: /gone is not a directory'
  assert_eq "$(ritual_last_run from-file)" ''
  assert_eq "$(cmd_ritual list --json | jq_ -r '.[] | select(.name == "from-file") | .last_run')" null
  ritual_stamp_set from-file "$((mon + 3600))"
  GENSOKYO_NOW=$((mon + 120)); ritual_note from-file 'ran (due 2026-09-07 09:07)'; GENSOKYO_NOW=$mon
  assert_eq "$(ritual_last_run from-file)" "$((mon + 120))"
  assert_eq "$(cmd_ritual list --json | jq_ -r '.[] | select(.name == "from-file") | .last_run')" "$((mon + 120))"
  assert_eq "$(printf '%s' "$out" | jq_ -r 'map(.name) | join(" ")')" 'from-file from-stdin never-again no-dir relative-cwd slack-morning'
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[] | select(.name == "slack-morning") | .path')" "$mine/slack-morning.md"
  # The tab's life is in there as the ritual would have to write it, default and all: a resident
  # asked why a run's tab went away has nowhere else to read it.
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[] | select(.name == "slack-morning") | .keep')" 2h
  # The shape itself, not just the values: this is what the skill will read, and a field renamed
  # later would otherwise break it with every assertion above still passing.
  assert_eq "$(printf '%s' "$out" | jq_ -r '.[0] | keys_unsorted | join(",")')" \
    'name,enabled,schedule,next_fire,next_fire_local,last_run,target,headless,keep,overlap,cwd,description,problem,path'
  rm -f "$mine/never-again.md"
  # With nothing scheduled at all it is still a list, because a tool reading it should not have
  # to tell "none" from "something went wrong".
  rm -f "$mine"/*.md
  assert_eq "$(cmd_ritual list --json)" '[]'
  assert_match "$(cmd_ritual list 2>&1)" 'nothing is scheduled yet'

  t "ritual new: a template that reads back as a ritual, disabled until its author says otherwise"
  # $EDITOR is `:` for the whole suite (tests/run.sh), so this writes the file and opens nothing.
  out=$(cmd_ritual new fresh-one 2>&1)
  assert_match "$out" "wrote $(tilde "$mine/fresh-one.md")"
  assert_match "$out" 'disabled, so it will not fire'
  ritual_load "$mine/fresh-one.md"
  assert_eq "$RIT_slug|$RIT_name" 'fresh-one|fresh-one'
  assert_fails ritual_enabled
  assert_eq "$RIT_unknown" ''            # every line in it, commented or not, is a key gensokyo knows
  assert_eq "$(ritual_problem)" ''       # and it is a ritual as it stands, but for being off
  assert_match "$RIT_prompt" 'What the resident should do'
  assert_match "$("$root/bin/gensokyo" ritual new fresh-one 2>&1)" 'already there'
  assert_match "$("$root/bin/gensokyo" ritual new 'not a name' 2>&1)" 'cannot be a ritual name'
  assert_match "$("$root/bin/gensokyo" ritual new 2>&1)" 'a name for it'

  t "ritual edit: the file in whatever editor the environment names, and its own report afterwards"
  out=$(cmd_ritual edit fresh-one 2>&1)
  assert_match "$out" 'disabled, so it will not fire'
  cmd_ritual enable fresh-one >/dev/null 2>&1
  assert_match "$(cmd_ritual edit fresh 2>&1)" 'next fire'
  was_visual=${VISUAL:-}
  VISUAL=/no/such/editor
  out=$(cmd_ritual edit fresh-one 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'could not open'
  VISUAL=$was_visual

  t "ritual remove: the file, its notes and its journal, and the whole name before any of it goes"
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\nenabled: true\n---\nwork\n' "$scratch" > "$mine/gone-soon.md"
  ritual_note gone-soon 'ran (by hand)'          # which makes the directory its notes live in
  printf 'what it learned last time\n' > "$(ritual_dir gone-soon)/memory.md"
  # A part of a name reaches a ritual everywhere else and deliberately not here: `run slack`
  # reaching slack-morning saves a word, and `remove slack` reaching it spends a file on a guess.
  out=$("$root/bin/gensokyo" ritual remove gone 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'did you mean gone-soon?'
  assert_ok test -f "$mine/gone-soon.md"
  out=$(cmd_ritual remove gone-soon 2>&1)
  assert_match "$out" "gone-soon is gone, and $(tilde "$mine/gone-soon.md") with it"
  assert_match "$out" "its notes and its journal went too, from $(tilde "$(ritual_dir gone-soon)")"
  assert_fails test -e "$mine/gone-soon.md"
  assert_fails test -e "$(ritual_dir gone-soon)"
  assert_eq "$(find_ritual gone-soon)" ''
  assert_nomatch "$(cmd_ritual list 2>&1)" gone-soon   # and gone out of what is scheduled with it
  # `delete` and `rm` are the same verb: which of the three a person types is not worth a lesson.
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\n---\nwork\n' "$scratch" > "$mine/twice-over.md"
  assert_match "$(cmd_ritual delete twice-over 2>&1)" 'twice-over is gone'
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\n---\nwork\n' "$scratch" > "$mine/twice-over.md"
  assert_match "$(cmd_ritual rm twice-over 2>&1)" 'twice-over is gone'
  assert_fails test -e "$mine/twice-over.md"
  # The one gensokyo cannot read is exactly the one worth deleting, so the name it reports comes
  # off the file name and never out of a frontmatter that may not be there at all.
  printf 'all prompt, no frontmatter\n' > "$mine/bare-gone.md"
  assert_match "$(cmd_ritual remove bare-gone 2>&1)" 'bare-gone is gone'
  assert_fails test -e "$mine/bare-gone.md"
  assert_match "$("$root/bin/gensokyo" ritual remove nothing-of-the-sort 2>&1)" "no ritual called 'nothing-of-the-sort'"
  assert_match "$("$root/bin/gensokyo" ritual remove a b 2>&1)" 'one ritual at a time'
  assert_match "$("$root/bin/gensokyo" ritual remove 2>&1)" 'which one?'

  t "ritual remove: an example gensokyo ships is not the user's to delete, and your copy of one is"
  was_share=$SHARE
  SHARE=$scratch/share-rm; mkdir -p "$SHARE/rituals"
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\nenabled: false\n---\nthe shipped one\n' "$scratch" \
    > "$SHARE/rituals/example-gone.md"
  out=$(cmd_ritual remove example-gone 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'one of the examples gensokyo ships'
  assert_ok test -f "$SHARE/rituals/example-gone.md"
  cmd_ritual disable example-gone >/dev/null 2>&1     # which is what takes the copy
  assert_ok test -f "$mine/example-gone.md"
  out=$(cmd_ritual remove example-gone 2>&1)
  assert_fails test -e "$mine/example-gone.md"
  assert_ok test -f "$SHARE/rituals/example-gone.md"  # the install tree, untouched
  # And the example the copy was shadowing is back in the listing, which is said: a name still
  # there after a delete reads as a delete that did not work, and the next thing tried is another.
  assert_match "$out" 'is in the listing again'
  assert_eq "$(find_ritual example-gone)" "$SHARE/rituals/example-gone.md"
  SHARE=$was_share; rm -rf "$scratch/share-rm"

  t "ritual remove: not while a run of it is going, which is about to write the notes it would take"
  fresh; rm -rf "$STATE_DIR/rituals"
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\nenabled: true\n---\nwork\n' "$scratch" > "$mine/busy-one.md"
  was_start=$(declare -f start_server); was_pane=$(declare -f open_pane)
  was_srv=$(declare -f server_running)
  start_server() { :; }
  open_pane() { printf '%%7\n'; }
  server_running() { :; }        # asked at all only with a cockpit up, so here there is one
  cmd_ritual run busy-one >/dev/null 2>&1
  out=$(cmd_ritual remove busy-one 2>&1); rc=$?
  assert_eq "$rc" 1
  assert_match "$out" 'busy-one is running now'
  assert_ok test -f "$mine/busy-one.md"
  # A run that has finished is a different answer: it waits in its tab until the owner closes
  # it, so it does not stop the delete - but it was started with a prompt naming the notes file
  # that just went, and anything typed there writes the directory back where nothing can see it
  # again. So the delete goes ahead and the tab is named.
  # The pane and the Stop hook by hand: what `_run` writes from inside the pane it lands in,
  # and what the hook writes when the run finishes its turn (lib/residents.sh, lib/hooks.sh).
  id=$(ls "$RES_DIR" | head -n 1)
  rec_set "$RES_DIR/$id" pane '%7'
  status_write "$id" stopped '' ''
  was_live=$(declare -f live_panes)
  live_panes() { printf '%%7\n'; }
  assert_fails ritual_running busy-one        # finished, so it is not a run in progress
  out=$(cmd_ritual remove busy-one 2>&1)
  assert_match "$out" 'busy-one is gone'
  assert_match "$out" 'a run of it is still in a tab: close busy-one'
  assert_fails test -e "$mine/busy-one.md"
  # And with no run of it anywhere, that line is not said at all.
  fresh
  printf -- '---\nschedule: "@daily"\ncwd: "%s"\n---\nwork\n' "$scratch" > "$mine/busy-one.md"
  out=$(cmd_ritual remove busy-one 2>&1)
  assert_match "$out" 'busy-one is gone'
  assert_nomatch "$out" 'still in a tab'
  eval "$was_live"
  eval "$was_start"; eval "$was_pane"; eval "$was_srv"

  rm -f "$mine"/*.md; rm -rf "$STATE_DIR/rituals"
  SHARE=$was_share_all; rm -rf "$scratch/share-none"
  [ -z "$was_now" ] && GENSOKYO_NOW='' || GENSOKYO_NOW=$was_now
  return 0
}
