---
title: Border Sign "Sync Up"
summary: every resident tells the others what it is on, and raises any overlap with the one it affects
---
The other residents on this machine are: {residents}. If that says `nobody`, tell me there
is no one to sync with and stop here.

Otherwise, send each of them one `SendMessage` whose text begins with the exact tag
`[sync-up]` and which says, in two lines at most: that you are {self}, working in {cwd};
what you are working on; and anything you are about to touch that they should keep out of.
End every note with the words "reply only if this touches your work".

Notes tagged `[sync-up]` will arrive from them as well. Read them, and answer a note **only**
when what it describes overlaps what you are doing - the same file, the same directory, the
same piece of work. When it does, send that resident exactly one `SendMessage` tagged
`[sync-up reply]` saying which of you should take it and what you will keep off. Say who you
are, because there is no thread: a reply reads as a fresh message.

One reply each is the whole of it. Never answer a `[sync-up reply]`, never send a second note
to the same resident, and if nothing overlaps send nothing at all - silence here is the normal
outcome, not a failure.

When the notes have stopped arriving, tell me in a few lines who is doing what, what overlapped
and how the two of you settled it. Then carry on with what you were doing.
