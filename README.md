# gensokyo

A bash + tmux cockpit for running several Claude Code sessions side by side:
named residents in a status bar, a tab each, click one to work in it, desktop notifications
when one needs you, broadcast "spell cards", and scheduled "rituals".

**Status:** pre-alpha. The cockpit, summon/banish/list/recall, the status bar, the "needs you" notifications and the per-resident telemetry (model, context, cost, usage) work; broadcasts and schedules are not there yet.

## Requirements

**To run a release:** macOS, iTerm2 3.5 or newer, and Claude Code (≥ 2.1.224).
Nothing else. tmux and jq are vendored from their official dependency-free builds
(`vendor/README.md`).

Started from an iTerm2 tab, `gensokyo` runs its tmux session in control mode: the residents
are native iTerm2 panes and tabs, the chips live in iTerm2's status bar, and no key belongs
to gensokyo. Anywhere else — another terminal, or `gensokyo --tty` in iTerm2 — the plain tmux
client draws the cockpit itself, with its own status bar and the `Ctrl-Space` keys. One tmux
server serves both, but its status line can only be set up one way at a time, so detach one
kind of client before attaching the other. `gensokyo doctor` says which one you are about to
get, who is attached, and whether iTerm2 has the gensokyo profile that carries the status bar
and the per-pane title bars.

**To develop** (not needed for the release):

| Tool | Why | Install |
|---|---|---|
| `shellcheck` | every commit is shellcheck-clean (`shellcheck -x -s bash bin/gensokyo tests/run.sh tests/stub-claude`, `-s sh install.sh scripts/vendor.sh`; `-x` follows the `lib/*.sh` sources) | `brew install shellcheck` |
| `curl` | fetches the vendored binaries | preinstalled on macOS |
| Docker (optional) | run the Linux vendor binaries in `debian:stable-slim` | https://docker.com |
| Claude Code | the real acceptance tests spawn real sessions | https://code.claude.com |

`/bin/bash` 3.2 is the target for `bin/gensokyo` and the `lib/*.sh` files it sources
(records, registry, tmux server, cockpit keys and bar, resident commands, recall, hooks and
notifications, telemetry); `install.sh` and `scripts/*.sh` are POSIX `sh`. Do not use bash 4 features.

## Developing

```sh
git clone … gensokyo && cd gensokyo
scripts/vendor.sh          # fetch tmux + jq for this machine into vendor/<os>-<arch>/
scripts/vendor.sh --status
bin/gensokyo doctor        # which tmux / jq / claude will be used, versions, server state
bin/gensokyo               # attach the cockpit (--tty for the plain tmux client, --detach for the server alone)
bin/gensokyo help          # command list; --json is what the resident skill reads
bin/gensokyo new ~/dev/x -n Marisa   # a resident in a window of its own; also close <name>, list
bin/gensokyo resume [Marisa]         # who has departed; with a name, bring that one back
tests/run.sh                         # unit tests + a headless smoke test with the stub claude (-v for names)
```

## Installing

```sh
./install.sh                    # links ~/.local/bin/gensokyo -> bin/gensokyo; fetches tmux + jq if needed
./install.sh --bin-dir ~/bin    # another link directory (or GENSOKYO_BIN_DIR)
./install.sh --no-fetch         # never download; needs vendored or system tmux >= 3.3 and jq >= 1.6
gensokyo doctor                 # shows which copy is on PATH, the plugin dir and what resolved
```

`install.sh` is POSIX `sh`, writes nothing outside the checkout except that one symlink, and
prints the `export PATH=…` line if the link directory is not on your PATH. Running from the
checkout without installing also works (`bin/gensokyo`); residents then find the CLI through
`$GENSOKYO_BIN`. Release tarballs and `curl | sh` come with the first release.

`bin/gensokyo`
reads no `~/.tmux.conf` and never edits `~/.claude/settings.json`: it runs its own tmux server
(`tmux -L gensokyo`) with `share/tmux.conf`, and per-user tweaks go in `~/.config/gensokyo/`
(`config` for KEY=value settings such as `PREFIX=C-Space`, `tmux.conf` sourced last).

## When a resident needs you

Every resident is launched with `--settings` carrying a few hooks (they merge with the user's
own hooks, nothing in `~/.claude` changes). The hooks call `gensokyo _hook`, which records what
the resident waits for and turns its chip gold: `✦` for a permission prompt or a finished turn
nobody has looked at yet, `✧` for a question it asked. The first transition into a waiting
state also shows a tmux message on every attached client, a desktop notification (`osascript`
on macOS, `notify-send` on Linux when present) and a terminal bell, so iTerm2 bounces the Dock
icon when the window is in the background. Desktop alert and bell are skipped when that pane is
on screen in a client you touched in the last ten seconds, and nothing repeats while the resident
keeps waiting. Typing in the resident clears the flag. `~/.config/gensokyo/config` can set
`NOTIFY_TOAST`, `NOTIFY_DESKTOP` or `NOTIFY_BELL` to `off`. macOS asks once whether your terminal
application may show notifications; `gensokyo doctor` reminds you.

## Departing and coming back

`gensokyo close Youmu` (or `/exit` inside the pane) ends the Claude Code session; the pane
stays and shows `Youmu has left the shrine` with `r` to recall and `x` to close. The session
is Claude Code's, so it can be resumed as long as its transcript exists under
`~/.claude/projects/`. `gensokyo resume Youmu` (name, slot or session id; `recall` is the
same command) brings a departed resident back: into its pane if that is still open, otherwise
into a new slot. `gensokyo resume` alone lists everyone who has departed, newest first,
including residents from earlier runs of the cockpit (their records move to the state
directory's `departed/` when the tmux server restarts); `--json` gives the same to tools.
`Ctrl-Space g r` is the menu of the same list. A recalled resident keeps its session id, name
and transcript, gets the same launch flags as at summon time, and the hooks and status line
again; the first prompt given at summon is not replayed.

Claude Code asks its workspace trust question the first time it runs in a directory. That
dialog appears inside the pane like any other prompt; the hooks do not fire before it is
answered, so a fresh resident that sits in `starting` for long is usually waiting for that.

## What each resident reports

The same `--settings` points the resident's Claude Code status line at `gensokyo _statusline`.
Claude Code feeds it JSON after every API response (model, effort, context window, cache hit
rate, cost, and on Pro/Max/Team accounts the 5-hour and weekly usage); gensokyo keeps the last
report per resident under its state directory and shows it everywhere:

- **Chips**: `1 ✦ Reimu Sonnet 42%`, model and context used. In iTerm2's status bar (left),
  where a resident who needs you comes first; row 1 of the tmux bar under `--tty`, where it
  turns gold instead. They follow a hook at once and are refreshed every three seconds.
- **Usage**: the account-wide numbers from the newest report,
  `5h ▓▓▓░░░░░░░ 37% ↻2h11m   wk ▓▓▓▓▓▓░░░░ 62% ↻3d4h` (hidden on API-key accounts), at the
  right of iTerm2's status bar, or of row 2 under `--tty`.
- **The shrine tab**: gensokyo's own first tab draws a line per resident - slot, state, name,
  directory, branch, the same telemetry as the border, and what a resident who needs you is
  waiting for. It redraws every three seconds and the moment a hook has news. Click a resident
  to bring its tab to the front, and the buttons under them to summon, banish or recall one;
  every button carries the letter that does the same thing, and `?` lists them.
- **Pane border**: `1 Reimu · gensokyo ⎇ main · Sonnet 5→⚖ Opus · high · plan · ⚡91% · $0.42`:
  directory and branch (read from `.git/HEAD`, no git needed), model and advisor model (from
  the resident's settings chain), effort, permission mode (from the hooks: known after the first
  prompt; a Shift+Tab shows at the next prompt), session cache hit rate, cost. Unknown fields
  are left out.
- **Inside the pane**: gensokyo's own one-line status line,
  `Sonnet 5→⚖ Opus · medium · ▓░░░░░░░░░ 12% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m`
  (context bar with the window size, session and per-turn cache, cost, lines added/removed, age).
  Prefer your own Claude Code status line? `STATUSLINE=user` in the config makes gensokyo run
  your `statusLine` command with the same JSON after recording it, output untouched.
- **`gensokyo list`** (and `Ctrl-Space g w`) adds a line per resident with all of it plus the
  age of the data, and a `usage` line; `list --json` carries the same under `telemetry`
  (null until the first report) and `branch`.

Numbers refresh only when Claude Code calls the API, so a resident idle for hours shows its
last values; the age tells. Sessions not started by gensokyo have no telemetry.

Environment overrides for tests and CI: `GENSOKYO_TMUX`, `GENSOKYO_JQ`, `GENSOKYO_CLAUDE`
(binaries), `GENSOKYO_STATE_DIR`, `GENSOKYO_CONFIG_DIR`, `GENSOKYO_SOCKET`. `tests/stub-claude` stands in
for `claude` (registry, names, /rename, /exit) so the cockpit runs without Claude Code or a login:
`GENSOKYO_CLAUDE=$PWD/tests/stub-claude GENSOKYO_SOCKET=t bin/gensokyo`. `tests/run.sh` does exactly
that on its own socket and state dir, so it can run in CI.

## Residents speak gensokyo

Every resident is launched with `--plugin-dir share/plugin` and a one-paragraph
`--append-system-prompt`, both per session (`~/.claude` is never touched). The plugin's
`gensokyo` skill is the resident's handbook: plain requests such as "start a new agent in
~/dev/x called Cirno", "who is waiting on me?", "close Sakuya" or "show me Marisa" turn into
`gensokyo new/list/close/focus` calls, with a confirmation before closing. Themed words
(summon, who, banish, recall) work too. The skill reads `gensokyo help --json` for the
authoritative command list. `tests/skills/gensokyo.md` is the phrasing checklist run by hand
against a real resident, since the stub cannot run a model.
