build:
  nix develop -c zig build

check:
  nix develop -c zig build check

test:
  nix develop -c zig build test

build-nix:
  nix build

build-libvterm:
  nix build .#zmx-libvterm

# Update zig2nix dependency hashes from build.zig.zon
zig2nix:
  nix run github:Cloudef/zig2nix#zon2nix -- build.zig.zon
