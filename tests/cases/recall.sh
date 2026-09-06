# tests/cases/recall.sh - departed residents: the rows, the transcript and what resume refuses.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

recall_tests() {
  local out id1=aaaaaaaa-1111-4000-8000-000000000001 id2=bbbbbbbb-2222-4000-8000-000000000002 id3=cccccccc-3333-4000-8000-000000000003

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
  fresh; rm -rf "$DEPARTED_DIR" "$scratch/cc/projects"
  assert_match "$(cmd_resume)" 'nobody has departed'
}
