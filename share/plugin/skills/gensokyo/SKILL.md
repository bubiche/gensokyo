---
name: gensokyo
description: Use when the user talks about other Claude Code sessions on this machine or wants to start a new agent/session in some directory, list who is running or waiting, close or stop a session, bring a departed session back, or focus one. Also triggers on the cockpit's own words - resident, summon, who, banish, recall, shrine. Runs the gensokyo CLI; never drives tmux directly.
---

# gensokyo handbook

You are one *resident* of gensokyo: a tmux cockpit that runs several Claude Code sessions
side by side. Each resident has a slot number (1, 2, ...), a name (the session name, shown in
the status bar) and a working directory. The user talks to residents in their panes and to you
about the others. Everything below goes through the `gensokyo` CLI.

## Where the CLI is

Run `gensokyo`; if that is not on PATH use `"$GENSOKYO_BIN"` (set in your environment). The
authoritative command list is `gensokyo help --json`. Read it when you are unsure what exists,
and never promise a command that is not in it.

## Glossary (plain word first, the cockpit's word after)

| Plain | Cockpit | Meaning |
|---|---|---|
| session, agent | resident | one Claude Code session in one pane |
| new, start | summon | start a resident in a directory |
| list | who | the residents: slot, state, name, directory |
| close, stop | banish | ask a resident to `/exit`; its pane shows a "departed" screen |
| resume | recall | bring a departed resident back into a pane |
| focus, switch to | focus | bring a resident's window to the front |
| status bar | shrine bar | the two red/cream rows at the top |
| exited | departed | the resident has left; it can be recalled |

## What the user says and what you run

| User says (plain first, cockpit words also count) | Run |
|---|---|
| "start a new agent in ~/dev/mozart called Marisa", "spin up another session for the rails repo", "summon a resident in ..." | `gensokyo new ~/dev/mozart -n Marisa` (`-n` optional: a name is picked; `-m model`, `-e effort`, `-p permission-mode` only when the user asked for them) |
| "list the sessions", "who's around?", "who's waiting on me?", "who" | `gensokyo list --json`, then summarize: slot, name, state, directory. States: `busy`, `waiting` (needs the user: a permission prompt, or a finished turn the user has not seen; `detail` says what), `question` (asked the user something; `detail` is the question), `idle`, `departed`. "Waiting on me" means `waiting` or `question`. `--all` adds sessions not started by gensokyo. |
| "what model is Marisa on?", "how full is Sakuya's context?", "what has this cost so far?", "how much of my 5-hour quota is left?", "which branch is Youmu on?" | `gensokyo list --json`: `branch`, `permission_mode`, and `telemetry` (`model`, `advisor`, `context_pct`, `effort`, `cache_pct`, `turn_cache_pct`, `cost_usd`, `five_hour`/`seven_day` `{used_pct, resets_at}`, `at` = epoch of the report; null until the resident's first API call). Usage is account-wide: read it from any row. |
| "close the fretwork session", "stop Youmu", "banish Youmu" | confirm first (another session's work is at stake), then `gensokyo close Youmu` (name or slot number) |
| "recall Youmu", "bring Youmu back", "resume session 3" | `gensokyo resume Youmu` (name or slot of a departed resident, or a session id). It comes back into its pane, or into a new slot when its pane is gone. |
| "resume the session I had on higan yesterday", "who can I bring back?" | `gensokyo resume --json` lists everyone who has departed (newest first: `session_id`, `name`, `cwd`, `departed_at`, `in_pane`). Match on directory and time; if more than one fits, ask which; then `gensokyo resume <session_id>`. |
| "show me Marisa", "switch to Sakuya", "focus 3" | `gensokyo focus Marisa` |
| "tell Marisa to run the tests", "ask Sakuya what she found" | not the CLI: use the SendMessage tool; residents are addressable by their session name |
| "message from Marisa?" | `gensokyo list --json` gives names; messages arrive through your normal channel |

## Rules

- Prefer the CLI over reaching into tmux; the cockpit tracks its own state.
- Confirm before closing a resident. Never close one the user did not name.
- Report the CLI's output back in plain words (which resident, which slot, where). Quote errors
  as they are.
- Resident names are case-insensitive; slots are the numbers in the status bar chips.
- When the user means Claude Code's own features (`/resume` for your own transcript, `/loop`,
  `/schedule`, `/rename`), say so and use those instead.
- If a command is not in `gensokyo help --json`, it does not exist yet; say that instead of
  improvising with tmux.
