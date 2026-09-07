---
# An example, and paused. `gensokyo ritual edit slack-morning` takes a copy of it for you and
# opens that; change `cwd` to a directory you work in, then `gensokyo ritual enable
# slack-morning`. Run it once by hand first (`gensokyo ritual run slack-morning`) so that
# anything it asks permission for is answered while you are sitting there.
#
# This one reads Slack, so the directory it runs in needs a Slack MCP server configured for it.
name: slack-morning
description: what was said to me on Slack overnight, in one summary
schedule: "5 9 * * 1-5"           # weekdays at 09:05
cwd: ~/dev/yourproject            # yours: a directory Claude Code is already trusted in
model: haiku                      # a summary is not work for a big model
allowed_tools:
  - mcp__claude_ai_Slack__*
enabled: false
---

Read the Slack messages from the last day in the channels I am in: the ones that mention me,
that are addressed to me, or that are in a thread I have replied to.

Then write, in this pane and nowhere else, the shortest useful summary of them:

- who is waiting on me, what for, and a link to the message, longest wait first;
- anything that has been decided that I have not read yet, one line each;
- nothing else. If nobody needs me, say so in one line rather than padding it out.

Add one line to your notes with today's date and how many messages needed me, so that a later
run can say whether it is getting worse.
