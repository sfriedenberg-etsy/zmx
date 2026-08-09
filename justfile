
default: build test

build: build-nix

build-nix:
  nix build .#default

# Rust compilation check (for IDE integration / quick validation).
validate:
  cargo check

test: test-rust test-bats

test-rust:
  cargo test

test-bats:
  nix build .#bats-default --no-link --print-build-logs

# bats integration suite filtered by file_tag (e.g. just test-bats-tags help).
test-bats-tags *tags:
  nix build .#bats-{{tags}} --no-link --print-build-logs

# Fast iteration: runs against ./result/bin/zmx from a prior `just build`.
# Requires `bats` plus its libraries on PATH.
test-bats-local *targets="*.bats":
  ZMX_BIN=$(realpath ./result/bin/zmx) \
    BATS_TEST_TIMEOUT=10 \
    bats --jobs $(nproc) zz-tests_bats/{{targets}}

# Tag a release. The "v" prefix is added for you, so pass the semver
# without it. Usage: just tag 0.4.2 "feat: detach-all wiring"
[group("maint")]
tag version message:
  #!/usr/bin/env bash
  set -euo pipefail
  tag="v{{version}}"
  prev=$(git tag --sort=-v:refname -l "v*" | head -1)
  if [[ -n "$prev" ]]; then
    echo "Previous: $prev"
    git log --oneline "$prev"..HEAD
  fi
  git tag -s -m "{{message}}" "$tag"
  echo "Created tag: $tag"
  git push origin "$tag"
  echo "Pushed $tag"
  git tag -v "$tag"

# Sed-rewrite zmxVersion in flake.nix (and the Cargo package version) to the
# given semver. The version string is burnt into the binary at build time via
# ZMX_VERSION (see build.rs and src/main.rs print_version), so flake.nix is
# the source of truth for release builds and Cargo.toml the fallback for
# plain cargo builds. No-op if already at the target version.
# Usage: just bump-version 0.4.2
[group("maint")]
bump-version new_version:
  #!/usr/bin/env bash
  set -euo pipefail
  current=$(grep 'zmxVersion = ' flake.nix | sed 's/.*"\(.*\)".*/\1/')
  if [[ "$current" == "{{new_version}}" ]]; then
    echo "already at {{new_version}}"
    exit 0
  fi
  sed -i.bak 's/zmxVersion = "'"$current"'"/zmxVersion = "{{new_version}}"/' flake.nix && rm flake.nix.bak
  sed -i.bak '0,/^version = "/s/^version = ".*"/version = "{{new_version}}"/' Cargo.toml && rm Cargo.toml.bak
  cargo update -p zmx --offline 2>/dev/null || cargo check -q >/dev/null 2>&1 || true
  echo "bumped zmxVersion: $current → {{new_version}}"

# Cut a release: must be run on master. Bumps zmxVersion in flake.nix,
# commits the bump with a changelog-style message built from commits
# since the last v* tag, pushes master, then signs and pushes the
# v{{version}} tag. The "v" prefix is added for you, so pass the
# semver without it. Usage: just release 0.4.2
#
# Use `just tag <version> <message>` directly if you want to control
# the commit message yourself without bumping.
[group("maint")]
release version:
  #!/usr/bin/env bash
  set -euo pipefail
  current_branch=$(git rev-parse --abbrev-ref HEAD)
  if [[ "$current_branch" != "master" ]]; then
    echo "just release must be run on master (currently on $current_branch)" >&2
    exit 1
  fi
  prev=$(git tag --sort=-v:refname -l "v*" | head -1)
  header="release v{{version}}"
  if [[ -n "$prev" ]]; then
    summary=$(git log --format='- %s' "$prev"..HEAD)
    if [[ -n "$summary" ]]; then
      msg="$header"$'\n\n'"$summary"
    else
      msg="$header"
    fi
  else
    msg="$header"
  fi
  just bump-version "{{version}}"
  if ! git diff --quiet flake.nix Cargo.toml Cargo.lock; then
    git add flake.nix Cargo.toml Cargo.lock
    git commit -m "chore: release v{{version}}"
    git push origin master
    echo "pushed version bump to master"
  fi
  just tag "{{version}}" "$msg"
