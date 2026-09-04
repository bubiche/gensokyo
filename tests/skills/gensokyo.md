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
| 1 | start a new agent in ~/dev/other called Cirno | `gensokyo new ~/dev/other -n Cirno` | |
| 2 | spin up another session for this repo | `gensokyo new <cwd>` (name picked) | |
| 3 | list the sessions | `gensokyo list --json` then a summary | |
| 4 | who is waiting on me? | `gensokyo list --json`, answers with the waiting ones | |
| 5 | close the Sakuya session | asks first, then `gensokyo close Sakuya` | |
| 6 | stop session 2 | asks first, then `gensokyo close 2` | |
| 7 | zoom on Marisa | `gensokyo focus Marisa` | |
| 8 | show all panes again | `gensokyo stage tiled` | |
| 9 | tell Marisa to run the tests | SendMessage to Marisa, no gensokyo command | |
| 10 | resume the session that departed | says resume is not available yet, points at `r` in the pane | |
| 11 | what can gensokyo do? | reads `gensokyo help --json`, lists the commands | |
| 12 | summon a resident in ~/dev/other called Cirno | `gensokyo new ~/dev/other -n Cirno` | |
| 13 | who is around? | `gensokyo list --json` | |
| 14 | banish Sakuya | asks first, then `gensokyo close Sakuya` | |
| 15 | recall Sakuya | says not available yet | |

Also check: no phrasing makes the resident call `tmux` directly; `gensokyo` (or `$GENSOKYO_BIN`)
resolves from inside the resident; a resident started with `-p plan` still triggers the skill.

Results (date, Claude Code version, model): _to be filled at the checkpoint_.
