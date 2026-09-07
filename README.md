# gensokyo

A bash + tmux cockpit for running several Claude Code sessions side by side:
named residents in a status bar, a tab each, click one to work in it, desktop notifications
when one needs you, broadcast "spell cards", and scheduled "rituals".

**Status:** pre-alpha. The cockpit, summon/banish/list/recall, the status bar, the "needs you" notifications, the per-resident telemetry (model, context, cost, usage) and the spell cards work; schedules are not there yet.

## Requirements

**To run a release:** macOS, iTerm2 3.5 or newer, and Claude Code (≥ 2.1.224).
Nothing else. tmux and jq are vendored from their official dependency-free builds
(`vendor/README.md`).

Started from an iTerm2 tab, `gensokyo` runs its tmux session in control mode: the residents
get a native iTerm2 tab each, the chips live in iTerm2's status bar, and no key belongs
to gensokyo. Anywhere else — another terminal, or `gensokyo --tty` in iTerm2 — the plain tmux
client draws the cockpit itself, with its own status bar and the `Ctrl-Space` keys. One tmux
server serves both, but its status line can only be set up one way at a time, so detach one
kind of client before attaching the other. `gensokyo doctor` says which one you are about to
get, who is attached, and whether iTerm2 has the gensokyo profile that carries the chips.

One iTerm2 setting has to be yours: **Settings > General > tmux > "When attaching, restore
window as" > Tabs in the attaching window**. At what iTerm2 ships instead, every resident gets a
macOS window of its own, and clicking one in the shrine opens yet another window rather than
raising its tab. `gensokyo doctor` says which of the two you are set to; it never writes the
setting, or any other iTerm2 preference.

`gensokyo iterm setup` is the one file gensokyo may add: a *dynamic profile* named "gensokyo" in
`~/Library/Application Support/iTerm2/DynamicProfiles/`, which iTerm2 picks up the moment it is
written. It inherits everything — font, colours, keys — from your default profile and adds one
thing to it, the status bar with the two components that show gensokyo's chips and your usage.
Start the cockpit from a tab using that profile (Profiles menu, or set it as the default) and
the chips appear. `gensokyo iterm remove` deletes the file again; both refuse to touch a
`gensokyo.json` they did not write.

**To develop** (not needed for the release):

| Tool | Why | Install |
|---|---|---|
| `shellcheck` | every commit is shellcheck-clean (`shellcheck -x -s bash bin/gensokyo tests/run.sh tests/stub-claude`, `-s sh install.sh scripts/vendor.sh`; `-x` follows the `lib/*.sh` and `tests/cases/*.sh` sources) | `brew install shellcheck` |
| `curl` | fetches the vendored binaries | preinstalled on macOS |
| Claude Code | the real acceptance tests spawn real sessions | https://code.claude.com |

`/bin/bash` 3.2 is the target for `bin/gensokyo` and the `lib/*.sh` files it sources
(records, registry, tmux server and bar, attach, resident commands, recall, the shrine tab,
hooks and notifications, telemetry, the iTerm2 profile); `install.sh` and `scripts/*.sh` are
POSIX `sh`. Do not use bash 4 features.

## Developing

```sh
git clone … gensokyo && cd gensokyo
scripts/vendor.sh          # fetch tmux + jq for this machine into vendor/<os>-<arch>/
scripts/vendor.sh --status
bin/gensokyo doctor        # which tmux / jq / claude will be used, versions, server state
bin/gensokyo               # attach the cockpit (--tty for the plain tmux client, --detach for the server alone)
bin/gensokyo help          # command list; --json is the same for tools
bin/gensokyo new ~/dev/x -n Marisa   # a resident in a tab of its own; also close <name>, list
bin/gensokyo resume [Marisa]         # who has departed; with a name, bring that one back
bin/gensokyo reload                  # after changing bin/gensokyo or lib/*.sh: run the new code (Ctrl-Space l)
bin/gensokyo quit                    # ask everyone to /exit, then close the cockpit (Ctrl-Space g q)
tests/run.sh                         # unit tests + a headless smoke test with the stub claude (-v for names)
                                     # the harness lives there, the tests in tests/cases/*.sh
```

## Installing

```sh
./install.sh                    # links ~/.local/bin/gensokyo -> bin/gensokyo; fetches tmux + jq if needed
./install.sh --bin-dir ~/bin    # another link directory (or GENSOKYO_BIN_DIR)
./install.sh --no-fetch         # never download; needs vendored or system tmux >= 3.3 and jq >= 1.6
gensokyo doctor                 # shows which copy is on PATH, the plugin dir and what resolved
gensokyo iterm setup            # add the "gensokyo" iTerm2 profile (iterm remove takes it away)
```

`install.sh` is POSIX `sh`, writes nothing outside the checkout except that one symlink, and
prints the `export PATH=…` line if the link directory is not on your PATH. Running from the
checkout without installing also works (`bin/gensokyo`); a resident's shell finds that copy as
`$GENSOKYO_BIN`. Release tarballs and `curl | sh` come with the first release.

`bin/gensokyo`
reads no `~/.tmux.conf` and never edits `~/.claude/settings.json`: it runs its own tmux server
(`tmux -L gensokyo`) with `share/tmux.conf`, and per-user tweaks go in `~/.config/gensokyo/`
(`config` for KEY=value settings such as `PREFIX=C-Space`, `tmux.conf` sourced last).

## Using the cockpit

`gensokyo` in an iTerm2 tab opens the cockpit: gensokyo's own first tab, the **shrine**, and one
iTerm2 tab per resident after it. No key belongs to gensokyo there — everything is a click.

- The shrine draws a line per resident and a row of buttons under them. Click `[ summon ]`,
  click one of the directories it offers (or `other directory…` and type a path, Tab completes),
  type a name or press Enter for a random one: a tab appears with that resident in it.
- Click a resident's line in the shrine, or its tab in the tab bar, to work in it. A resident
  fills its tab; nothing is ever split, tiled or zoomed, and you can drag a tab out to watch two
  at once.
- `[ banish ]` asks which resident and then asks again before interrupting one: it stops
  mid-thought and its tab shows the departed screen, where `[ close ]` finally lets the tab go.
  `[ recall ]` lists everyone who has departed, this run or an earlier one; `[ quit ]` closes
  the whole cockpit.
- `[ cast ]` sends one prompt to several residents at once; the section below is about that.
- Every button carries the letter that does the same thing (`[ summon n ]`) for when your hands
  are already on the keyboard, and `[ ? ]` lists them all.

Shell > tmux > Detach leaves everyone running; `gensokyo` again brings all the tabs back. The
CLI (`gensokyo new`, `close`, `resume`, `list`, `broadcast`, `quit`) does the same things for
scripts and for gensokyo's own use, but nothing in the cockpit needs you to type it.

With the plain tmux client — `gensokyo --tty`, or any terminal that is not iTerm2 — the
shrine is a window instead of a tab, and the same actions are on `Ctrl-Space g` and a letter,
with `Ctrl-Space g ?` listing them.

`gensokyo reload` (the `reload` button on the shrine tab, or `Ctrl-Space l` under `--tty`)
runs the code that is on disk now: the tmux options, the key bindings, the bar style, the
pinned binaries, and the two processes gensokyo runs of its own - the shrine's loop and the
clock behind the status bar - without touching a single resident. Editing `lib/*.sh` does not
reach anything already running, so this is what to press after a change; two things it cannot
reach are the settings a resident was launched with (a change to the hooks needs that resident
recalled) and the environment the tmux server itself inherited, such as `PATH` or a newly
vendored tmux, which needs the cockpit restarted.

## Spell cards

A **spell card** is a prompt in a file. `[ cast ]` on the shrine tab asks which card, then who
gets it — everyone, everyone who needs you, everyone who is resting, or one resident — and
types it into each of their prompts, exactly as if you had typed it there yourself. Four ship
with gensokyo:

| Card | What it asks for |
|---|---|
| `Spirit Sign "Status Report"` | three lines from every resident: what it is on, where that stands, what is next |
| `Border Sign "Sync Up"` | every resident tells the others what it is on, and raises any overlap with the one it affects |
| `Review Sign "Second Opinion"` | one resident asks another to review its uncommitted diff, and iterates until it hears LGTM |
| `Time Sign "Wrap Up"` | summarize the session, leave the tree clean, then go quiet |

Your own go in `~/.config/gensokyo/spellcards/<name>.md`, and writing one is writing a file —
ask any Claude Code session to do it. The name uses letters, digits, `.`, `_` and `-`. A little
frontmatter is optional; the rest is the prompt:

```markdown
---
title: Moon Sign "Test It"
summary: run the tests and say only whether they pass
peer: required
---
Run the test suite in {cwd} and tell me in one line whether it passes. Then ask {peer}
whether it agrees, and say that its reply must come back to you as a SendMessage
addressed to {self}.
```

Four placeholders are filled in for each resident the card reaches: `{self}` is its own name,
`{cwd}` its directory, `{residents}` the names of the other live residents (or `nobody`), and
`{peer}` the resident you pick after the target. A card whose frontmatter says
`peer: required` is a **pair card**: it goes to one resident, and the shrine asks who that
resident should talk to.

Residents talk to each other with `SendMessage`, and Claude Code keeps no thread of such an
exchange — so the rules of one live in the card's own text, where they cost a resident nothing
until that card is cast. The sentence that matters most is the reply address: **a card that
asks for an answer has to say that the answer comes back as a `SendMessage` addressed to
`{self}`**. Told only what the reply should look like, the resident being asked writes its
findings into its own pane, where the resident waiting for them never sees it — and the wait
looks exactly like `SendMessage` being broken. `Review Sign "Second Opinion"` says it; so
should yours.

Casting never types into a resident that has something open — a permission or plan dialog, a
question of its own, or the workspace-trust dialog a session shows the first time it is
summoned into a directory. The card would go into the dialog and the Enter after it would
*answer* the dialog, which is not what you asked for. Those residents are named and left out
rather than skipped quietly, and a card that does not reach a resident's prompt is reported as
not sent rather than counted:

```
$ gensokyo broadcast status-report all
gensokyo: broadcast: Cirno is still starting up; left out
cast Spirit Sign "Status Report" on 2 residents
```

Casting also clears a resident's gold `✦`: gensokyo typed for you, so whatever it was waiting
to be told, it has been.

`gensokyo broadcast` is the same thing for scripts: `gensokyo broadcast` alone lists the cards,
`gensokyo broadcast status-report all` casts one, and
`gensokyo broadcast second-opinion Marisa --with Sakuya` casts a pair card. A card is named by
its filename or its title, or by enough of either to pick out one card. There is no way to
broadcast free text: telling one resident something is typing in its pane, and a prompt worth
sending to everybody is worth a file.

## When a resident needs you

Every resident is launched with `--settings` carrying a few hooks (they merge with the user's
own hooks, nothing in `~/.claude` changes). The hooks call `gensokyo _hook`, which records what
the resident waits for and turns its chip gold: `✦` for a permission prompt or a finished turn
nobody has looked at yet, `✧` for a question it asked. The glyph goes on that resident's tab
title too, so the tab bar shows who is waiting. The first transition into a waiting state also
brings a desktop notification (`osascript` on macOS, `notify-send` on Linux when present) and a
terminal bell, which marks the tab in iTerm2. Under the plain client it shows a tmux message
on top of that; iTerm2 draws no tmux messages, so none is sent to it. Desktop alert and bell
are skipped when you are already watching that resident: in iTerm2, when it is the application
in front and the tab you are looking at is that resident's - not merely the last cockpit tab
you visited, since an ordinary iTerm2 tab beside the cockpit counts as looking away; with the
plain client, when its pane is on screen in a client you touched in the last ten seconds.
Nothing repeats while the resident keeps waiting, and typing in the resident clears the flag.
`~/.config/gensokyo/config` can set `NOTIFY_TOAST`, `NOTIFY_DESKTOP` or `NOTIFY_BELL` to `off`.
macOS asks once whether your terminal application may show notifications; `gensokyo doctor`
reminds you.

## Departing and coming back

`/exit` inside a resident (or the shrine's `[ banish ]`, or `gensokyo close Youmu`) ends the
Claude Code session; the tab stays and shows `Youmu has left the shrine` with `[ recall ]` and
`[ close ]` to click, or `r` and `x` to press. The session is Claude Code's, so it can be
resumed as long as its transcript exists under `~/.claude/projects/`. The shrine's `[ recall ]`
button lists everyone who has departed, newest first, including residents from earlier runs of
the cockpit (their records move to the state directory's `departed/` when the tmux server
restarts); clicking one brings it back, into its own tab if that is still open and otherwise
into a new one. `Ctrl-Space g r` is the same list under `--tty`, and `gensokyo resume` is the
command behind both — with a name, slot or session id it recalls that one, alone it prints the
list, and `--json` gives the same to tools. A recalled resident keeps its session id, name and
transcript, gets the same launch flags as at summon time, and the hooks and status line again;
the first prompt given at summon is not replayed.

The shrine's `[ quit ]` button closes the whole cockpit, and asks before it does: everyone still
here is asked to `/exit`, and once they have gone - or twenty seconds later, whichever comes
first - the tmux server stops, which takes the shrine, the clock and every tab with it; in iTerm2
that is the tmux tabs, and the window they were in stays. `gensokyo quit` and, under `--tty`,
`Ctrl-Space g q` do the same thing. Nothing is lost by it: the next `gensokyo` finds the
records of everyone who was here and offers them all back under `[ recall ]`.

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
- **`gensokyo list`** (and `Ctrl-Space g w` under `--tty`) adds a line per resident with all
  of it plus the age of the data, and a `usage` line; `list --json` carries the same under `telemetry`
  (null until the first report) and `branch`.

Numbers refresh only when Claude Code calls the API, so a resident idle for hours shows its
last values; the age tells. Sessions not started by gensokyo have no telemetry.

Environment overrides for tests and CI: `GENSOKYO_TMUX`, `GENSOKYO_JQ`, `GENSOKYO_CLAUDE`
(binaries), `GENSOKYO_STATE_DIR`, `GENSOKYO_CONFIG_DIR`, `GENSOKYO_SOCKET`, `GENSOKYO_ITERM_DIR`
(where the dynamic profile is written). `tests/stub-claude` stands in
for `claude` (registry, names, /rename, /exit) so the cockpit runs without Claude Code or a login:
`GENSOKYO_CLAUDE=$PWD/tests/stub-claude GENSOKYO_SOCKET=t bin/gensokyo`. `tests/run.sh` does exactly
that on its own socket and state dir, so it can run in CI.

## What residents are told

Almost nothing, on purpose: a resident's context window is your budget, and anything gensokyo
teaches it sits there for the whole session whether it is used or not. Everything the cockpit
does is a click, so no resident is taught how to start, close, recall or switch between the
others — you do that yourself, in one gesture, and their contexts stay yours to spend.

What every resident is launched with, per session and without touching `~/.claude`, is
`--plugin-dir share/plugin` and a three-sentence `--append-system-prompt`: the name it is
living under, that the other sessions in `claude agents` can be written to with `SendMessage`
but that the first message of an exchange is the `gensokyo-peers` skill's to write, and that a
standing schedule belongs to gensokyo rather than to Claude Code's own scheduling. The last two
are the only things a click cannot express.

The plugin carries one skill, `gensokyo-peers`, and it is deliberately the only one. Casting a
card is a button and needs no words spent on it, and a card's own text carries whatever rules
that card needs. But an exchange you start by *typing* — "get Sakuya to review this" — has no
card to carry them, and the rules are not guessable: Claude Code keeps no thread, so the
opening message has to name who is asking, what is wanted, the reply shape, the round cap, and
above all the reply address. Only the frontmatter description of a skill sits in a resident's
context; the rest is read if and when it is used.

Both skills are named in that paragraph as prohibitions — *never* compose that first message
yourself, *never* use Claude Code's own scheduling — and that phrasing is doing real work. A
resident merely told it *may* use `gensokyo-peers` does not: asked to "get Aya to review the
uncommitted diff", it writes a perfectly reasonable message that never says where the answer
should go, and the review lands in Aya's own pane where nobody is looking. Named as a
prohibition, the same request loads the skill and the message comes back.

The resident being written *to* is taught nothing at all: given one well-formed message it
keeps the convention it was addressed with, tag and all, without ever having been told the
format.
