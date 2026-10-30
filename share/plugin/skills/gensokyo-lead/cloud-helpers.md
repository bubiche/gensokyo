# Cloud helpers

A cloud helper works on claude.ai and counts as one of your helpers. Summon it like this:

```bash
gensokyo new /abs/path/to/repo --cloud --worktree <name> --name Aya --prompt-file - <<'BRIEF'
…the brief…
BRIEF
```

- **The repository**: its `origin` is on GitHub, and not through an SSH host alias
  (`git@github.com-work:…`); gensokyo refuses that, with the fix to tell the user. With
  `--worktree`, the new branch is pushed first. Without it, the current branch must be pushed
  and up to date: the session sees only what is on GitHub.
- **Your mode**: it runs its tools and connectors with nobody asked, so you start one only in
  `auto` or `bypassPermissions`. In a narrower mode, suggest the command to the user instead.
- **No role, system prompt, mode, effort or allowed tools**: claude.ai sets the session up.
  `--model` and `--env env_…` work.
- **The brief says**, in place of the usual file report and "no commit or push":
  - it commits and pushes its work: its pushed branch is all you get back. The `--worktree`
    branch is only its base; it works on a `claude/…` branch of its own;
  - its last message is its report, **under 1500 characters**: its log cuts longer ones;
  - detail goes in commits and the PR description, on its branch;
  - it uses connectors only when the task needs them: each one's tools run unasked there.
- **Wait and read** as for any helper. `read` ends with the session's link, where the whole report
  is. It has no screen here and asks nobody, so `read --screen`, `wait --until needs` and
  `banish` don't apply: look at a slow one on its link. Its branch shows in
  `gensokyo list --json` (`cloud.branch`) once found; `gh pr view` it.
- **Follow up** with `gensokyo cloud say Aya "<words>"`, not SendMessage, which cannot reach it.
  Then wait again.
- **Close** it when its work is in: gensokyo stops following it, and the session stays on
  claude.ai for the user to archive. Give them its link. Taking it over here
  (`gensokyo new --teleport`) is the user's.
