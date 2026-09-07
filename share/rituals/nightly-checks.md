---
# An example, and paused. `gensokyo ritual edit nightly-checks` takes a copy of it for you and
# opens that; change `cwd` and the two commands to your project's own, then `gensokyo ritual
# enable nightly-checks`. Run it once by hand first (`gensokyo ritual run nightly-checks`) so
# that anything it asks permission for is answered while you are sitting there - a run at 02:00
# that stops at a permission prompt waits for you until morning.
name: nightly-checks
description: the tests and the linter overnight, and what broke since last night
schedule: "0 2 * * *"             # every night at 02:00
cwd: ~/dev/yourproject            # yours: a directory Claude Code is already trusted in
allowed_tools:                    # so the run does not stop to ask about these
  - Read
  - Grep
  - Glob
  - Bash(npm test:*)
  - Bash(npm run lint:*)
enabled: false
---

Run the project's tests and its linter, one after the other, and read the output.

Then write in this pane: what passed, what failed, and for each failure the file and line and
your one-sentence guess at why. Compare it with your notes: say which failures are new
tonight, which are the same as last night, and which have gone.

Change nothing. Do not edit files, do not commit, do not push. Your job here is to find out
what state the project is in while nobody is looking at it, and to say so.

Then replace your notes with tonight's list of failures, so that tomorrow's run has something
to compare against.
