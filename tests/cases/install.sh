# tests/cases/install.sh - gensokyo's own install, end to end: install.sh run from the checkout
# into a scratch bin dir, release.sh's tarballs built and unpacked again, the `curl | sh` path
# against a release directory served over file://, and `update` and `uninstall` on what that
# leaves behind. No test here touches the machine's real HOME, config, state or iTerm2 folder.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

# rel_gensokyo <args...>: the gensokyo installed under $uh, run with its own home, config,
# state, iTerm2 folder, tmux socket and release base, so that nothing here can reach the real
# ones. bash's dynamic scoping is what lets it read $uh, $rel2 and $sock from its caller.
rel_gensokyo() {
  HOME=$uh GENSOKYO_RELEASE_BASE=file://$rel2 GENSOKYO_SOCKET=$sock \
    GENSOKYO_CONFIG_DIR=$uh/.config/gensokyo GENSOKYO_STATE_DIR=$uh/.local/state/gensokyo \
    GENSOKYO_ITERM_DIR=$uh/iterm "$uh/bin/gensokyo" "$@" 2>&1
}

# leftovers <home>: everything gensokyo leaves on a Mac - an unpacked tree, the symlink to it,
# a config with a ritual in it, a state directory, and gensokyo's own iTerm2 profile.
leftovers() {
  local h=$1
  rm -rf "$h"; mkdir -p "$h/.local/bin" "$h/.config/gensokyo/rituals" "$h/.local/state/gensokyo/residents" "$h/iterm"
  tar -xzf "$dist/$pre-$PLATFORM.tar.gz" -C "$h" && mv "$h/$pre" "$h/.gensokyo"
  ln -sfn "$h/.gensokyo/bin/gensokyo" "$h/.local/bin/gensokyo"
  printf 'PREFIX=C-a\n' > "$h/.config/gensokyo/config"
  : > "$h/.config/gensokyo/rituals/mine.md"
  : > "$h/.local/state/gensokyo/residents/abc"
  cp "$root/share/iterm2/gensokyo.json" "$h/iterm/gensokyo.json"
}

# uninstall_sh <home> <args...>: the standalone uninstaller against that home and nothing else.
# env -i on purpose: with only a faked HOME, uninstall.sh's `command -v gensokyo` would still
# find the real one on the real PATH, and this machine's own install is not the test's to touch.
uninstall_sh() {
  local h=$1; shift
  env -i PATH=/usr/bin:/bin "HOME=$h" "TMPDIR=${TMPDIR:-/tmp}" "GENSOKYO_ITERM_DIR=$h/iterm" \
    "GENSOKYO_SOCKET=$sock" sh "$root/uninstall.sh" "$@" 2>&1
}

install_tests() {
  local out bin=$scratch/ibin
  t "install.sh links bin/gensokyo into --bin-dir and the link resolves to this checkout"
  if [ ! -x "$root/vendor/$PLATFORM/tmux" ] && ! command -v tmux >/dev/null 2>&1; then skip "no tmux for install.sh --no-fetch"; return; fi
  out=$(cd "$scratch" && sh "$root/install.sh" --bin-dir "$bin" --no-fetch 2>&1)
  assert_match "$out" "linked $bin/gensokyo -> $root/bin/gensokyo"
  assert_match "$out" "tmux + jq: "
  assert_eq "$(readlink "$bin/gensokyo")" "$root/bin/gensokyo"
  assert_eq "$("$bin/gensokyo" version)" "gensokyo $VERSION"
  out=$(PATH=$bin:/usr/bin:/bin "$bin/gensokyo" doctor)
  assert_match "$out" "home       $root"
  assert_match "$out" "on PATH    $bin/gensokyo -> this copy"

  t "install.sh is idempotent, warns about PATH only when needed, and refuses to clobber a file"
  out=$(sh "$root/install.sh" --bin-dir "$bin" --no-fetch 2>&1)
  assert_match "$out" "linked $bin/gensokyo"
  assert_match "$out" "$bin is not on your PATH"
  out=$(PATH=$bin:$PATH sh "$root/install.sh" --bin-dir="$bin" --no-fetch 2>&1)
  assert_nomatch "$out" 'not on your PATH'
  mkdir -p "$scratch/ibin2"; : > "$scratch/ibin2/gensokyo"
  if out=$(sh "$root/install.sh" --bin-dir "$scratch/ibin2" --no-fetch 2>&1); then bad "should refuse to clobber a file"; else assert_match "$out" 'is not a symlink'; fi
  assert_fails sh "$root/install.sh" --bogus
  assert_match "$(sh "$root/install.sh" --help)" 'install.sh --no-fetch'
  cp "$root/install.sh" "$scratch/install.sh"
  assert_fails sh "$scratch/install.sh" --no-fetch   # not next to bin/gensokyo

  t "release.sh builds the three tarballs and a SHA256SUMS that verifies"
  if [ ! -x "$root/vendor/macos-arm64/tmux" ] || [ ! -x "$root/vendor/macos-x86_64/jq" ]; then
    skip "no vendored binaries for both platforms (run scripts/vendor.sh --all)"; return
  fi
  local dist=$scratch/dist ver=9.9.9-test pre
  pre="gensokyo-$ver"
  out=$(sh "$root/scripts/release.sh" --version "$ver" --out "$dist" --no-fetch 2>&1) || bad "release.sh failed: $out"
  assert_match "$out" "$pre-macos-arm64.tar.gz"
  assert_match "$out" "$pre-macos-x86_64.tar.gz"
  assert_match "$out" "$pre-macos-all.tar.gz"
  assert_eq "$(cd "$dist" && ls | LC_ALL=C sort | tr '\n' ' ')" "SHA256SUMS VERSION $pre-macos-all.tar.gz $pre-macos-arm64.tar.gz $pre-macos-x86_64.tar.gz "
  assert_ok sh -c "cd '$dist' && shasum -a 256 -c SHA256SUMS"
  assert_eq "$(cat "$dist/VERSION")" "$ver"

  t "a tarball unpacks into one directory that installs and reports the stamped version"
  mkdir -p "$scratch/unpack"
  assert_ok tar -xzf "$dist/$pre-macos-arm64.tar.gz" -C "$scratch/unpack"
  assert_eq "$("$scratch/unpack/$pre/bin/gensokyo" version)" "gensokyo $ver"
  assert_eq "$(sed -n 's/^VERSION=//p' "$root/bin/gensokyo" | head -n 1)" "$VERSION"   # the checkout keeps its own
  out=$(cd "$scratch/unpack/$pre" && sh ./install.sh --bin-dir "$scratch/rbin" --no-fetch 2>&1)
  assert_match "$out" "tmux + jq: vendored"
  out=$(GENSOKYO_STATE_DIR=$scratch/rstate GENSOKYO_CONFIG_DIR=$scratch/rcfg GENSOKYO_ITERM_DIR=$scratch/riterm "$scratch/rbin/gensokyo" doctor)
  assert_match "$out" "gensokyo $ver"
  assert_re "$out" 'tmux +3\.[0-9]+[a-z]?.*\(vendored\)'
  assert_re "$out" 'jq +1\.[0-9.]+.*\(vendored\)'

  t "the tarballs carry what a release install needs and no more"
  local list
  list=$(tar -tzf "$dist/$pre-macos-arm64.tar.gz")
  for f in bin/gensokyo lib/rituals.sh share/plugin/.claude-plugin/plugin.json share/rituals/inbox-zero.md \
           install.sh uninstall.sh README.md scripts/vendor.sh vendor/SHA256SUMS vendor/LICENSES/COPYING.jq \
           vendor/macos-arm64/tmux vendor/macos-arm64/jq; do
    t "  the arm64 tarball has $f"
    assert_match "$list" "$pre/$f"
  done
  t "the arm64 tarball has neither the other platform, the tarball sums, nor the planning files"
  assert_nomatch "$list" 'vendor/macos-x86_64'
  assert_nomatch "$list" "$pre/SHA256SUMS"
  assert_nomatch "$list" 'implementation_notes'
  assert_nomatch "$list" 'tests/'
  assert_nomatch "$list" '/._'
  t "the all-platforms tarball has both"
  list=$(tar -tzf "$dist/$pre-macos-all.tar.gz")
  assert_match "$list" "$pre/vendor/macos-arm64/tmux"
  assert_match "$list" "$pre/vendor/macos-x86_64/tmux"

  t "release.sh refuses a version it cannot put in a filename, and unknown options"
  assert_fails sh "$root/scripts/release.sh" --version "0.1.0 or so" --out "$scratch/dist-bad" --no-fetch
  assert_fails sh "$root/scripts/release.sh" --version "../etc" --out "$scratch/dist-bad" --no-fetch
  assert_fails sh "$root/scripts/release.sh" --bogus
  assert_fails sh "$root/scripts/release.sh" --version
  assert_match "$(sh "$root/scripts/release.sh" --help)" 'release.sh --out DIR'

  t "install.sh piped into sh downloads the release, verifies it and links it"
  # The whole `curl | sh` path, offline: curl speaks file://, so a directory laid out the way
  # GitHub serves a release stands in for github.com. $0 is `sh` here, which is what tells
  # install.sh there is no tree beside it and it has to download one.
  local rel=$scratch/rel fake=$scratch/fakehome
  mkdir -p "$rel/download/v$ver" "$rel/latest/download" "$fake"
  cp "$dist/$pre-macos-arm64.tar.gz" "$dist/$pre-macos-x86_64.tar.gz" "$dist/SHA256SUMS" "$rel/download/v$ver/"
  cp "$dist/VERSION" "$rel/latest/download/VERSION"
  out=$(HOME=$fake GENSOKYO_RELEASE_BASE=file://$rel sh -s -- --bin-dir "$fake/bin" < "$root/install.sh" 2>&1) ||
    bad "piped install failed: $out"
  assert_match "$out" "gensokyo $ver: downloading $pre-$PLATFORM.tar.gz"
  assert_match "$out" "unpacked into $fake/.gensokyo"
  assert_match "$out" "linked $fake/bin/gensokyo -> $fake/.gensokyo/bin/gensokyo"
  assert_eq "$("$fake/bin/gensokyo" version)" "gensokyo $ver"
  assert_eq "$(readlink "$fake/bin/gensokyo")" "$fake/.gensokyo/bin/gensokyo"

  t "the piped install works with nothing but /usr/bin and /bin on PATH"
  # What a release install actually has: no brew tmux, no brew jq, no shell rc, nothing but the
  # system's own binaries. The tarball's own vendor/<platform>/ is what has to carry it.
  local bare=$scratch/barehome
  mkdir -p "$bare"
  out=$(env -i PATH=/usr/bin:/bin HOME="$bare" "TMPDIR=${TMPDIR:-/tmp}" \
        GENSOKYO_RELEASE_BASE="file://$rel" sh -s -- --bin-dir "$bare/bin" < "$root/install.sh" 2>&1) ||
    bad "install on a bare PATH failed: $out"
  assert_match "$out" 'tmux + jq: vendored'
  out=$(env -i PATH=/usr/bin:/bin HOME="$bare" GENSOKYO_STATE_DIR="$bare/state" \
        GENSOKYO_CONFIG_DIR="$bare/cfg" GENSOKYO_ITERM_DIR="$bare/iterm" "$bare/bin/gensokyo" doctor 2>&1)
  assert_match "$out" "gensokyo $ver"
  assert_re "$out" 'tmux +3\.[0-9]+[a-z]?.*\(vendored\)'
  assert_re "$out" 'jq +1\.[0-9.]+.*\(vendored\)'

  t "a second piped install says to update instead of clobbering the first"
  if out=$(HOME=$fake GENSOKYO_RELEASE_BASE=file://$rel sh -s -- --bin-dir "$fake/bin" < "$root/install.sh" 2>&1); then
    bad "should refuse to install over $fake/.gensokyo"
  else
    assert_match "$out" "already holds gensokyo: run 'gensokyo update'"
  fi

  t "a tarball that does not match the release's SHA256SUMS is refused, and nothing is written"
  local bad_rel=$scratch/rel-bad bad_home=$scratch/fakehome-bad
  mkdir -p "$bad_rel/download/v$ver" "$bad_rel/latest/download" "$bad_home"
  cp "$dist/SHA256SUMS" "$bad_rel/download/v$ver/"; cp "$dist/VERSION" "$bad_rel/latest/download/"
  cp "$dist/$pre-macos-arm64.tar.gz" "$bad_rel/download/v$ver/"
  cp "$dist/$pre-macos-x86_64.tar.gz" "$bad_rel/download/v$ver/"
  printf 'tampered' >> "$bad_rel/download/v$ver/$pre-$PLATFORM.tar.gz"
  if out=$(HOME=$bad_home GENSOKYO_RELEASE_BASE=file://$bad_rel sh -s -- --bin-dir "$bad_home/bin" < "$root/install.sh" 2>&1); then
    bad "should refuse a tarball whose checksum does not match"
  else
    assert_match "$out" 'checksum mismatch'
    assert_match "$out" 'Refusing to install'
  fi
  assert_fails test -e "$bad_home/.gensokyo"
  assert_fails test -e "$bad_home/bin/gensokyo"

  t "a release with no SHA256SUMS is refused rather than installed unverified"
  rm -f "$bad_rel/download/v$ver/SHA256SUMS"
  cp "$dist/$pre-$PLATFORM.tar.gz" "$bad_rel/download/v$ver/"
  if out=$(HOME=$bad_home GENSOKYO_RELEASE_BASE=file://$bad_rel sh -s -- --bin-dir "$bad_home/bin" < "$root/install.sh" 2>&1); then
    bad "should refuse a release with no SHA256SUMS"
  else
    assert_match "$out" 'refusing to install an unverified tarball'
  fi

  t "install.sh names the version it cannot find, and says so when there is no latest either"
  out=$(HOME=$bad_home GENSOKYO_RELEASE_BASE=file://$rel sh -s -- --version 9.0.0 --dir "$bad_home/g" < "$root/install.sh" 2>&1) &&
    bad "should fail for a version that is not published"
  assert_match "$out" "gensokyo-9.0.0-$PLATFORM.tar.gz (no such release"
  out=$(HOME=$bad_home GENSOKYO_RELEASE_BASE=file://$scratch/nowhere sh -s -- --dir "$bad_home/g" < "$root/install.sh" 2>&1) &&
    bad "should fail when the latest version cannot be resolved"
  assert_match "$out" 'cannot tell which release is the latest'

  t "install.sh piped into sh still answers --help, with no file to read it from"
  out=$(GENSOKYO_RELEASE_BASE=file://$rel sh -s -- --help < "$root/install.sh")
  assert_match "$out" 'install.sh | sh -s --'
  assert_match "$(sh "$root/install.sh" --help)" './install.sh --dir DIR'

  t "update --check names the release that is out, and update swaps the tree for it"
  # A second release to move to, and a base whose "latest" is that one. The layout is what
  # GitHub serves: download/v<version>/<assets> and latest/download/VERSION.
  local ver2=9.9.10-test pre2 rel2=$scratch/rel2 uh=$scratch/uhome sock=gtestrel$$
  pre2="gensokyo-$ver2"
  sh "$root/scripts/release.sh" --version "$ver2" --out "$scratch/dist2" --no-fetch >/dev/null 2>&1 ||
    bad "release.sh failed for $ver2"
  mkdir -p "$rel2/download/v$ver2" "$rel2/latest/download" "$uh"
  ln -sfn "$rel/download/v$ver" "$rel2/download/v$ver"
  cp "$scratch/dist2"/*.tar.gz "$scratch/dist2/SHA256SUMS" "$rel2/download/v$ver2/"
  cp "$scratch/dist2/VERSION" "$rel2/latest/download/VERSION"
  # An install of the older release to update from, and a config and a ritual to survive it.
  out=$(HOME=$uh GENSOKYO_RELEASE_BASE=file://$rel2 sh -s -- --version "$ver" --bin-dir "$uh/bin" < "$root/install.sh" 2>&1) ||
    bad "install of $ver failed: $out"
  mkdir -p "$uh/.config/gensokyo/rituals" "$uh/.local/state/gensokyo"
  printf 'PREFIX=C-a\n' > "$uh/.config/gensokyo/config"
  : > "$uh/.config/gensokyo/rituals/mine.md"
  assert_eq "$(rel_gensokyo version)" "gensokyo $ver"
  assert_eq "$(rel_gensokyo update --check)" "gensokyo $ver is installed; $ver2 is out ('gensokyo update' fetches it)"
  assert_eq "$(rel_gensokyo version)" "gensokyo $ver"   # --check changes nothing
  out=$(rel_gensokyo update)
  assert_match "$out" "downloading $pre2-$PLATFORM.tar.gz"
  assert_match "$out" "gensokyo $ver -> $ver2 in ~/.gensokyo"
  assert_eq "$(rel_gensokyo version)" "gensokyo $ver2"

  t "the update keeps the link, the config and the rituals, and leaves nothing behind"
  assert_eq "$(readlink "$uh/bin/gensokyo")" "$uh/.gensokyo/bin/gensokyo"
  assert_eq "$(cat "$uh/.config/gensokyo/config")" "PREFIX=C-a"
  assert_ok test -f "$uh/.config/gensokyo/rituals/mine.md"
  assert_ok test -d "$uh/.local/state/gensokyo"
  assert_eq "$(ls -a "$uh" | grep -c 'gensokyo-update\|\.old\.')" "0"
  assert_eq "$(rel_gensokyo update)" "gensokyo $ver2 is already the release you asked for"
  assert_eq "$(rel_gensokyo update --version "$ver2")" "gensokyo $ver2 is already the release you asked for"

  t "update refuses a git checkout, an unpublished version and a tarball that does not verify"
  out=$("$root/bin/gensokyo" update --check 2>&1) && bad "update should refuse a git checkout"
  assert_match "$out" "is a git checkout, not a release install: 'git pull' updates it"
  out=$(rel_gensokyo update --version 5.0.0) && bad "update should fail for an unpublished version"
  assert_match "$out" "gensokyo-5.0.0-$PLATFORM.tar.gz (no such release"

  t "update --version installs the release it names, backwards as well as forwards"
  assert_match "$(rel_gensokyo update --version "$ver")" "gensokyo $ver2 -> $ver in ~/.gensokyo"
  assert_eq "$(rel_gensokyo version)" "gensokyo $ver"
  assert_match "$(rel_gensokyo update)" "gensokyo $ver -> $ver2 in ~/.gensokyo"

  t "a tarball that does not match the release's SHA256SUMS leaves the install as it was"
  local rel3=$scratch/rel3
  mkdir -p "$rel3/download/v$ver"
  cp "$rel/download/v$ver"/* "$rel3/download/v$ver/"
  printf 'tampered' >> "$rel3/download/v$ver/$pre-$PLATFORM.tar.gz"
  out=$(HOME=$uh GENSOKYO_RELEASE_BASE=file://$rel3 GENSOKYO_SOCKET=$sock \
        GENSOKYO_CONFIG_DIR=$uh/.config/gensokyo GENSOKYO_STATE_DIR=$uh/.local/state/gensokyo \
        GENSOKYO_ITERM_DIR=$uh/iterm "$uh/bin/gensokyo" update --version "$ver" 2>&1) &&
    bad "update should refuse a tarball that does not match SHA256SUMS"
  assert_match "$out" 'checksum mismatch'
  assert_eq "$(rel_gensokyo version)" "gensokyo $ver2"   # still the good one
  assert_ok test -x "$uh/.gensokyo/bin/gensokyo"

  t "update and uninstall refuse to touch an install whose cockpit is running"
  local tmux_bin=$root/vendor/$PLATFORM/tmux
  [ -x "$tmux_bin" ] || tmux_bin=$(command -v tmux)
  "$tmux_bin" -L "$sock" new-session -d -s gensokyo 'sh -c "while :; do sleep 5; done"' 2>/dev/null
  if "$tmux_bin" -L "$sock" has-session -t '=gensokyo' 2>/dev/null; then
    out=$(rel_gensokyo uninstall --yes) && bad "uninstall should refuse while the cockpit runs"
    assert_match "$out" "the cockpit is running: 'gensokyo quit' first"
    assert_ok test -x "$uh/.gensokyo/bin/gensokyo"
    "$tmux_bin" -L "$sock" kill-server 2>/dev/null
  else
    skip "cannot start a tmux server on socket $sock"
  fi

  t "uninstall --keep-data takes the link and the profile and leaves the data and the tree"
  mkdir -p "$uh/iterm"
  cp "$root/share/iterm2/gensokyo.json" "$uh/iterm/gensokyo.json"
  out=$(rel_gensokyo uninstall --keep-data)
  assert_match "$out" "gensokyo $ver2 in ~/.gensokyo"
  assert_match "$out" 'removed the link ~/bin/gensokyo'
  assert_match "$out" 'removed the iTerm2 profile ~/iterm/gensokyo.json'
  assert_match "$out" 'kept ~/.config/gensokyo and ~/.local/state/gensokyo (--keep-data)'
  assert_match "$out" 'never written to'
  assert_fails test -e "$uh/bin/gensokyo"
  assert_fails test -e "$uh/iterm/gensokyo.json"
  assert_ok test -f "$uh/.config/gensokyo/config"
  assert_ok test -x "$uh/.gensokyo/bin/gensokyo"   # no terminal to ask, so it stays

  t "uninstall leaves a gensokyo.json somebody else wrote alone"
  printf '{"Profiles":[{"Guid":"someone-elses","Name":"x"}]}\n' > "$uh/iterm/gensokyo.json"
  out=$(HOME=$uh GENSOKYO_RELEASE_BASE=file://$rel2 GENSOKYO_SOCKET=$sock \
        GENSOKYO_CONFIG_DIR=$uh/.config/gensokyo GENSOKYO_STATE_DIR=$uh/.local/state/gensokyo \
        GENSOKYO_ITERM_DIR=$uh/iterm "$uh/.gensokyo/bin/gensokyo" uninstall --keep-data 2>&1)
  assert_match "$out" 'another Guid, so not the profile gensokyo wrote'
  assert_match "$out" 'no link to this copy in the usual places'
  assert_ok test -f "$uh/iterm/gensokyo.json"

  t "uninstall --yes deletes the config, the state and the copy it is running from"
  out=$(HOME=$uh GENSOKYO_RELEASE_BASE=file://$rel2 GENSOKYO_SOCKET=$sock \
        GENSOKYO_CONFIG_DIR=$uh/.config/gensokyo GENSOKYO_STATE_DIR=$uh/.local/state/gensokyo \
        GENSOKYO_ITERM_DIR=$uh/iterm "$uh/.gensokyo/bin/gensokyo" uninstall --yes 2>&1)
  assert_match "$out" 'removed ~/.config/gensokyo'
  assert_match "$out" 'removed ~/.local/state/gensokyo'
  assert_match "$out" 'removed ~/.gensokyo'
  assert_match "$out" 'done. Claude Code, its sessions and its settings are untouched.'
  assert_fails test -e "$uh/.gensokyo"
  assert_fails test -e "$uh/.config/gensokyo"
  assert_fails test -e "$uh/.local/state/gensokyo"

  t "uninstall in a git checkout never deletes the checkout"
  out=$(HOME=$scratch/cohome GENSOKYO_SOCKET=$sock GENSOKYO_CONFIG_DIR=$scratch/cohome/cfg \
        GENSOKYO_STATE_DIR=$scratch/cohome/state GENSOKYO_ITERM_DIR=$scratch/cohome/iterm \
        "$root/bin/gensokyo" uninstall --yes 2>&1)
  assert_match "$out" "is a git checkout: left alone (delete it yourself if you want to)"
  assert_ok test -x "$root/bin/gensokyo"

  t "uninstall.sh carries the same profile Guid as lib/iterm.sh, or it would leave the profile"
  assert_eq "$(sed -n 's/^GUID=//p' "$root/uninstall.sh")" "$ITERM_GUID"

  t "uninstall.sh with no arguments says what is there and deletes none of it"
  local uh2=$scratch/leftovers
  leftovers "$uh2"
  out=$(uninstall_sh "$uh2")
  assert_match "$out" 'nothing is deleted without --yes'
  assert_match "$out" 'found  link: ~/.local/bin/gensokyo'
  assert_match "$out" 'found  install tree: ~/.gensokyo'
  assert_match "$out" 'found  iTerm2 profile: ~/iterm/gensokyo.json'
  assert_match "$out" 'found  config and rituals: ~/.config/gensokyo'
  assert_match "$out" 'found  records, journal and run logs: ~/.local/state/gensokyo'
  assert_match "$out" 'nothing was deleted. Add --yes'
  assert_match "$out" 'Claude Code, its settings and its sessions are untouched'
  assert_ok test -x "$uh2/.gensokyo/bin/gensokyo"
  assert_ok test -f "$uh2/.config/gensokyo/rituals/mine.md"
  assert_ok test -f "$uh2/iterm/gensokyo.json"

  t "uninstall.sh --yes removes the five, and says so on a home with nothing left"
  out=$(uninstall_sh "$uh2" --yes)
  assert_match "$out" 'removed  link: ~/.local/bin/gensokyo'
  assert_match "$out" 'removed  install tree: ~/.gensokyo'
  assert_match "$out" 'removed  iTerm2 profile: ~/iterm/gensokyo.json'
  assert_match "$out" 'removed  config and rituals: ~/.config/gensokyo'
  assert_match "$out" 'removed  records, journal and run logs: ~/.local/state/gensokyo'
  assert_fails test -e "$uh2/.gensokyo"
  assert_fails test -e "$uh2/.local/bin/gensokyo"
  assert_fails test -e "$uh2/.config/gensokyo"
  assert_fails test -e "$uh2/.local/state/gensokyo"
  assert_fails test -e "$uh2/iterm/gensokyo.json"
  assert_match "$(uninstall_sh "$uh2" --yes)" "nothing of gensokyo's is here."

  t "uninstall.sh --keep-data takes the install and leaves the rituals and the records"
  leftovers "$uh2"
  out=$(uninstall_sh "$uh2" --yes --keep-data)
  assert_match "$out" 'removed  install tree: ~/.gensokyo'
  assert_match "$out" 'kept  ~/.config/gensokyo (--keep-data)'
  assert_match "$out" 'kept  ~/.local/state/gensokyo (--keep-data)'
  assert_ok test -f "$uh2/.config/gensokyo/rituals/mine.md"
  assert_ok test -f "$uh2/.local/state/gensokyo/residents/abc"
  assert_fails test -e "$uh2/.gensokyo"

  t "uninstall.sh leaves alone a profile with another Guid, a gensokyo that is a real file, and a --dir that is not one"
  leftovers "$uh2"
  printf '{"Profiles":[{"Guid":"someone-elses","Name":"x"}]}\n' > "$uh2/iterm/gensokyo.json"
  rm -f "$uh2/.local/bin/gensokyo"; printf '#!/bin/sh\necho mine\n' > "$uh2/.local/bin/gensokyo"
  out=$(uninstall_sh "$uh2" --yes)
  assert_match "$out" 'left alone  ~/iterm/gensokyo.json was not written by gensokyo (another Guid)'
  assert_nomatch "$out" 'link: ~/.local/bin/gensokyo'
  assert_ok test -f "$uh2/iterm/gensokyo.json"
  assert_ok test -f "$uh2/.local/bin/gensokyo"
  mkdir -p "$uh2/notgensokyo"; : > "$uh2/notgensokyo/keepme"
  out=$(uninstall_sh "$uh2" --yes --dir "$uh2/notgensokyo")
  assert_match "$out" 'does not look like a gensokyo install'
  assert_ok test -f "$uh2/notgensokyo/keepme"

  t "uninstall.sh removes nothing at all while the cockpit is still running"
  leftovers "$uh2"
  "$tmux_bin" -L "$sock" new-session -d -s gensokyo 'sh -c "while :; do sleep 5; done"' 2>/dev/null
  if "$tmux_bin" -L "$sock" has-session -t '=gensokyo' 2>/dev/null; then
    out=$(uninstall_sh "$uh2" --yes) && bad "uninstall.sh should refuse while the cockpit runs"
    assert_match "$out" "the cockpit is still running on tmux socket '$sock'"
    assert_match "$out" 'kill-server'
    assert_match "$out" 'nothing was removed'
    assert_ok test -x "$uh2/.gensokyo/bin/gensokyo"
    assert_ok test -f "$uh2/.config/gensokyo/config"
    "$tmux_bin" -L "$sock" kill-server 2>/dev/null
  else
    skip "cannot start a tmux server on socket $sock"
  fi

  t "uninstall.sh tidies the socket tmux leaves behind once its server has gone"
  out=$(uninstall_sh "$uh2" --yes)
  assert_match "$out" 'removed  dead tmux socket'
  assert_match "$out" 'removed  install tree: ~/.gensokyo'
  assert_fails test -e "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$sock"

  t "uninstall.sh answers --help and refuses what it does not know"
  assert_match "$(sh "$root/uninstall.sh" --help)" './uninstall.sh --yes --keep-data'
  assert_match "$(sh -s -- --help < "$root/uninstall.sh")" 'sh -s -- [--yes]'
  assert_fails sh "$root/uninstall.sh" --bogus
  assert_fails sh "$root/uninstall.sh" --dir
}
