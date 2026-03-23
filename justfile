
default: build test

build:
  nix build .#default

check:
  nix run .#build -- check

test:
  nix run .#test

ghostty_commit := "a692cb9e5fabfd337827cc99cd62e3ea90ab9c92"

# Vendor ghostty dependency into deps/ghostty
vendor:
  rm -rf deps/ghostty
  git clone --no-checkout https://github.com/ghostty-org/ghostty.git deps/ghostty
  cd deps/ghostty && git checkout {{ghostty_commit}}
  rm -rf deps/ghostty/.git

# Update zig2nix dependency hashes from build.zig.zon
zig2nix:
  nix run github:Cloudef/zig2nix#zon2nix -- build.zig.zon
