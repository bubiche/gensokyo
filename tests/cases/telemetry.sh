# tests/cases/telemetry.sh - the status line wrapper and what the bar, border and list make of its digest.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

# The status line wrapper and what the bar, border and list make of its digest.
telemetry_tests() {
  local id=cafecafe-0000-4000-8000-000000000002 id2=cafecafe-0000-4000-8000-000000000003 out now
  if [ -z "$JQ_BIN" ]; then t "telemetry tests"; skip "no jq"; return; fi

  t "git_branch reads .git/HEAD walking up, follows worktree files, shows a detached head short; nothing outside a repo"
  mkdir -p "$scratch/repo/.git/worktrees/wt" "$scratch/repo/sub/dir" "$scratch/wt" "$scratch/norepo"
  echo 'ref: refs/heads/feature/x' > "$scratch/repo/.git/HEAD"
  assert_eq "$(git_branch "$scratch/repo/sub/dir")" feature/x
  echo 'abcdef1234567890' > "$scratch/repo/.git/HEAD"
  assert_eq "$(git_branch "$scratch/repo")" abcdef1
  printf 'gitdir: %s/repo/.git/worktrees/wt\n' "$scratch" > "$scratch/wt/.git"
  echo 'ref: refs/heads/wt-branch' > "$scratch/repo/.git/worktrees/wt/HEAD"
  assert_eq "$(git_branch "$scratch/wt")" wt-branch
  assert_eq "$(git_branch "$scratch/norepo")" ''

  t "formatting: pct_bar, fmt_eta, fmt_age, fmt_cost, mode_label, model_short, tele_fields leaves unknowns out"
  assert_eq "$(pct_bar 0)|$(pct_bar 36)|$(pct_bar 100)|$(pct_bar 250)" '░░░░░░░░░░|▓▓▓░░░░░░░|▓▓▓▓▓▓▓▓▓▓|▓▓▓▓▓▓▓▓▓▓'
  assert_eq "$(fmt_eta 7860) $(fmt_eta 275000) $(fmt_eta 840) $(fmt_eta 0)" '2h11m 3d4h 14m now'
  assert_eq "$(fmt_age 12) $(fmt_age 200) $(fmt_age 7200) $(fmt_age 90000)" '12s 3m 2h 1d'
  assert_eq "$(fmt_cost 0.18978) $(fmt_cost 12)" '$0.19 $12.00'
  assert_eq "$(mode_label acceptEdits)/$(mode_label bypassPermissions)/$(mode_label dontAsk)/$(mode_label plan)" 'accept-edits/bypass/dont-ask/plan'
  assert_eq "$(model_short 'Sonnet 5')|$(model_short Fable)" 'Sonnet|Fable'
  assert_eq "$(tele_fields 'Sonnet 5' 42 high 91 88 0.42 main opus acceptEdits)" 'Sonnet 5→⚖ Opus · high · accept-edits · ⚡91% · $0.42'
  assert_eq "$(fmt_tokens 1000000) $(fmt_tokens 200000) $(fmt_tokens 999)" '1M 200k 999'
  assert_eq "$(tele_fields 'Sonnet 5' 42 high 91 88 0.42 main opus acceptEdits verbose)" 'Sonnet 5→⚖ Opus · ctx 42% · high · accept-edits · ⚡91% (turn 88%) · $0.42 · ⎇ main'
  assert_eq "$(tele_fields Haiku '' '' '' '' '' '' '' '')" 'Haiku'
  assert_eq "$(tele_fields '' '' '' '' '' '' main '' plan)" 'plan'

  t "_statusline: keeps the JSON and a digest, then prints gensokyo's own status line"
  fresh; rm -rf "$TELE_DIR"
  rec "$id" slot=1 name=Reimu cwd=/tmp pane=%9 window=@1
  out=$("$root/bin/gensokyo" _statusline "$id" < "$here/fixtures/statusline.json")
  assert_eq "$out" 'Sonnet 5→⚖ Opus · medium · ░░░░░░░░░░ 5% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m'
  assert_eq "$(jq_ -r .model.display_name "$TELE_DIR/$id.json")" 'Sonnet 5'
  tele_load "$id"
  assert_eq "$T_model|$T_ctx|$T_effort|$T_cache|$T_tcache|$T_cost|$T_five|$T_five_reset|$T_week|$T_week_reset|$T_advisor|$T_added|$T_removed|$T_dur" \
    'Sonnet 5|5|medium|93|99|0.18978259999999997|36|1788543000|49|1788595200|opus|8|0|325'
  assert_re "$T_at" '^[0-9]+$'

  t "_statusline: a sparse report (haiku, API key: no effort, no rate limits, no usage yet) leaves fields empty"
  out=$(jq_ 'del(.effort, .rate_limits, .prompt_cache, .cost) | .context_window.current_usage = null' "$here/fixtures/statusline.json" \
    | "$root/bin/gensokyo" _statusline "$id")
  assert_eq "$out" 'Sonnet 5→⚖ Opus · ░░░░░░░░░░ 5% of 1M'
  tele_load "$id"
  assert_eq "$T_model|$T_ctx|$T_effort|$T_cache|$T_tcache|$T_five|$T_week" 'Sonnet 5|5|||||'
  assert_eq "$(usage_text)" ''

  t "_statusline: STATUSLINE=user runs the user's own status line command with the same JSON; own line when they have none"
  printf 'STATUSLINE=user\n' > "$CONFIG_DIR/config"
  out=$("$root/bin/gensokyo" _statusline "$id" < "$here/fixtures/statusline.json")
  assert_eq "$out" 'USER LINE'
  assert_eq "$(jq_ -r .session_id "$scratch/sl.in")" f2fe56c9-466e-4333-bed0-4a89460dd0b8
  out=$(CLAUDE_CONFIG_DIR=$scratch/cc-empty "$root/bin/gensokyo" _statusline "$id" < "$here/fixtures/statusline.json")
  assert_match "$out" 'Sonnet 5 · medium · ░░░░░░░░░░ 5% of 1M'
  assert_eq "$(rec_get "$TELE_DIR/$id.kv" advisor)" ''
  rm -f "$CONFIG_DIR/config"

  t "usage_text: newest digest with rate limits wins; countdowns only for resets still ahead"
  now=$(date +%s)
  printf 'five=36\nfive_reset=%s\nweek=49\nweek_reset=%s\nat=%s\n' "$((now + 7900))" "$((now + 275000))" "$now" > "$TELE_DIR/$id.kv"
  printf 'five=80\nfive_reset=%s\nweek=90\nweek_reset=%s\nat=%s\n' "$((now + 100))" "$((now + 100))" "$((now - 60))" > "$TELE_DIR/$id2.kv"
  assert_eq "$(usage_text)" '5h ▓▓▓░░░░░░░ 36% ↻2h11m   wk ▓▓▓▓░░░░░░ 49% ↻3d4h'
  printf 'five=36\nfive_reset=%s\nweek=49\nweek_reset=\nat=%s\n' "$((now - 5))" "$now" > "$TELE_DIR/$id.kv"
  assert_eq "$(usage_text)" '5h ▓▓▓░░░░░░░ 36%   wk ▓▓▓▓░░░░░░ 49%'
  rm -f "$TELE_DIR/$id.kv"
  assert_eq "$(usage_text)" '5h ▓▓▓▓▓▓▓▓░░ 80% ↻1m   wk ▓▓▓▓▓▓▓▓▓░ 90% ↻1m'
  fresh; rm -rf "$TELE_DIR"
}
