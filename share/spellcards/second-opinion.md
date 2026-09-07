---
title: Review Sign "Second Opinion"
summary: another resident reviews your uncommitted diff, and you iterate until it says LGTM
peer: required
---
Ask {peer} for a review of the uncommitted changes in {cwd}, and speak to no other session
about this.

Run `git diff` first, so you know what you are asking about. Then send {peer} one
`SendMessage` whose text begins with the exact tag `[second-opinion round 1/3]` and which
says all of this:

- that you are {self}, and that you want a review of the uncommitted diff in {cwd};
- **that the reply must come back to you as a `SendMessage` addressed to {self}** - without
  that sentence {peer} will write its findings into its own pane, where you will never see
  them, and you will wait for a reply that is not coming;
- that the reply must be either the single word `LGTM` or a numbered list of findings;
- that the exchange is capped at 3 rounds.

When {peer}'s reply reaches you: if it lists findings, fix the ones you agree with, say so
for any you do not, and then send {peer} another `SendMessage` tagged
`[second-opinion round 2/3]` asking it to look at the diff again. Do the same for round 3 if
it is still not satisfied. Stop as soon as {peer} says `LGTM`, or when round 3 is done.

Then tell me in one short paragraph what {peer} found, what you changed and how many rounds
it took. Do not ask me anything before then. If no reply arrives from {peer} at all, tell me
that instead of messaging it again.
