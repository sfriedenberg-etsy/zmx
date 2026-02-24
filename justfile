
default: build test

build:
  nix build .#zmx-libvterm

check:
  nix run .#build -- check

test:
  nix run .#test

# Update zig2nix dependency hashes from build.zig.zon
zig2nix:
  nix run github:Cloudef/zig2nix#zon2nix -- build.zig.zon
