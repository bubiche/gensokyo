# gensokyo

Several Claude Code sessions side by side, one shown at a time. Each session is a **resident**
of the shrine: a sidebar lists them all with what each is doing, the one you pick fills the rest
of the screen, and a resident that needs you — a permission prompt, a question, a finished turn
nobody has read — turns gold and rings. Prompts can be cast at several residents at once
(**spell cards**) and fired on a schedule (**rituals**), and a resident can run helpers of its
own and report back.

One Rust binary: a daemon that owns every resident's terminal, and a client that draws the
shrine. It needs macOS on Apple silicon and is made for iTerm2; other terminals mostly work.

## Requirements

- macOS on Apple silicon, and Claude Code (`claude` on `PATH`). This release was tested on
  2.1.289 to 2.1.291; nothing checks the version.
- iTerm2 3.5 or newer is the terminal it is made for. Terminal.app works without the kitty
  keyboard protocol, so Shift+Enter and a few other chords are not told apart there.
- For the PR watcher only: `gh`, logged in, and `jq` (macOS 15 and later ship it), and a
  claude.ai login for the page.
- To build it yourself: Rust (the version in `.tool-versions`), Zig **0.16.0** exactly (the
  pinned Ghostty builds with nothing else; `zig version` says which you have), and git.

## Installing

```sh
curl -fsSL https://github.com/bubiche/gensokyo/releases/latest/download/install.sh | sh
```

That downloads the latest release, checks it against the release's `SHA256SUMS`, unpacks it
into `~/.gensokyo` and links `~/.local/bin/gensokyo` (it says so if that is not on your `PATH`).
Options go after `sh -s --`: `--version 0.2.0`, `--dir DIR`, `--bin-dir DIR` (or
`GENSOKYO_VERSION`, `GENSOKYO_DIR`, `GENSOKYO_BIN_DIR`). Nothing is written anywhere else.

- `gensokyo doctor` says which binary and which `claude` are in use, whether the daemon and the
  login agent are running, and what looks wrong.
- `gensokyo login setup` starts the daemon at every login, so rituals fire on a day you never
  open a terminal, and launchd starts it again if it crashes. It takes this shell's `PATH`, which
  is how the daemon finds `claude`: run it again after `claude` moves. Only that `PATH`,
  `CLAUDE_CONFIG_DIR` and gensokyo's own location variables carry over (the agent's file is
  readable by anyone), so a variable such as `ANTHROPIC_BASE_URL` or a proxy set in your shell
  does not reach residents once launchd starts the daemon; `doctor` names any it sees.
  `gensokyo login` says whether it is on; `gensokyo login remove` turns it off. A daemon
  `gensokyo quit` stopped stays stopped until the next login or the next `gensokyo`.
- `gensokyo update` swaps `~/.gensokyo` for the latest release (`--check` only says whether there
  is one, `--version` picks one). The running daemon keeps the old binary until `gensokyo
  restart`.
- `gensokyo uninstall`, with the daemon stopped (`gensokyo quit` first), lists the install, its
  link, the login agent and your config and state, and asks once before removing them
  (`--keep-data` keeps the config and state, `--yes` doesn't ask). With the binary already gone,
  `curl -fsSL …/releases/latest/download/uninstall.sh | sh` lists what is left, and
  `sh -s -- --yes` removes it. Claude Code's own settings and sessions are never touched.

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
and on to the resident. A resident's scrollback is its own, so two clients showing it scroll
together.

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
all back under recall. Each of these is also a command, for scripts (below).

## When a resident needs you

Every resident is launched with `--settings` carrying a few hooks and a status line (they merge
with your own; nothing in `~/.claude` is written). The hooks tell the daemon what a resident is
doing, and its sidebar line shows it: `●` busy, `✦` a permission prompt or a finished turn you
have not seen, `✧` a question it asked. A resident that needs you is gold. When it is not the
one on screen in a focused terminal, the shrine rings the bell and posts an iTerm2 notification
(OSC 9). `~/.config/gensokyo/config` can set `NOTIFY_BELL=off` or `NOTIFY_DESKTOP=off`.

The status line reports feed the rest: each sidebar line shows the model and context used, the
title over the screen has the directory, branch, model, effort, permission mode, cache hit rate
and cost, and the sidebar's foot has the account's 5-hour and weekly usage. The directory is
where Claude works now, which follows it into a worktree or a subdirectory, and its branch is on
a dim line under each resident in the sidebar while they all fit. `BRANCH_PREFIX=you/` in the
config leaves that prefix off there. Inside the resident,
gensokyo draws its own one-line status line; `STATUSLINE=user` in the config runs your own
`statusLine` command instead, with the same input.

## Spell cards

A **spell card** is a prompt in a file. `Ctrl-] c` asks which card, then who gets it —
everyone, everyone who needs you, everyone resting, or one resident — and types it into each
one's prompt as if you had typed it there. Four ship with gensokyo:

| Card | Its summary |
|---|---|
| `Spirit Sign "Status Report"` | three lines from every resident: what you are on, where it stands, what is next |
| `Border Sign "Sync Up"` | every resident tells the others what it is on, and raises any overlap with the one it affects |
| `Review Sign "Second Opinion"` | another resident reviews your uncommitted diff, and you iterate until it says LGTM |
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
would answer the dialog. Nor into one where you have half typed a prompt: the card would go in
with it, as one prompt, until you send that or clear it with Ctrl-C (a line emptied with
Backspace still counts, since gensokyo doesn't read it). Nor into one that another card or a
ritual's prompt is still going into, until its prompt is in. A busy resident still takes a
card, which Claude Code queues; if a dialog opens over the card before its Enter, the Enter is
held and the card stays in the input line, unsent. Those residents are named
and left out, and a card that does not show up in a resident's prompt is reported as not sent:

```
$ gensokyo broadcast status-report all
cast Spirit Sign "Status Report" on Reimu and Sakuya; Marisa has a dialog waiting for you; left out
```

From a shell, `gensokyo broadcast` alone lists the cards; the targets are `all`, `awaiting`,
`idle` or residents by name, slot or id, and `--with Sanae` names a pair card's peer.

## Rituals

A ritual is standing work on a schedule: a markdown file that says when to run, where, and what
to ask. The daemon's clock checks the schedules every twenty seconds, and a ritual that has come
round is summoned as a resident of its own, which you can watch, take over or ignore. The
resident on screen keeps the focus.

```
~/.config/gensokyo/rituals/slack-morning.md

---
name: slack-morning          # the file's name
description: Morning Slack triage
schedule: "3 9 * * 1-5"     # five cron fields, local time; or @hourly … @yearly, "every 30m", "every 2h"
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
where, and what it will need permission for, and writes the file once you agree. It arrives
paused: resume it with `gensokyo ritual enable` or the timetable, both of which show what each
run may do without asking (mode, tools, MCP config, probe). "Pause that" and "what have I got
scheduled?" work the same way.

- **`target`**: `new` (the default) is a fresh session per fire. `persistent` keeps one session
  for the ritual: the first fire starts it and every later fire is typed into it, recalling it
  first if it has left. `target: <name>` types the prompt into a resident you run yourself.
  `target: branch` (with `when`) types into whichever resident works on a branch, below.
  A fire that cannot be delivered says so in the journal and as a notification.
- **`deliver`** (with `persistent` or a resident's name): `idle`, the default, holds a fire
  until its resident is idle: its turn over (10 s since it was last heard from), no dialog or
  question open, nothing half typed into it. Whether it is on screen makes no difference. A
  newer fire takes the place of one still held, and 4 h after the first of them the fire is
  dropped, with what held it; the journal says `held`, then `sent` or `not sent`. When what
  holds it is yours to clear (a dialog, or text typed and not sent), a notification says so.
  Text typed and not sent counts until a prompt goes in, Ctrl-C, `/clear`, or the Enter that
  runs a `/` or `!` command. Fires for one resident go in one at a time, each after the turn
  the last one started. A resident that has not come up within a minute (not yet seen by
  Claude Code's session list, as at the trust dialog) is given up on. `deliver: now` types it
  in at once, mid-turn too, where Claude Code queues it, and gives up on a dialog instead of
  waiting.
- **`model`**, **`effort`**, **`mode`** (the permission mode), **`allowed_tools`** and
  **`mcp_config`** (a file of MCP servers, as `claude --mcp-config` takes) are what a run starts
  with. A ritual that reads Slack or mail needs its MCP server: a claude.ai connector, the
  `cwd`'s own Claude Code setup, or `mcp_config`.
- **`headless: true`** (with `target: new`): no resident at all; the run is a `claude -p` in the
  background, what it said goes to a log beside the ritual's notes, and a run still going after
  an hour is stopped. Nobody is there to answer a permission prompt, so what it needs goes in
  `allowed_tools`; the log names any tool it was refused.
- **`worktree`** (with `target: new`): every run works in that worktree of `cwd`'s repository,
  made the first time as `new --worktree` makes one, and in the same subdirectory as `cwd`. One
  checkout for all its runs, so not with `overlap: parallel`.
- **`keep`** (with `target: new`): how long a finished run's resident stays, `2h` by default
  (`30m`, `1d`); idle time, so typing in it starts the count again. `keep: forever` leaves it to
  you.
- **`overlap`** (with `target: new`): a fire while the last run is still going is skipped;
  `parallel` runs a second one beside it and `queue` runs it as soon as the first is done (one
  deep, dropped after an hour).
- **`catch_up`**: a fire missed while the machine slept or the daemon was down is made up once,
  for the newest miss within a week; `catch_up: false` makes up nothing.
- **`when`**: a probe, which turns the schedule into a polling interval. Each fire runs the
  probe first, in the ritual's `cwd` with a 60 s limit, and starts a run only when its output
  differs from what it printed for the last run that started. Output that has not changed
  starts nothing and goes in no journal. A probe that fails (a non-zero exit, the limit, over
  256 KB of output) starts one run, and its recovery another. The run reads the output from
  `probe.out` in the ritual's directory, and why it failed from `probe.err`; neither goes into
  the prompt (but see `target: branch`). The probe finds its own last output through
  `GENSOKYO_PROBE_LAST`. A probe runs outside Claude's permissions, so it must be an executable
  in the config dir's `probes/` or the shipped `share/probes/`, named without a path (`when:
  gh-prs`, or `when: "my-probe --flag"` with arguments), and not a link out of there. That only holds while no resident can write
  those directories: keep them outside every resident's `cwd` if it runs with edits accepted.
- **`target: branch`**: its probe prints one JSON object, `{"<repo>:<branch>": {facts}}`, the
  repo as the path of `origin`'s URL (`owner/repo`, matched ignoring case), and each branch's
  facts go to the resident working on that branch of that repo, a worktree or a `cd` included
  (`list --json` `here`). A fork or a renamed repo has its own path, so a key reaches only a
  clone whose `origin` is that repo. The prompt is the ritual's body with `{branch}`, `{key}`,
  `{facts}` (`ci: FAILURE@1a2b3c4, threads: 3`) and any `{_name}` filled in. A branch is told
  only when one of its facts is new, and then all of them are listed, the new ones first: the
  same facts again are not news, a fact going away sends nothing, and its return does. A held
  fire for a branch that has left the output is dropped. A fact named `_name` is context, never
  news. This is the one place a probe's output reaches a prompt, so a value must be a short
  token (letters, digits, `_.:/#@-`, at most 200) and a name lowercase; anything else is left
  out, each journaled once. Tokens can still spell words, so a probe passes only what it works
  out itself (states, counts, hashes, numbers), never a name or label someone else chose. A
  probe that cannot find out exits non-zero rather than print a partial or empty answer: a
  branch missing from it has its next facts all new. Nothing ships for it; the ritual skill has
  a GitHub-PR probe to copy.
- **`quiet: true`** (with `target: new`): its runs' finished turns neither ring nor turn gold.
  Their permission prompts still do.

A ritual whose settings don't fit together (a `headless` one with `target: persistent`, a
`cwd` Claude Code was never trusted in) does not fire, and `gensokyo ritual` and the timetable
say why. A run that stopped at the trust dialog would sit there, and every later fire would skip
itself as still going.

**Watching your pull requests.** Ask a resident to *"watch my PRs"*. It publishes a private
page on claude.ai, and adds a ritual on the shipped `gh-prs` probe, which asks GitHub every five
minutes. When something has changed, a short quiet haiku run copies the new list to the page,
and an open page redraws by itself, on the phone too. The page groups your open pull requests by
what each needs from you: failing checks, changes requested or conflicts first, then ready to
merge, then waiting on others. The list carries the day's date, so there is one run a day even
when nothing changed, and a page whose date is two days old says it has not been checked since.
While GitHub can't be reached the probe prints the last list as it was, which starts nothing.

Every fire of `new` or `persistent` names the ritual's own `memory.md` in its prompt, so a fresh
session per run still knows what it handled last time; every run may write it. A resident named
by `target` gets the prompt alone.

By hand, or to see what is there:

```sh
gensokyo ritual                      # what is scheduled, when each fires next, and why one is not firing
gensokyo ritual new nightly-checks   # a commented template in $EDITOR; it arrives paused
gensokyo ritual add --name standup --schedule "0 9 * * 1-5" --cwd ~/dev/x --prompt-file - <<'EOF'
…
EOF
gensokyo ritual run nightly-checks   # fire it now, to see what it stops to ask
gensokyo ritual log nightly-checks   # its fires, skips and complaints (-n N, --all), and the newest headless log
gensokyo ritual disable slack-morning  # and enable
gensokyo ritual remove standup       # the file, its notes and its journal, for good
```

`ritual add` takes every setting above as a flag (`gensokyo help ritual add`); it is what the
skill runs. `gensokyo ritual list --json` is the listing for scripts.

`Ctrl-] t` is the **timetable**: every ritual with when it fires next; pick one for its
schedule, its last run, what each run may do without asking, what is wrong with it if
anything, and run now, pause or resume, and remove (which asks). The ritual that fires next is
always on the sidebar's `⏲` line.

Three examples ship paused in `share/rituals/`: `slack-morning`, `nightly-checks` and
`inbox-zero`. `gensokyo ritual edit slack-morning` makes a copy of yours and opens it; each
names a directory that is not on your machine, which is the line to change first. A shipped
example can't be removed, only disabled; removing your copy brings the paused original back.
Run a new ritual by hand once before leaving it to the clock, and add what it asks permission
for to its `allowed_tools`: each fire is a fresh session, and an answer given to one does not
carry over.

Rituals fire only while the daemon runs, and on a machine that is awake. A laptop whose lid was
shut through a fire makes it up at the next tick after it wakes.

## Helpers

Ask a resident to *"get a couple of helpers to …"* and its `gensokyo-lead` skill summons other
residents for the parts, briefs each, waits on them in the background, reads their reports and
closes them. A resident it summons is its **helper**, and it the helper's **lead**; the sidebar
draws helpers under their lead with `└`.

**Worktrees.** `gensokyo new <dir> --worktree <name>` summons into
`<repo>/.claude/worktrees/<name>` under the main checkout (from inside another worktree too),
where Claude Code keeps its own, on branch `<name>` (with
`BRANCH_PREFIX` from the config in front, or `--branch`). An existing worktree of that name is
reused. A branch only the remote has is checked out tracking it, to review a pull request (a
single-branch clone gets it added to `origin`'s fetched branches); a new
one starts from the remote's default branch, fetched first, or `--base`. Started from a
subdirectory of the repository, the resident works in the same one inside the worktree. A
branch checked out elsewhere is refused, and so is a worktree whose directory is gone or whose
checkout was cut off, with the git command that clears it. The summon dialog asks for a worktree after the name
when the directory is in a git repository; leaving it empty works right there. gensokyo never
removes a worktree: `git worktree remove` is yours. It does not use `claude --worktree`, whose
`/exit` stops at a keep-or-remove question. A lead's skill gives each helper its own this way.

A lead has at most 5 live helpers (`HELPERS=` in the config), and a helper can't summon, though
it keeps Claude Code's own subagents. A helper's finished turn is quiet while its lead waits on
it or is busy, since the lead will read it; it rings once the lead's turn ends without having
done so. Its permission prompts and questions always ring: only you can answer them. A helper
whose lead has left for good is closed after two hours idle; a lead recalled before then keeps
it.

## What residents are told

Almost nothing, on purpose: a resident's context window is your budget. Each is launched,
without touching `~/.claude`, with `--plugin-dir share/plugin` and a short
`--append-system-prompt`: the name it lives under, that the first message of an exchange with
another session is the `gensokyo-peers` skill's to write, that standing schedules are
gensokyo's (Claude Code's own `CronCreate`, `CronList` and `CronDelete` are turned off for
residents and headless runs), and that helpers are the `gensokyo-lead` skill's. A helper hears
instead who its lead is, and that its last message is its report. The three skills carry what a
click cannot: `gensokyo-peers` writes an opening message that says who is asking, what is
wanted, the round cap and the reply address; `gensokyo-ritual` turns "every weekday at 9:05"
into a ritual file; `gensokyo-lead` briefs helpers, waits on them and reads their reports. Only
a skill's description sits in a resident's context; the rest is read when it is used.

## Scripting

Everything the shrine does is also a command, and the commands are the interface for scripts
and for the residents' own skills:

```sh
gensokyo list [--all] [--json]          # who is here: slot, state, directory, model, context
gensokyo new ~/dev/x -n Marisa -m haiku # summon; also -e effort, -p permission mode, --prompt,
                                        #   --prompt-file FILE|-, --allowed-tools TOOL, --json
gensokyo new ~/dev/x --worktree fix     # in a worktree of its own (below); --branch, --base
gensokyo resume Marisa                  # bring a departed one back (list --all shows them)
gensokyo banish Marisa                  # hang up: HUP, then TERM, then KILL
gensokyo close Marisa                   # ask it to /exit; a departed one leaves the sidebar
gensokyo broadcast status-report all    # cast a spell card
gensokyo wait Marisa Sanae --any        # until they have news: a turn ended, a dialog, gone
gensokyo read Marisa [--screen]         # its last answer, kept after it leaves; or its screen
gensokyo ritual …                       # the rituals (above)
gensokyo quit                           # everyone /exit, then the daemon stops
gensokyo restart                        # a new daemon, the same residents
gensokyo help <command>                 # every command has its own
```

A resident is named by its name, its slot or its id. `list --json` is an array with one object
per resident: `id`, `name`, `slot`, `cwd` (where it was started), `here` (where it works now,
when that is elsewhere), `pid`, `state` (`busy`, `awaits`, `asked`, `resting`
or `departed`), `detail` (what it waits for), `blocked` (why nothing may be typed into it now),
`finished`, `owner` (its lead's id), `turns` and `needs` (turns ended and dialogs opened, ever),
`mode`, `branch` (at `here`), `telemetry`, and for a departed one `departed` (epoch seconds), `exit` and
`signal`. `new --json` prints the new resident the same way.

`wait` holds until each resident named has news (`--any`: one of them), or only the kind
`--until done|needs|gone` names, for at most `--timeout` (`30m`). It prints one JSON line per
resident: `id`, `name`, `state`, `turns`, `needs`, `news` (whether it had any), and how its last
turn `ended` (`stop`, `failed`, `interrupted` or `unreported`) with the first line of its
`answer`; `read` has the whole answer. From your shell a wait counts from the moment it starts.
From a lead about its own helpers it counts from what the lead was last told, so a turn that
ended before the wait still counts.

Commands exit 0 when done, 1 with a `gensokyo:` line on stderr when not, and 2 on a usage
error; `wait` exits 3 on its timeout and 4 when the daemon went away (a restart: wait again).

Run from inside a resident, these have a resident's rights rather than yours. Residents get
`GENSOKYO_RESIDENT` (their id) and `GENSOKYO_SOCKET` in their environment, and that is how the
daemon tells them apart. A resident can list, summon, cast and keep rituals as its skills do.
It can close, banish, recall, `wait` on or `read` only its own helpers, and it can't `quit` or
`restart`. A helper it summons works in its permission mode or a narrower one, and may be let
use only `Read`, `Glob`, `Grep`, `WebSearch` and `WebFetch` without asking. A ritual it adds
arrives paused, and only you resume, run or `ritual edit` one; its MCP config must already be
in the config dir. Its file tools are refused in the config dir, where rituals, probes and MCP
configs run later with nobody watching. It can't type into, show or resize any screen either:
the screens and keyboards are yours. A card it casts at `all` leaves it out, and the helpers of
other residents. Its `new` refuses a directory Claude Code was never trusted in, because nobody
would be there to answer the trust dialog. The daemon runs as you, so these rights keep a
resident from slips, not from setting out to get round them.

Under the commands is a socket (`run/gensokyo.sock`, one JSON object per line, `src/proto.rs`).
It is not an interface: its protocol changes between builds, and a client speaking another
version of it is refused. Script against the commands.

## Files

- `~/.config/gensokyo/`: `config` (`KEY=value` lines), `spellcards/`, `rituals/`, `probes/`.
  `GENSOKYO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/gensokyo`.
- `~/.local/state/gensokyo/`: `residents/` and `departed/` (one JSON record each), `answers/`
  (each resident's last answer), `daemon.log`, `run/` (the socket and locks), and each ritual's
  notes, journal and probe output. `GENSOKYO_STATE_DIR` moves it. `daemon.log` moves to
  `daemon.log.1` past 5 MB. A departed record, and its answer, is forgotten once it is more than
  30 days old and over 100 others have left since; a ritual keeps its newest 50 runs.

## Building and developing

```sh
git clone https://github.com/bubiche/gensokyo && cd gensokyo
scripts/ghostty.sh                       # the pinned Ghostty source into vendor/, checked; again whenever its pin changes
cargo build --release                    # target/release/gensokyo, linked against system libraries only
cargo nextest run                        # the suite, about 25 s (cargo test works too, slower)
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Put `target/release/gensokyo` on your `PATH`; a symlink to it is fine, and finds the shipped
names, cards and rituals in the checkout it was built in (`GENSOKYO_SHARE` points elsewhere).
After a rebuild, `gensokyo restart` replaces the running daemon with the new binary and brings
every resident back into its own conversation. Run it from a terminal of your own, not from
inside a resident.

CI runs the suite, clippy and fmt on macOS. The tests start real daemons on the stub in
`tests/stub-claude`, which stands in for `claude` (hooks, the registry, `/exit`), each in a
state directory of its own under `target/`, so nothing touches yours and no Claude Code login is
needed. A changed screen snapshot fails its test and leaves a `.snap.new` beside the old one in
`tests/snapshots/`; read it, then `cargo insta review` (cargo-insta) or rename it over the old.
The install, update and uninstall tests package this build with `scripts/release.sh` and run
the shipped scripts in a home of their own, with a stand-in `launchctl`. One test loads a real
launchd agent under a `dev.gensokyo.test.*` label and boots it out again: it is left out unless
asked for, with `cargo nextest run --test login --run-ignored ignored-only`.

A release is a `v<version>` tag matching `Cargo.toml`: the release workflow builds the arm64
tarball with `scripts/release.sh`, installs it into a scratch home, and publishes it with
`SHA256SUMS`, `VERSION`, `install.sh` and `uninstall.sh`. `scripts/release.sh` alone builds the
same into `dist/`.

- `src/daemon/`: the daemon, on one thread: the shrine, each resident's PTY and emulator
  (libghostty-vt), the socket, hooks, spell cards, helpers and the ritual clock.
- `src/client/`: the shrine's screen (ratatui), keys, mouse and selection.
- `src/cli/`: the commands; `src/proto.rs` is the socket's NDJSON.
- `src/vt.rs`: the emulator adapter; `src/ritual/` and `src/card.rs` the two file formats.

`GENSOKYO_CLAUDE` points at another `claude`, `GENSOKYO_CLIENT_LOG` writes the client's own
trace, and `daemon.log` has one JSON line per event, panics included.

## License

MIT (`LICENSE`). A release also carries Ghostty's license (`LICENSES/ghostty.txt`), whose
terminal emulator is built into the binary.
