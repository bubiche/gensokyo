# gensokyo

Several Claude Code sessions side by side, one shown at a time. Each session is a **resident**
of the shrine: a sidebar lists them all with what each is doing, the one you pick fills the rest
of the screen, and a resident that needs you — a permission prompt, a question, a finished turn
nobody has read — turns gold and rings. Prompts can be cast at several residents at once
(**spell cards**) and fired on a schedule (**rituals**).

**Status:** pre-release. One Rust binary: a daemon that owns every resident's terminal, and a
client that draws the shrine. It needs macOS and is built for iTerm2; other terminals mostly
work. There is no installer yet: build it from source (below).

## Requirements

- macOS, and Claude Code (`claude` on `PATH`, 2.1.224 or newer).
- iTerm2 3.5 or newer is the terminal it is made for. Terminal.app works without the kitty
  keyboard protocol, so Shift+Enter and a few other chords are not told apart there.
- To build: Rust (the version in `.tool-versions`), Zig **0.16.0** exactly (the pinned Ghostty
  builds with nothing else; `zig version` says which you have), and git.

## Building

```sh
git clone https://github.com/bubiche/gensokyo && cd gensokyo
scripts/ghostty.sh              # the pinned Ghostty source into vendor/, checked; needs zig 0.16.0
cargo build --release           # target/release/gensokyo, linked against system libraries only
```

Put `target/release/gensokyo` on your `PATH`; a symlink to it is fine, and finds the shipped
names, cards and rituals in the checkout it was built in (`GENSOKYO_SHARE` points elsewhere).

After a rebuild, `gensokyo restart` replaces the running daemon with the new binary and brings
every resident back into its own conversation. Run it from a terminal of your own, not from
inside a resident.

## The shrine

`gensokyo` with no command opens the shrine, starting the daemon if it is not running. The
daemon keeps running after the client leaves: closing the terminal or detaching (`Ctrl-] d`)
leaves every resident as it was, and `gensokyo` again shows them.

Keys go to the resident on screen, except the **leader**, `Ctrl-]`, and the key after it. The
sidebar's foot says so (` ^] then a key `), and once the leader is pressed the sidebar lists
what comes next:

| `Ctrl-]` then | |
|---|---|
| `n` / `c` | summon a resident / cast a spell card |
| `b` / `r` | banish the one on screen / recall a departed one |
| `t` | the timetable of rituals |
| `x` | close the one on screen (`/exit`; a departed one leaves the sidebar) |
| `j` / `k` | the next / previous resident in the sidebar |
| `a` | the next resident that needs you |
| `1`–`9` | the resident in that slot |
| `[` | scroll back through the resident's scrollback |
| `m` | mouse capture on or off |
| `d` | detach: the client leaves, the residents keep running |
| `q` | quit: every resident `/exit`s and the daemon stops |
| `?` | help, with the glyphs |
| `Ctrl-]` | send the resident a `Ctrl-]` of its own |

With nobody on screen, or a departed resident on screen, the keys work without the leader.
The wheel scrolls back through a resident's scrollback too. Scrolled back, `j`/`k` or the
arrows move a row, `b`/`f`, Space or PgUp/PgDn a screen, `g` or Home goes to the top, and `q`,
`G`, End or Esc back to the live screen; anything else you type goes back to the live screen
and on to the resident. A resident's scrollback is its
own, so two clients showing it scroll together.

In a dialog, Enter does the main thing, Esc goes back one stage, and `y`/`n` answer the
yes-or-no ones.

Everything is also a click: the sidebar's lines focus a resident, and every button carries the
key that does the same thing (`[summon n]`). `[mouse: shrine m]` turns mouse capture off, which
hands selection back to iTerm2; with it on, a drag in the resident's screen selects text and
copies it with `pbcopy` (`GENSOKYO_COPY` names another command).

A resident that has left — `/exit`, a banish, a crash — stays in the sidebar as departed, with
`[recall r]` and `[close x]`. Recall brings it back into its own conversation with the flags it
was summoned with; `Ctrl-] r` lists everyone who has departed, this run or an earlier one.
`Ctrl-] q` asks every resident to `/exit` and stops the daemon; the next `gensokyo` offers them
all back under recall.

The same things from a shell, for scripts and for the residents' own skills:

```sh
gensokyo list [--all] [--json]          # who is here: slot, state, directory, model, context
gensokyo new ~/dev/x -n Marisa -m haiku # summon; --prompt gives it a first prompt
gensokyo resume [Marisa]                # the departed; with a name, slot or id, bring one back
gensokyo banish Marisa                  # hang up: HUP, then TERM, then KILL
gensokyo close Marisa                   # ask it to /exit; a departed one leaves the sidebar
gensokyo broadcast status-report all    # cast a spell card
gensokyo ritual                         # what is scheduled (the section below)
gensokyo quit                           # everyone /exit, then the daemon stops
gensokyo restart                        # a new daemon, the same residents
gensokyo help <command>                 # every command has its own
```

## When a resident needs you

Every resident is launched with `--settings` carrying a few hooks and a status line (they merge
with your own; nothing in `~/.claude` is written). The hooks tell the daemon what a resident is
doing, and its sidebar line shows it: `●` busy, `✦` a permission prompt or a finished turn you
have not seen, `✧` a question it asked. A resident that needs you is gold. When it is not the
one on screen in a focused terminal, the shrine rings the bell and posts an iTerm2 notification
(OSC 9). `~/.config/gensokyo/config` can set `NOTIFY_BELL=off` or `NOTIFY_DESKTOP=off`.

The status line reports feed the rest: each sidebar line shows the model and context used, the
title over the screen has the directory, branch, model, effort, permission mode, cache hit rate
and cost, and the sidebar's foot has the account's 5-hour and weekly usage. Inside the resident,
gensokyo draws its own one-line status line; `STATUSLINE=user` in the config runs your own
`statusLine` command instead, with the same input.

## Spell cards

A **spell card** is a prompt in a file. `Ctrl-] c` asks which card, then who gets it —
everyone, everyone who needs you, everyone resting, or one resident — and types it into each
one's prompt as if you had typed it there. Four ship with gensokyo:

| Card | What it asks for |
|---|---|
| `Spirit Sign "Status Report"` | three lines from every resident: what it is on, where that stands, what is next |
| `Border Sign "Sync Up"` | every resident tells the others what it is on, and raises any overlap with the one it affects |
| `Review Sign "Second Opinion"` | one resident asks another to review its uncommitted diff, and iterates until it hears LGTM |
| `Time Sign "Wrap Up"` | summarize the session, leave the tree clean, then go quiet |

Your own go in `~/.config/gensokyo/spellcards/<name>.md`, and a card of yours shadows a shipped
one of the same name. The name is letters, digits, `.`, `_` and `-`, starting with a letter or
digit. A little frontmatter is optional; the rest is the prompt:

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

`{self}` is the resident's own name, `{cwd}` its directory, `{residents}` the other live
residents (or `nobody`), and `{peer}` the resident you pick after the target. A card with
`peer: required` is a **pair card**: it goes to one resident, and the shrine asks whom it talks
to. A frontmatter line that means nothing (`pear: required`) keeps the card from being cast,
and the picker says why.

Residents talk to each other with `SendMessage`, and Claude Code keeps no thread of an
exchange, so a card that asks for an answer has to say that **the answer comes back as a
`SendMessage` addressed to `{self}`**. Told only what the reply should look like, the resident
asked writes it into its own screen, where the one waiting never sees it.

Casting never types into a resident that has something open — a permission dialog, a
question of its own, the trust dialog of a new directory — because the Enter after the card
would answer the dialog. Those residents are named and left out, and a card that does not show
up in a resident's prompt is reported as not sent:

```
$ gensokyo broadcast status-report all
cast Spirit Sign "Status Report" on Reimu and Sakuya; Marisa has a dialog waiting for you; left out
```

## Rituals

A ritual is standing work on a schedule: a markdown file that says when to run, where, and what
to ask. The daemon's clock checks the schedules every twenty seconds, and a ritual that has come
round is summoned as a resident of its own, which you can watch, take over or ignore. The
resident on screen keeps the focus.

```
~/.config/gensokyo/rituals/slack-morning.md

---
name: slack-morning
description: Morning Slack triage
schedule: "3 9 * * 1-5"     # five cron fields, local time; also @hourly @daily @weekly, "every 30m"
cwd: ~/dev/mozart
model: haiku
allowed_tools: ["Read", "Grep"]
enabled: true
---
Check Slack for anything addressed to me since your last run, summarize what
needs a reply, and list the open questions.
```

The shortest way to one is to ask a resident: *"every weekday at 9:05 check Slack for messages
to me and summarize them"*. Its `gensokyo-ritual` skill says back what it is about to schedule,
where, and what it will need permission for, and writes the file once you agree. "Pause that"
and "what have I got scheduled?" work the same way.

- **`target`**: `new` (the default) is a fresh session per fire. `persistent` keeps one session
  for the ritual: the first fire starts it and every later fire is typed into it, recalling it
  first if it has left. `target: <name>` types the prompt into a resident you run yourself.
  A fire that cannot be delivered says so in the journal and as a notification.
- **`headless: true`**: no resident at all; the run is a `claude -p` in the background, what it
  said goes to a log beside the ritual's notes, and a run still going after an hour is stopped.
  Nobody is there to answer a permission prompt, so what it needs goes in `allowed_tools`; the
  log names any tool it was refused.
- **`keep`**: how long a finished run's resident stays, `2h` by default; idle time, so typing
  in it starts the count again. `keep: forever` leaves it to you.
- **`overlap`**: a fire while the last run is still going is skipped; `parallel` runs a second
  one beside it and `queue` runs it as soon as the first is done (one deep, dropped after an
  hour).
- **`catch_up`**: a fire missed while the machine slept or the daemon was down is made up once,
  for the newest miss within a week; `catch_up: false` makes up nothing.

Every fire's prompt names the ritual's own `memory.md`, so a fresh session per run still knows
what it handled last time.

By hand, or to see what is there:

```sh
gensokyo ritual                      # what is scheduled, when each fires next, and why one is not firing
gensokyo ritual new nightly-checks   # a commented template in $EDITOR; it arrives paused
gensokyo ritual run nightly-checks   # fire it now: the way to approve its prompts once
gensokyo ritual log nightly-checks   # its fires, skips and complaints, and the newest headless log
gensokyo ritual disable slack-morning
gensokyo ritual remove slack-morning # the file, its notes and its journal, for good
```

`Ctrl-] t` is the **timetable**: every ritual with when it fires next; pick one for its
schedule, its last run, what is wrong with it if anything, and run now, pause or resume, and
remove (which asks). The ritual that fires next is always on the sidebar's `⏲` line.

Three examples ship paused in `share/rituals/`: `slack-morning`, `nightly-checks` and
`inbox-zero`. `gensokyo ritual edit slack-morning` makes a copy of yours and opens it; each
names a directory that is not on your machine, which is the line to change first. Run a new
ritual by hand once before leaving it to the clock. A directory Claude Code has never been
trusted in is refused with the reason: a run that stops at the trust dialog would sit there,
and every later fire would skip itself as still going.

Rituals fire only while the daemon runs, and on a machine that is awake. A laptop whose lid was
shut through a fire makes it up at the next tick after it wakes.

## What residents are told

Almost nothing, on purpose: a resident's context window is your budget. Each is launched,
without touching `~/.claude`, with `--plugin-dir share/plugin` and a short
`--append-system-prompt`: the name it lives under, that the first message of an exchange with
another session is the `gensokyo-peers` skill's to write, and that standing schedules are
gensokyo's, not Claude Code's own. The two skills carry what a click cannot: `gensokyo-peers`
writes an opening message that says who is asking, what is wanted, the round cap and the reply
address; `gensokyo-ritual` turns "every weekday at 9:05" into a ritual file. Only a skill's
description sits in a resident's context; the rest is read when it is used.

## Files

- `~/.config/gensokyo/`: `config` (`KEY=value` lines), `spellcards/`, `rituals/`.
  `GENSOKYO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/gensokyo`.
- `~/.local/state/gensokyo/`: `residents/` and `departed/` (one JSON record each),
  `daemon.log`, `run/` (the socket and locks) and each ritual's notes and journal.
  `GENSOKYO_STATE_DIR` moves it.

## Developing

```sh
scripts/ghostty.sh                       # once, and again whenever its pin changes
cargo nextest run                        # the suite, about 25 s (cargo test works too, slower)
cargo clippy --all-targets -- -D warnings
cargo fmt
```

CI runs the same three on macOS. The tests start real daemons on the stub in
`tests/stub-claude`, which stands in for `claude` (hooks, the registry, `/exit`), each in a
state directory of its own under `target/`, so nothing touches yours and no Claude Code login is
needed. A changed screen snapshot fails its test and leaves a `.snap.new` beside the old one in
`tests/snapshots/`; read it, then `cargo insta review` (cargo-insta) or rename it over the old.

- `src/daemon/`: the daemon, on one thread: the shrine, each resident's PTY and emulator
  (libghostty-vt), the socket, hooks, spell cards and the ritual clock.
- `src/client/`: the shrine's screen (ratatui), keys, mouse and selection.
- `src/cli/`: the commands; `src/proto.rs` is the socket's NDJSON.
- `src/vt.rs`: the emulator adapter; `src/ritual/` and `src/card.rs` the two file formats.

`GENSOKYO_CLAUDE` points at another `claude`, `GENSOKYO_CLIENT_LOG` writes the client's own
trace, and `daemon.log` has one JSON line per event, panics included.
