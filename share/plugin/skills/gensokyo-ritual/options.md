# Less common `ritual add` options

The defaults suit almost every ritual; each entry says when it is worth naming.

- `--target` is where the fire lands. Leave it out for the default, a fresh resident per run,
  which is the right answer for almost everything - the prompt is written for a session that
  has never seen the job before, and the memory file is what carries continuity.
  `--target persistent` gives the ritual one session it keeps between fires; offer it only when
  the user says the runs need to remember each other in conversation rather than in notes, and
  say that the context and the bill grow. `--target <resident name>` types the prompt into a
  resident they already have - for "ask Sakuya to do X every morning" - and that prompt gets no
  memory-file sentence, so it has to stand on its own.
- `--target branch` types a probe's news per branch into whoever works on that branch: read
  `branch-target.md`, beside this file, before you write one.
- `--deliver` is for a prompt typed into a resident (`persistent`, a name or `branch`). The default,
  `idle`, waits until that resident's turn is over and nothing is open or half typed in it, so
  the prompt never lands mid-task; leave it out. `--deliver now` is for the rare ritual that must
  reach a busy session at once (Claude Code queues it behind the turn).
- `--quiet`: its runs' finished turns neither ring nor turn gold. Their permission prompts still
  do. For a ritual whose work shows up somewhere else, such as a page. Default target only.
- `--keep` is how long the finished run stays in the shrine before gensokyo asks it to leave,
  `2h` by default; it can still be recalled afterwards. Only worth naming when the user asks for
  it - `--keep forever` for a ritual whose run they want to come back to, a shorter one for a
  ritual that fires often. Idle time, so anything they type into that resident puts its life
  back.
- `--overlap` is what happens when a fire lands while the last run is still going: `skip` (the
  default, and the right answer for a daily job), `queue` to run it as soon as the ritual is
  free, `parallel` to start a second run beside the first. Worth raising only for a ritual that
  fires often enough to catch itself up - "every 30m" and a job that sometimes takes longer. It
  is a default-target setting; a session that is already there queues its own prompts.

`--keep` is only for the default target without `--headless` (a headless run leaves no
resident to keep); `--overlap` is for the default target, headless or not; `--headless` is a
`claude -p` of its own, so it goes with no `--target`. `gensokyo ritual add` refuses any other
mix rather than writing a line that does nothing.
