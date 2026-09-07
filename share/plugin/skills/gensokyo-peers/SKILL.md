---
name: gensokyo-peers
description: Required before you send another resident the first message of an exchange you want an answer to - asking one to review a diff, give an opinion, or take a hand-off. Also use it when a reply you are waiting on has not arrived. It covers what that first message has to say for the answer to reach you instead of being written into their own pane.
---

# Talking to another resident

The other Claude Code sessions on this machine are residents of the same cockpit. `ListAgents`
names them; so does `gensokyo list`. You reach one with `SendMessage` addressed to its name.

Claude Code keeps no thread. Every message you send arrives as a fresh turn with none of this
conversation attached, so the exchange only holds together if you carry it yourself.

## The opening message has to say five things

1. **who is asking** - your own resident name, because the other side sees a message, not a
   sender it knows;
2. **what you want** - a review of the uncommitted diff in a named absolute directory, an
   opinion on a specific question. Point at a path, never at "my changes";
3. **the reply shape** - "either the single word `LGTM` or a numbered list of findings";
4. **the reply channel** - *that the reply must come back as a `SendMessage` addressed to
   you, by name*;
5. **the cap** - "this exchange is capped at 3 rounds".

Item 4 is the one that looks like a broken tool when it is missing. A resident that receives a
request treats it the way it treats a request from a user, and answers where a user would read
it: in its own pane. Its findings will be correct, complete, and invisible to you, and you will
wait for a reply that was already given. If you are waiting on a peer and nothing has arrived,
this is the first thing to suspect - and you cannot fix it by asking again the same way.

## Keeping it together

- Tag every message after the first `[<topic> round k/N]`, using the same topic throughout, so
  the other side reads it as the same exchange rather than a new request.
- Answer what came back. If it found something, fix it and say what you changed; if you disagree,
  say why rather than complying - a peer is a reviewer, not an authority.
- Stop at the first of: it says `LGTM`, the cap is reached, or either of you needs the user.
- Then tell the user, in your own pane, what was asked, what came back and where it landed. The
  user watched none of it.

## What goes wrong

- **An unknown or renamed name fails cleanly and non-fatally**: `Not sent - no agent named 'X'
  is reachable.` Your turn continues. Use `ListAgents` and try the name you find. A resident
  renamed since you last heard of it is indistinguishable from one that never existed.
- **A resident awaiting the user** will not read your message until the user has seen to it. Say
  so to the user rather than sending again.
- **A message that arrives mid-turn is appended to that turn**, not deferred, so a reply may
  come back faster than you expect and interleave with what the peer was doing.

## When not to use this

Saying the same thing to everybody is a spell card, not a conversation: `gensokyo broadcast`,
or the cockpit's `[ cast ]` button. If the user wants this exchange again later, the thing to
write is a card in `~/.config/gensokyo/spellcards/` - a file whose body is the opening message,
with `{self}`, `{peer}`, `{cwd}` and `{residents}` filled in when it is cast.
