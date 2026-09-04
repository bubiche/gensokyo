# gensokyo

A bash + tmux cockpit for running several Claude Code sessions side by side:
named residents in a status bar, live panes, zoom to focus, desktop notifications
when one needs you, broadcast "spell cards", and scheduled "rituals".

**Status:** pre-alpha. The cockpit, summon/banish/list and the status bar work; notifications, broadcasts and schedules are not there yet.

## Requirements

**To run a release:** Claude Code (≥ 2.1.224). Nothing else. tmux and jq are
vendored from their official dependency-free builds (`vendor/README.md`).
macOS and Linux, arm64 and x86_64.

**To develop** (not needed for the release):

| Tool | Why | Install |
|---|---|---|
| `shellcheck` | every checkpoint is shellcheck-clean (`shellcheck -s bash bin/gensokyo lib/*.sh`, `-s sh install.sh scripts/vendor.sh`) | `brew install shellcheck` |
| `curl` | fetches the vendored binaries | preinstalled on macOS |
| Docker (optional) | run the Linux vendor binaries in `debian:stable-slim` | https://docker.com |
| Claude Code | the real acceptance tests spawn real sessions | https://code.claude.com |

`/bin/bash` 3.2 is the target for `bin/gensokyo`; `install.sh` and `scripts/*.sh`
are POSIX `sh`. Do not use bash 4 features.

## Developing

```sh
git clone … gensokyo && cd gensokyo
scripts/vendor.sh          # fetch tmux + jq for this machine into vendor/<os>-<arch>/
scripts/vendor.sh --status
bin/gensokyo doctor        # which tmux / jq / claude will be used, versions, server state
bin/gensokyo               # start the cockpit (Ctrl-Space then ? lists the keys)
bin/gensokyo help          # command list; --json is what the resident skill reads
bin/gensokyo new ~/dev/x -n Marisa   # a resident in a pane; close <name>, list, focus <name>, stage <layout>
tests/run.sh                         # unit tests + a headless smoke test with the stub claude (-v for names)
```

Run it from the checkout; `install.sh` (symlink into `~/.local/bin`) comes later. `bin/gensokyo`
reads no `~/.tmux.conf` and never edits `~/.claude/settings.json`: it runs its own tmux server
(`tmux -L gensokyo`) with `share/tmux.conf`, and per-user tweaks go in `~/.config/gensokyo/`
(`config` for KEY=value settings such as `PREFIX=C-Space`, `tmux.conf` sourced last).

Environment overrides for tests and CI: `GENSOKYO_TMUX`, `GENSOKYO_JQ`, `GENSOKYO_CLAUDE`
(binaries), `GENSOKYO_STATE_DIR`, `GENSOKYO_CONFIG_DIR`, `GENSOKYO_SOCKET`. `tests/stub-claude` stands in
for `claude` (registry, names, /rename, /exit) so the cockpit runs without Claude Code or a login:
`GENSOKYO_CLAUDE=$PWD/tests/stub-claude GENSOKYO_SOCKET=t bin/gensokyo`. `tests/run.sh` does exactly
that on its own socket and state dir, so it can run in CI.

## Residents speak gensokyo

Every resident is launched with `--plugin-dir share/plugin` and a one-paragraph
`--append-system-prompt`, both per session (`~/.claude` is never touched). The plugin's
`gensokyo` skill is the resident's handbook: plain requests such as "start a new agent in
~/dev/x called Cirno", "who is waiting on me?", "close Sakuya" or "zoom on Marisa" turn into
`gensokyo new/list/close/focus` calls, with a confirmation before closing. Themed words
(summon, who, banish, recall) work too. The skill reads `gensokyo help --json` for the
authoritative command list. `tests/skills/gensokyo.md` is the phrasing checklist run by hand
against a real resident, since the stub cannot run a model.
