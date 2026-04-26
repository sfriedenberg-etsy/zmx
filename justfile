
default: build test

build:
  nix build .#default

check:
  nix develop -c zig build check -Dbackend=libvterm

test:
  nix develop -c zig build test -Dbackend=libvterm

ghostty_commit := "a692cb9e5fabfd337827cc99cd62e3ea90ab9c92"

# Vendor ghostty dependency into deps/ghostty
vendor:
  rm -rf deps/ghostty
  git clone --no-checkout https://github.com/ghostty-org/ghostty.git deps/ghostty
  cd deps/ghostty && git checkout {{ghostty_commit}}
  rm -rf deps/ghostty/.git

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

# Sed-rewrite zmxVersion in flake.nix to the given semver. The version
# string is burnt into the binary at build time via -Dversion (see
# build.zig and src/main.zig printVersion), so flake.nix is the single
# source of truth. No-op if already at the target version.
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
  echo "bumped zmxVersion: $current → {{new_version}}"

# Cut a release: must be run on main. Bumps zmxVersion in flake.nix,
# commits the bump with a changelog-style message built from commits
# since the last v* tag, pushes main, then signs and pushes the
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
  if [[ "$current_branch" != "main" ]]; then
    echo "just release must be run on main (currently on $current_branch)" >&2
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
  if ! git diff --quiet flake.nix; then
    git add flake.nix
    git commit -m "chore: release v{{version}}"
    git push origin main
    echo "pushed flake.nix bump to main"
  fi
  just tag "{{version}}" "$msg"
