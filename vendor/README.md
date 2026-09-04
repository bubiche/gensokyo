# vendor/

Binaries gensokyo ships so the release works with nothing installed but Claude Code.
They are **not committed**; `scripts/vendor.sh` (or `install.sh`) downloads them from
the official upstream releases and verifies them against `SHA256SUMS`.

| File | Upstream | Pinned |
|---|---|---|
| `<os>-<arch>/tmux` | https://github.com/tmux/tmux-builds/releases/tag/v3.7c (official, no external dependencies) | 3.7c |
| `<os>-<arch>/jq`   | https://github.com/jqlang/jq/releases/tag/jq-1.8.2 (official static builds) | 1.8.2 |
| `LICENSES/`        | `LICENSES.tar.gz` from tmux-builds, `COPYING` from the jq repo at tag jq-1.8.2 | |

`SHA256SUMS` is a plain `shasum -a 256` file keyed by upstream asset name; the URLs are
derived from the version pins at the top of `scripts/vendor.sh`. tmux-builds publishes no
checksum file, so the tmux sums were taken from the assets as downloaded on 2026-09-04.
The jq sums match upstream's `sha256sum.txt` for that release.

Bumping: change the two version pins in `scripts/vendor.sh`, run
`scripts/vendor.sh --print-sums --all` to compute the new sums, replace `SHA256SUMS`.
`scripts/vendor.sh --check-upstream` reports whether a newer release exists.
