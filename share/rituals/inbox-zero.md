---
# An example, and paused. `gensokyo ritual edit inbox-zero` takes a copy of it for you and
# opens that; change `cwd` to a directory you work in, then `gensokyo ritual enable inbox-zero`.
# Run it once by hand first (`gensokyo ritual run inbox-zero`) so that anything it asks
# permission for is answered while you are sitting there.
#
# This one reads your email, so the directory it runs in needs an email MCP server configured
# for it (Gmail, Front, Microsoft 365 - whichever one you use).
name: inbox-zero
description: the morning's unread mail, sorted into what needs me and what does not
schedule: "0 8 * * 1-5"           # weekdays at 08:00
cwd: ~/dev/yourproject            # yours: a directory Claude Code is already trusted in
model: haiku
enabled: false
---

Read my unread mail from the last day and sort it into three lists, written in this pane:

1. needs a reply from me, soonest first - who, what they asked, and how many words the answer
   probably is;
2. needs me to know it, but not to answer - one line each;
3. neither, and can be archived - senders and counts only, not one line per message.

Write no drafts and send nothing. Archive nothing, mark nothing as read. This is a reading of
the inbox, not a tidying of it: the decisions are mine and I will make them from your lists.

Keep in your notes the senders whose mail landed in list 3 for a week running, so that you can
tell me which subscriptions I would not miss.
