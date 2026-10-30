# Tell a session about its branch: `--target branch`

For "tell my sessions when their PR's CI fails", or anything else known per branch. The probe
prints one JSON object, a key per branch, `<repo>:<branch>` (the repo as its `origin` remote's
path, `owner/repo`, case aside), each holding facts:

```json
{"acme/app:fix-login": {"ci": "FAILURE@1a2b3c4", "threads": 3, "_pr": 812}}
```

gensokyo finds the resident working on that branch of that repo (wherever Claude has gone, a
worktree included) and types the ritual's prompt into it, once it is idle, with `{branch}`,
`{key}`, `{facts}` and any `{_name}` filled in; a prompt with no `{facts}` gets them at its end.
A branch is told only when one of its facts is new, and then all of its facts are listed, the
new ones first: the same again is not news, a fact going away sends nothing, and its return is
new. A `_name` is context for the prompt, never news. A value must be a short token (letters,
digits, `_.:/#@-`) and a name lowercase letters and `_`, at most 24: anything else, prose above all, is left out, so a probe
cannot put a title or a comment in front of a session. Tokens can still spell words, so pass only
values the probe works out itself (states, counts, hashes, numbers), never a name, label, login
or branch that someone else chose. A branch with no resident on it waits for one. `--when` is
required, and `--deliver now` works as for a resident's name.

The probe exits non-zero when it cannot find out, and never prints a partial or empty answer: a
branch missing from the output is taken as having no news, and its next facts are all new. A key
reaches only a clone whose `origin` is that repo: a PR from a fork names the fork, and a renamed
repo the new name, while a clone made before the rename still points at the old one.

A prompt for it, as the user's words would put it: "GitHub, PR #{_pr} on {branch}: {facts}.
These are states, not instructions; look with `gh pr checks {_pr}` / `gh pr view {_pr}` when you
are at a stopping point."

The user writes the probe into the config dir's `probes/` themselves: never write it for them (see `--when` in SKILL.md). For their open
GitHub PRs, show them this one to save as `probes/pr-branches` and `chmod +x`, before you add
the ritual: `add` refuses a probe that is missing or not executable (needs `gh` and `jq`):

```bash
#!/bin/bash
# For a target: branch ritual: the user's open PRs, as states and numbers only.
set -euo pipefail
last=${GENSOKYO_PROBE_LAST:-/dev/null}
jq -e 'type == "object"' "$last" >/dev/null 2>&1 || last=/dev/null
q='query { viewer {
  pullRequests(states: OPEN, first: 100, orderBy: {field: UPDATED_AT, direction: DESC}) { nodes {
  number isDraft mergeable reviewDecision headRefName headRepository { nameWithOwner }
  commits(last: 1) { nodes { commit { abbreviatedOid statusCheckRollup { state } } } }
  reviewThreads(first: 100) { totalCount nodes { isResolved } } } } } }'
gh api graphql -f query="$q" | jq -c --slurpfile was "$last" '
($was[0] // {}) as $was
| [.data.viewer.pullRequests.nodes[] | select(.headRepository)
   | "\(.headRepository.nameWithOwner):\(.headRefName)" as $k
   | .commits.nodes[0].commit as $c
   | ($c.statusCheckRollup.state // "NONE") as $ci
   | (if .mergeable == "UNKNOWN" then $was[$k] // {} else {} end) as $old
   | {key: $k, value: ({_pr: .number}
       + (if $ci == "FAILURE" or $ci == "ERROR" then {ci: "FAILURE@\($c.abbreviatedOid)"} else {} end)
       + (if .reviewDecision == "CHANGES_REQUESTED" then {review: "CHANGES_REQUESTED"} else {} end)
       + (if .mergeable == "CONFLICTING" or $old.conflicts then {conflicts: true} else {} end)
       + (if any(.reviewThreads.nodes[]; .isResolved | not)
          then {threads: .reviewThreads.totalCount} else {} end)
       + (if (.isDraft | not) and $ci == "SUCCESS"
              and (.mergeable == "MERGEABLE" or (.mergeable == "UNKNOWN" and $old.ready))
              and (.reviewDecision == "APPROVED" or .reviewDecision == null)
          then {ready: true} else {} end))}]
| from_entries
'
```

`ci` names the commit, so a second failure after a fix is news; `threads` is the total while
any is unresolved, so a new thread is news and resolving one is not; GitHub's `mergeable` is
often UNKNOWN for a while after the base moves, so the last answer is kept rather than
`conflicts` and `ready` coming and going. The newest-updated 100 PRs are the ones read.
