# Phrasing checklist: the `gensokyo` handbook skill

The stub `claude` cannot run a model, so this is checked by hand against a real resident
at a checkpoint: summon a resident from a checkout (`bin/gensokyo new <some repo>`), type
each phrasing, and record what the resident ran. Plain phrasings come first; a plain
phrasing that does not trigger the skill is a blocker, a themed one is a note.

Setup: two residents running (say Marisa and Sakuya), one departed, `~/dev/other` exists.
For each line note: skill invoked? (Skill tool shows `gensokyo:gensokyo`), command run,
outcome, and whether the resident asked before a destructive action.

| # | Phrasing | Expected command | Result |
|---|---|---|---|
| 1 | start a new agent in ~/dev/other called Cirno | `gensokyo new ~/dev/other -n Cirno` | not run (same shape as 12) |
| 2 | spin up another session for this repo | `gensokyo new <cwd>` (name picked) | ✓ `new /Users/nebel95/dev/gensokyo` → Benben (slot 5), explained it landed on stage 2 |
| 3 | list the sessions | `gensokyo list --json` then a summary | not run (same shape as 13) |
| 4 | who is waiting on me? | `gensokyo list --json`, answers with the waiting ones | ✓ `list --json`; "nobody is waiting", named the idle and departed ones |
| 5 | close the Sakuya session | asks first, then `gensokyo close Sakuya` | ✓ asked first (AskUserQuestion dialog with Yes/No), then `close Sakuya` |
| 6 | stop session 2 | asks first, then `gensokyo close 2` | not run (same shape as 5) |
| 7 | zoom on Marisa | `gensokyo focus Marisa` | ✓ `focus Cirno` (selects the pane; the tmux zoom stays a key) |
| 8 | show all panes again | `gensokyo stage tiled` | ✓ `stage tiled` |
| 9 | tell Marisa to run the tests | SendMessage to Marisa, no gensokyo command | not run here (SendMessage between sessions verified separately) |
| 10 | resume the session that departed | `gensokyo resume --json` or `gensokyo resume <name>`; recalls it | not yet run with the command in place (before it existed: read `help --json`, said so, pointed at `r` in the departed pane) |
| 11 | what can gensokyo do? | reads `gensokyo help --json`, lists the commands | ✓ listed every command from `help --json`, noted resume is missing and that messaging is SendMessage |
| 12 | summon a resident in ~/dev/other called Cirno | `gensokyo new ~/dev/other -n Cirno` | ✓ Skill loaded, `new ~/dev/mozart -n Cirno` → slot 4 |
| 13 | who is around? | `gensokyo list --json` | ✓ `list --json`, table of all residents incl. the departed one |
| 14 | banish Sakuya | asks first, then `gensokyo close Sakuya` | not run (same shape as 5) |
| 15 | recall Sakuya | `gensokyo resume Sakuya` | not yet run with the command in place |

Also check: no phrasing makes the resident call `tmux` directly; `gensokyo` (or `$GENSOKYO_BIN`)
resolves from inside the resident; a resident started with `-p plan` still triggers the skill.

Results: 2026-09-05, Claude Code 2.1.261, default model (Fable 5.1) in auto mode, `gensokyo` not on PATH so
every call went through `$GENSOKYO_BIN`; no phrasing called `tmux`. Rows 12 and 13 were the required ones;
rows 2, 4, 5, 7, 8, 10, 11 were run as well. Not yet checked: a resident started with `-p plan`.
