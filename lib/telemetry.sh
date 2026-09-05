# lib/telemetry.sh - what a resident's Claude Code status line reports (model, context, effort,
# cache, cost, account usage), captured per resident by `gensokyo _statusline`, plus the git
# branch, and the formatting the chips, the pane border, bar row 2 and `list` share.
# Sourced by bin/gensokyo; bash 3.2.
# shellcheck shell=bash

TELE_DIR=$STATE_DIR/statusline

# ---------------------------------------------------------------- the status line wrapper
# `gensokyo _statusline <session-id>` is the statusLine command of every resident (set with
# `claude --settings`, which replaces the user's statusLine for that session only: statusLine
# is one key, last writer wins). Claude Code pipes JSON to it after every API response
# (Claude Code 2.1.260: model.display_name, effort.level when the model has one,
# context_window.used_percentage and current_usage, prompt_cache.hit_ratio,
# cost.total_cost_usd, rate_limits.five_hour/seven_day.{used_percentage,resets_at} on
# Pro/Max/Team accounts only). The wrapper keeps the raw JSON in statusline/<id>.json, a
# KEY=value digest in statusline/<id>.kv (read by the bar and border without spawning
# anything), then prints the pane's bottom line: gensokyo's own one-liner (own_statusline), or
# with STATUSLINE=user in the config the user's own statusLine command run with the same
# JSON, its output passed through byte for byte (their own line again when they have none).
cmd__statusline() {
  local id=$1 f=$TELE_DIR/$1 json cwd cmd=''
  json=$(cat)
  [ -n "$json" ] || return 0
  cwd=$(rec_get "$RES_DIR/$id" cwd); cwd=${cwd:-$PWD}
  mkdir -p "$TELE_DIR" 2>/dev/null
  printf '%s\n' "$json" > "$f.json.tmp.$$" 2>/dev/null && mv "$f.json.tmp.$$" "$f.json" 2>/dev/null
  # shellcheck disable=SC2016
  printf '%s' "$json" | jq_ -r --arg at "$(date +%s)" --arg advisor "$(settings_value "$cwd" .advisorModel)" '
    def s: if . == null then "" else (tostring | gsub("[|\n]"; " ")) end;
    def int: if . == null then "" else (floor | tostring) end;
    def turn: .context_window.current_usage as $u
      | if $u == null then "" else
          (($u.input_tokens // 0) + ($u.cache_read_input_tokens // 0) + ($u.cache_creation_input_tokens // 0)) as $t
          | if $t == 0 then "" else (($u.cache_read_input_tokens // 0) * 100 / $t | floor | tostring) end
        end;
    "model=\(.model.display_name | s)",
    "ctx=\(.context_window.used_percentage | int)",
    "effort=\(.effort.level | s)",
    "cache=\(.prompt_cache.hit_ratio | if . == null then "" else (. * 100 | floor | tostring) end)",
    "tcache=\(turn)",
    "cost=\(.cost.total_cost_usd | s)",
    "five=\(.rate_limits.five_hour.used_percentage | int)",
    "five_reset=\(.rate_limits.five_hour.resets_at | int)",
    "week=\(.rate_limits.seven_day.used_percentage | int)",
    "week_reset=\(.rate_limits.seven_day.resets_at | int)",
    "window=\(.context_window.context_window_size | int)",
    "added=\(.cost.total_lines_added | int)",
    "removed=\(.cost.total_lines_removed | int)",
    "dur=\(.cost.total_duration_ms | if . == null then "" else (. / 1000 | floor | tostring) end)",
    "advisor=\($advisor)",
    "at=\($at)"' > "$f.kv.tmp.$$" 2>/dev/null && mv "$f.kv.tmp.$$" "$f.kv" 2>/dev/null
  refresh_bar 2>/dev/null
  [ "$CFG_STATUSLINE" = user ] && cmd=$(settings_value "$cwd" '.statusLine.command')
  if [ -n "$cmd" ]; then
    printf '%s' "$json" | sh -c "$cmd"
  else
    own_statusline "$id"
  fi
}

# own_statusline <id>: the one line gensokyo shows at the bottom of a resident's pane. The
# border above already carries slot, name, directory, branch, permission mode and cost, and
# bar row 2 the account usage, so this line has the per-session numbers a working session
# watches: model→advisor, effort, the context window as a bar with its size, the cache hit
# rate (session, and this turn), cost, lines added/removed and the session's age. Unknown
# fields are left out.
own_statusline() {
  local out
  tele_load "$1"
  out=${T_model:-Claude}${T_advisor:+→⚖ $(title_word "$T_advisor")}
  [ -n "$T_effort" ] && out="$out · $T_effort"
  [ -n "$T_ctx" ] && out="$out · $(pct_bar "$T_ctx") $T_ctx%${T_window:+ of $(fmt_tokens "$T_window")}"
  [ -n "$T_cache" ] && out="$out · ⚡$T_cache%${T_tcache:+ (turn $T_tcache%)}"
  [ -n "$T_cost" ] && out="$out · $(fmt_cost "$T_cost")"
  # Lines and age only once there is something to count: the first reports (before any API
  # call) would otherwise read "+0/-0 · 0s".
  [ -n "$T_added" ] && [ "$T_added${T_removed:-0}" != 00 ] && out="$out · +$T_added/-${T_removed:-0}"
  [ -n "$T_dur" ] && [ "$T_dur" != 0 ] && out="$out · $(fmt_age "$T_dur")"
  printf '%s\n' "$out"
}

# tele_load <id>: the digest into T_<key>; everything empty when the resident has not
# reported yet (the first status line fires after the first API response).
tele_load() {
  # shellcheck disable=SC2034  # loaded for every reader; not every reader uses all of them
  T_model='' T_ctx='' T_effort='' T_cache='' T_tcache='' T_cost='' T_five='' T_five_reset='' T_week='' T_week_reset=''
  T_window='' T_added='' T_removed='' T_dur='' T_advisor='' T_at=''
  load_kv "$TELE_DIR/$1.kv" T model ctx effort cache tcache cost five five_reset week week_reset window added removed dur advisor at
}

# usage_newest: the account-wide 5-hour / weekly usage from the most recent digest that has
# it (rate limits are per account, so any resident's report is everyone's), into U_<key>.
usage_newest() {
  local f id best=0
  # shellcheck disable=SC2034
  U_five='' U_five_reset='' U_week='' U_week_reset='' U_at='' U_id=''
  for f in "$TELE_DIR"/*.kv; do
    [ -f "$f" ] || continue
    id=${f##*/}; id=${id%.kv}
    tele_load "$id"
    [ -n "$T_five$T_week" ] || continue
    [ "${T_at:-0}" -gt "$best" ] || continue
    best=$T_at
    U_five=$T_five U_five_reset=$T_five_reset U_week=$T_week U_week_reset=$T_week_reset U_at=$T_at U_id=$id
  done
}

# usage_text: "5h ▓▓▓░░░░░░░ 37% ↻2h11m   wk ▓▓▓▓▓▓░░░░ 62% ↻3d4h", or nothing (API-key
# accounts have no rate limits). A countdown is dropped once the window has reset.
usage_text() {
  local now out='' eta
  usage_newest
  now=$(date +%s)
  if [ -n "$U_five" ]; then
    out="5h $(pct_bar "$U_five") $U_five%"
    [ -n "$U_five_reset" ] && [ "$U_five_reset" -gt "$now" ] && out="$out ↻$(fmt_eta $((U_five_reset - now)))"
  fi
  if [ -n "$U_week" ]; then
    eta=''
    [ -n "$U_week_reset" ] && [ "$U_week_reset" -gt "$now" ] && eta=" ↻$(fmt_eta $((U_week_reset - now)))"
    out="${out:+$out   }wk $(pct_bar "$U_week") $U_week%$eta"
  fi
  printf '%s' "$out"
}

# ---------------------------------------------------------------- git branch
# git_branch <dir>: the checked-out branch of the repository containing dir (a detached HEAD
# shows its short hash), read from .git/HEAD without running git; worktrees (.git is a file
# pointing at the real git dir) are followed. Nothing when dir is not in a repository.
git_branch() {
  local d=$1 g head
  while [ -n "$d" ] && [ "$d" != / ]; do
    if [ -e "$d/.git" ]; then
      if [ -f "$d/.git" ]; then
        g=$(sed -n 's/^gitdir: //p' "$d/.git" 2>/dev/null)
        case $g in /*) ;; *) g=$d/$g ;; esac
      else
        g=$d/.git
      fi
      [ -f "$g/HEAD" ] || return 0
      IFS= read -r head < "$g/HEAD" || [ -n "$head" ]
      case $head in
        "ref: refs/heads/"*) printf '%s' "${head#ref: refs/heads/}" ;;
        *) printf '%s' "${head:0:7}" ;;
      esac
      return 0
    fi
    d=${d%/*}
  done
  return 0
}

# ---------------------------------------------------------------- formatting
# mode_label <permission mode>: the short word the border shows.
mode_label() {
  case $1 in
    acceptEdits) printf 'accept-edits' ;;
    bypassPermissions) printf 'bypass' ;;
    dontAsk) printf 'dont-ask' ;;
    *) printf '%s' "$1" ;;
  esac
}
# title_word <word>: opus -> Opus (the advisor model is a plain lowercase word in settings).
title_word() { printf '%s%s' "$(printf '%s' "${1:0:1}" | tr '[:lower:]' '[:upper:]')" "${1:1}"; }
fmt_cost() { LC_ALL=C printf '$%.2f' "$1"; }
# model_short <display name>: "Sonnet 5" -> Sonnet, for the chips.
model_short() { printf '%s' "${1%% *}"; }
# fmt_tokens <count>: 1000000 -> 1M, 200000 -> 200k.
fmt_tokens() {
  if [ "$1" -ge 1000000 ]; then printf '%sM' "$(($1 / 1000000))"
  elif [ "$1" -ge 1000 ]; then printf '%sk' "$(($1 / 1000))"
  else printf '%s' "$1"; fi
}

# pct_bar <0-100>: ten cells, filled per tenth.
pct_bar() {
  local n=$(($1 / 10)) i out=''
  [ "$n" -gt 10 ] && n=10
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if [ "$i" -le "$n" ]; then out="${out}▓"; else out="${out}░"; fi   # braces: bash reads $out▓ as one name
  done
  printf '%s' "$out"
}

# fmt_eta <seconds>: 3d4h, 2h11m, 14m, now.
fmt_eta() {
  local s=$1 d h m
  [ "$s" -gt 0 ] || { printf 'now'; return 0; }
  d=$((s / 86400)) h=$(((s % 86400) / 3600)) m=$(((s % 3600) / 60))
  if [ "$d" -gt 0 ]; then printf '%sd%sh' "$d" "$h"
  elif [ "$h" -gt 0 ]; then printf '%sh%sm' "$h" "$m"
  else printf '%sm' "$m"; fi
}
# fmt_age <seconds>: 12s, 3m, 2h, 1d.
fmt_age() {
  local s=$1
  if [ "$s" -lt 60 ]; then printf '%ss' "$s"
  elif [ "$s" -lt 3600 ]; then printf '%sm' "$((s / 60))"
  elif [ "$s" -lt 86400 ]; then printf '%sh' "$((s / 3600))"
  else printf '%sd' "$((s / 86400))"; fi
}

# tele_fields <model> <ctx> <effort> <cache> <tcache> <cost> <branch> <advisor> <mode> [verbose]:
# the " · "-joined telemetry the border and `list` show; unknown fields are left out, never
# shown as "?". The border leaves context out (the chip has it); verbose, for `list`, adds it
# with a label, the per-turn cache rate and the branch.
tele_fields() {
  local model=$1 ctx=$2 effort=$3 cache=$4 tcache=$5 cost=$6 branch=$7 advisor=$8 mode=$9 verbose=${10:-} out=''
  [ -n "$model" ] && out="$model${advisor:+→⚖ $(title_word "$advisor")}"
  [ -n "$ctx" ] && [ -n "$verbose" ] && out="${out:+$out · }ctx $ctx%"
  [ -n "$effort" ] && out="${out:+$out · }$effort"
  [ -n "$mode" ] && out="${out:+$out · }$(mode_label "$mode")"
  if [ -n "$cache" ]; then
    out="${out:+$out · }⚡$cache%"
    [ -n "$verbose" ] && [ -n "$tcache" ] && out="$out (turn $tcache%)"
  fi
  [ -n "$cost" ] && out="${out:+$out · }$(fmt_cost "$cost")"
  [ -n "$branch" ] && [ -n "$verbose" ] && out="${out:+$out · }⎇ $branch"
  printf '%s' "$out"
}
