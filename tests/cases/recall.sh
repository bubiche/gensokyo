# tests/cases/recall.sh - departed residents: the rows, the transcript and what resume refuses.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

recall_tests() {
  local out id1=aaaaaaaa-1111-4000-8000-000000000001 id2=bbbbbbbb-2222-4000-8000-000000000002 id3=cccccccc-3333-4000-8000-000000000003
  local was_tmux was_start was_pane
  # Half of these rows are departed residents *in a pane*, and a unit test has no tmux server to
  # hold one, so which panes are live is answered here. It is not scenery: it is the question
  # recall_rows asks of every record, and the same fixture with nothing live is the state after
  # `gensokyo quit`. The real functions come back at the end - the suite runs on in this shell.
  was_tmux=$(declare -f tmux_); was_start=$(declare -f start_server); was_pane=$(declare -f open_pane)
  tmux_() { case $1 in list-panes) printf '%%1\n%%2\n' ;; esac; return 0; }

  t "recall_rows: departed residents in panes and archived records, newest first, transcript found by id"
  fresh; rm -rf "$DEPARTED_DIR"; mkdir -p "$DEPARTED_DIR" "$scratch/cc/projects/-tmp-a" "$scratch/cc/projects/-tmp-b"
  rec $id1 slot=1 name=Reimu cwd=/tmp/a pane=%1 window=@1                      # here: not listed
  rec $id2 slot=2 name=Youmu cwd=/tmp/a pane=%2 window=@1 departed=1700000300   # departed screen in its pane
  printf 'slot=3\nname=Sakuya\ncwd=/tmp/b\nwindow=@1\nlaunched=1700000000\ndeparted=1700000200\nexit=0\n' > "$DEPARTED_DIR/$id3"
  : > "$scratch/cc/projects/-tmp-b/$id3.jsonl"; touch -t 202001010000 "$scratch/cc/projects/-tmp-b/$id3.jsonl"   # older than Youmu
  assert_eq "$(recall_rows | cut -d'|' -f2-5)" "$id2|Youmu|/tmp/a|2
$id3|Sakuya|/tmp/b|"
  assert_eq "$(recall_rows | sed -n 1p | cut -d'|' -f1,6)" '1700000300|'
  assert_eq "$(recall_rows | sed -n 2p | cut -d'|' -f6)" "$scratch/cc/projects/-tmp-b/$id3.jsonl"

  t "find_departed: name in any case, else an id prefix of 4+ characters; only archived records"
  assert_eq "$(find_departed sakuya)" "$DEPARTED_DIR/$id3"
  assert_eq "$(find_departed CCCC)" "$DEPARTED_DIR/$id3"
  assert_fails find_departed youmu   # departed, but still in its pane: find_resident's business
  assert_fails find_departed ccc
  assert_fails find_departed Nobody

  t "resume alone lists who can be recalled, --json the same for tools; wrong names and options fail"
  out=$(cmd_resume)
  assert_re "$out" '^  Youmu +/tmp/a +bbbbbbbb +[0-9]+[a-z]+ ago · still in its pane \(slot 2\) · no transcript$'
  assert_re "$out" '^  Sakuya +/tmp/b +cccccccc +[0-9]+[a-z]+ ago$'
  assert_match "$out" 'gensokyo resume <name|session id>'
  assert_eq "$(cmd_resume --json | jq_ -r '.[] | "\(.name) \(.slot) \(.in_pane) \(.transcript) \(.departed_at)"')" "Youmu 2 true false 1700000300
Sakuya null false true $(mtime_of "$scratch/cc/projects/-tmp-b/$id3.jsonl")"
  assert_match "$("$root/bin/gensokyo" resume Nobody 2>&1)" "nobody called 'Nobody' has departed"
  assert_match "$("$root/bin/gensokyo" resume reimu 2>&1)" 'Reimu is still here (slot 1)'
  assert_match "$("$root/bin/gensokyo" recall --bogus 2>&1)" 'unknown option --bogus'
  assert_match "$("$root/bin/gensokyo" resume a b 2>&1)" 'one resident at a time'

  t "recall: a resident whose pane went with the server is offered as an earlier run, not in place"
  # `gensokyo quit` leaves the records exactly as they are, pane ids and all, so that the next
  # cockpit can offer them back. Nothing in a record says its server has gone; only the pane can.
  tmux_() { return 1; }   # no server, so nothing is live
  out=$(cmd_resume)
  assert_nomatch "$out" 'still in its pane'
  assert_re "$out" '^  Youmu +/tmp/a +bbbbbbbb +[0-9]+[a-z]+ ago · no transcript$'
  assert_eq "$(cmd_resume --json | jq_ -r 'map(select(.name == "Youmu")) | .[0] | "\(.slot) \(.in_pane)"')" 'null false'
  # And recalling it archives the record on the way to a fresh pane instead of sending `r` into
  # a pane that is not there and reporting success. The cockpit itself is stubbed out.
  mkdir -p "$scratch/work/youmu"; rec_set "$RES_DIR/$id2" cwd "$scratch/work/youmu"
  start_server() { :; }
  open_pane() { printf '%s\n' "$2" > "$scratch/title.out"; printf '%%9\n'; }
  out=$(cmd_resume Youmu 2>&1)
  assert_match "$out" "recalled Youmu (slot 2) in $scratch/work/youmu"
  assert_nomatch "$out" 'into its pane'
  assert_ok test -f "$RES_DIR/$id2"
  assert_ok test ! -f "$DEPARTED_DIR/$id2"
  assert_eq "$(rec_get "$RES_DIR/$id2" resume)|$(rec_get "$RES_DIR/$id2" slot)|$(rec_get "$RES_DIR/$id2" departed)|$(rec_get "$RES_DIR/$id2" pane)" '1|2||'
  # The window title and the remembered directory are Youmu's, not the last record read by the
  # `another X is here already` guard on its way past - rec_load's R_* are globals, and Reimu
  # is still in RES_DIR to clobber them with.
  assert_match "$(cat "$scratch/title.out")" Youmu
  assert_eq "$(sed -n 1p "$STATE_DIR/recent-dirs")" "$scratch/work/youmu"

  eval "$was_tmux"; eval "$was_start"; eval "$was_pane"
  fresh; rm -rf "$DEPARTED_DIR" "$scratch/cc/projects"
  assert_match "$(cmd_resume)" 'nobody has departed'
}
