---
name: gensokyo-ritual
description: Required whenever the user asks for something to happen on a schedule or again and again - "every weekday at 9:05 check Slack", "run the tests nightly", "remind me every Monday", "do this daily", a cron line, "what have I got scheduled?", "pause that", "stop it running". Use it instead of the built-in schedule skill, CronCreate and scheduled tasks: on this machine a schedule is a gensokyo ritual, and this covers writing one, checking it, pausing it and reading what it has done.
---

# Scheduling work: rituals

You are running inside gensokyo, and a schedule here is a **ritual**: a file with a cron line
and a prompt in it. When its minute comes round the cockpit's clock starts a fresh Claude Code
session in a tab of its own, types the prompt into it and leaves it running where the user can
watch it, answer it and read it afterwards.

Do not use the built-in `schedule` skill, `CronCreate`, or scheduled tasks for this. Those are
somewhere else's clock; the user asked for something on **this** machine, and the tabs, the
notifications and the pausing all belong to gensokyo.

## Confirm before you create anything

A ritual runs unattended, so a wrong one is wrong every morning. Say back what you are about to
create - in one short block, not a form - and get a yes first:

- **what it will do**, in the words you are going to put in the prompt;
- **when**, as a time and days, not as a cron line ("weekdays at 09:05");
- **which directory** it runs in - the current one unless the user says otherwise;
- **what it will need permission for**, if anything (see below).

If any of the four is missing from what the user said, ask for that one thing rather than
guessing it. "Every morning" is a time you have to ask about; "in this project" is not.

## Then write it

The prompt is many lines, so put it in a file and hand that over:

```bash
cat > /tmp/slack-morning.txt <<'PROMPT'
Read the Slack messages from the last day that mention me or are addressed to me.

Write in this pane, and nowhere else, who is waiting on me and what for, longest wait first,
with a link to each message. If nobody needs me, say so in one line.
PROMPT

gensokyo ritual add --name slack-morning --schedule '5 9 * * 1-5' \
  --cwd /Users/me/dev/mozart --description 'overnight Slack, in one summary' \
  --model haiku --allowed-tools 'mcp__claude_ai_Slack__*' \
  --prompt-file /tmp/slack-morning.txt
```

- `--name` becomes the file name and the tab's name: letters, digits, `.` `_` `-`.
- `--schedule` takes five cron fields, `@hourly` `@daily` `@weekly` `@monthly`, or
  `every 30m`. It is checked before anything is written, and a schedule that never comes round
  (30 February) is refused.
- `--cwd` must be a directory that exists; leave it out for the one you are in.
- `--allowed-tools` can be repeated, or given a comma-separated list. Anything not on it stops
  the run at a permission dialog until the user answers, which for a 02:00 run means until
  morning - so name the tools the prompt is going to need. `--mode acceptEdits` is the blunter
  way; `--model haiku` is worth it for anything that is only reading and summarising.
- `--disabled` writes it without turning it on, for a ritual the user wants to look at first.

**Read what the command says back and pass it on.** It prints the next fire, and it warns -
having written the file - when the directory is one Claude Code has never been trusted in. That
warning matters: a run there stops at the trust dialog with nobody to answer it, and the fix is
for the user to open that directory once themselves. Never leave it out of what you report.

## Writing the prompt itself

The session that runs it has none of this conversation. It gets your text, the directory, and a
notes file of its own that gensokyo names in the prompt for it. So:

- write it to that session, in the second person, as a complete instruction;
- say where the answer goes (its own pane is the normal answer, and the user reads it there);
- say what it must **not** do. An unattended run that commits, pushes, sends mail or archives
  things is the way a ritual becomes a thing the user regrets;
- tell it what to keep in its notes if a later run should know what an earlier one found.

## After it is written

Tell the user to fire it once by hand while they are sitting there:

```bash
gensokyo ritual run slack-morning     # now, whatever the schedule says; the schedule is untouched
```

That is when its permission prompts get answered, and it is much better than finding out at
09:05 on Monday. The clock picks the new ritual up on its own; nothing needs restarting.

## The other things the user will ask for

```bash
gensokyo ritual list --json           # what is scheduled, when each fires next, what is wrong with one
gensokyo ritual disable slack-morning # "pause it" - the file stays, the schedule stops
gensokyo ritual enable slack-morning  # and back on again
gensokyo ritual log slack-morning     # every fire, every run skipped, every complaint
gensokyo ritual edit slack-morning    # opens the file in the user's editor - for them, not for you
```

Read `list --json` before you answer "what have I got scheduled?" or change one: it carries
each ritual's `name`, `enabled`, `schedule`, `next_fire`, `last_run`, `cwd` and `problem`. A
`problem` is why that ritual is not firing, and it is the answer to "why didn't it run?".

To change what a ritual does, edit its file at the `path` the JSON gives - `add` refuses a name
that already exists rather than overwriting somebody's file.

## What it cannot do yet, and must not be promised

- **Rituals only fire while gensokyo is running.** A closed cockpit or a sleeping laptop means
  no fire; the next start runs what was missed once. Say so if the user is counting on
  something happening while the machine is off - that is a cloud routine, not a ritual.
- Every run gets a tab you can watch. Runs with no pane, a ritual firing into a resident that
  is already there, and expiry after a run are not wired up yet: `headless`, `target`,
  `overlap` and `keep` are refused with a word about why rather than half-working.
- A ritual is one prompt on a schedule. Anything that needs to talk to another resident is the
  `gensokyo-peers` skill, and anything the user wants to fire off by hand at several residents
  at once is a spell card (`gensokyo broadcast`).
