---
name: gensokyo-cloud
description: Required when the user wants something to run in the cloud, on claude.ai, as a routine, or while their Mac is off or asleep - "even when my laptop is closed", "a cloud routine that checks Slack every morning", "make it a routine", "what are my routines doing?", "pause that routine", "run it now", "what did the routine do?". Use it instead of the built-in schedule skill and RemoteTrigger. It makes a routine paused through `gensokyo cloud`, shows and pauses routines, and leaves resuming and running them to the user.
---

# Routines on claude.ai

A **routine** is a prompt that claude.ai runs on its own schedule, in a fresh cloud session.
That session gets the GitHub repositories it names checked out and the claude.ai connectors it
is given (Slack, Linear, …). It runs whether this Mac is on or not. It cannot reach anything
on this machine: no files, no residents, no probes, no gensokyo. It reports only through what it
does, such as a Slack message or a PR comment, so its prompt has to say where the report goes.

Never use the built-in `schedule` skill or `claude --cloud` for this. `RemoteTrigger` is turned
off for you, and you must not reach it any other way (a nested `claude -p`, say). Everything
goes through `gensokyo cloud`, which makes each routine paused and with only the connectors
named.

## A ritual or a routine

- **A ritual** (the `gensokyo-ritual` skill) runs here. Pick one when the job:
  - needs this machine's files or a checkout here;
  - needs a probe;
  - runs more often than hourly;
  - is something the user wants to watch and answer as a resident.
- **A routine** runs on claude.ai. Pick one when the job:
  - must run while the Mac is off or asleep;
  - is a routine the user asked for by name.
- **Either fits** a job that needs only connectors and repositories on GitHub. Offer the
  routine, and say the trade:
  - it runs with the Mac shut, but nobody can answer it while it runs;
  - each of its connectors' tools runs unasked, writes too.

## Confirm before you make one

Say back in one short block, then get a yes:

- **what it will do**, in the words of the prompt;
- **when**, as a time and days on this machine's clock. Never convert it to UTC: gensokyo does
  that.
- **the repositories and connectors** it gets. Every tool of each connector runs without
  asking, posting and sending included: the routine's allowed tools do not limit them.
- **where it reports**: a channel, a DM, a PR comment.

## Make it

Write the prompt to a file first. The run starts with nothing else, so make it self-contained:
- what to read;
- what to do;
- where to post the result;
- what never to do (no posting anywhere else, say).

```sh
gensokyo cloud add standup-digest --schedule "0 9 * * 1-5" --prompt-file /tmp/digest.md \
  --connector Slack [--repo org/app] [--model claude-sonnet-5-5]
```

- **The schedule** is cron in local time, as for a ritual.
  - It runs at most hourly.
  - gensokyo refuses a schedule that no UTC cron keeps all year. That happens when the zone
    changes its clocks, or when a time falls on another day in UTC and the schedule names
    dates or months: then offer days of the week, in every month.
- **The prompt** is at most 4000 characters.
- **It arrives paused.** Tell the user it is ready and that `gensokyo cloud resume <name>` starts
  it: that command shows them what it does and asks. You cannot resume or run a routine, and
  you must not ask the user to pass `--yes` without reading it.
- **Errors you hand to the user.** Your file tools are refused in gensokyo's config dir.
  - "no cloud environment" or "no connector X in the config": the user adds a line,
    `CLOUD_ENV=env_…` or `CLOUD_CONNECTOR_<Name>=<uuid> <url>`, to `~/.config/gensokyo/config`.
    Claude Code's `/schedule`, run outside gensokyo, lists both ids.
  - "the Claude GitHub App": it goes on each repository the routine names.

## Look after them

```sh
gensokyo cloud                   # each routine, its schedule, next run and newest run's link
gensokyo cloud show NAME         # its schedule, model, repos, connectors and prompt
gensokyo cloud pause NAME        # fires no more until the user resumes it
```

- `gensokyo cloud` lists what the **cloud watch** last saw. The watch is off until the user runs
  `gensokyo cloud watch on`, since each look spends a little of their plan. With it off, a
  routine is named by its id (`trig_…`) rather than its name.
- **Give the user the link** to a run rather than opening it. `gensokyo cloud open` opens their
  browser.
- **Following up on a run is the user's**: `gensokyo cloud say NAME "<words>"` makes them its
  next turn. Suggest the words; they send them. So is taking one over here as a resident
  (`gensokyo new --teleport NAME`).
- **A run's log is the user's** (`gensokyo cloud log NAME`). It quotes whatever the run read,
  and that stays out of residents.
- **There is no edit and no delete.** Both are on the web, at claude.ai/code/routines. To
  change a prompt, make a new routine and have the user delete the old one there.
