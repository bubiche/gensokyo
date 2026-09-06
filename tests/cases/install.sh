# tests/cases/install.sh - install.sh: POSIX sh, run from the checkout into a scratch bin dir.
# Sourced by tests/run.sh, which holds the harness, the scratch state dir and the sourced
# bin/gensokyo these tests call.
# shellcheck shell=bash
# shellcheck disable=SC2154,SC2034,SC2016,SC2012,SC2013,SC2088  # functions and globals come from the sourced script; jq filters use $; ls on our own files

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
}
