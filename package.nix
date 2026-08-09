{
  lib,
  rustPlatform,
  # Defaulted so that non-flake consumers (e.g. direct `nix-build` against
  # this file) still work; flake.nix overrides both for release builds.
  version ? "dev",
  commit ? "unknown",
}:

rustPlatform.buildRustPackage {
  pname = "zmx";
  inherit version;

  src = lib.cleanSource ./.;

  cargoLock.lockFile = ./Cargo.lock;

  # Burnt into the binary by build.rs (`zmx version` output). The sandboxed
  # source has no .git, so the commit must come in via the environment.
  env = {
    ZMX_VERSION = version;
    ZMX_COMMIT = commit;
  };

  meta = {
    description = "Session persistence for terminal processes";
    mainProgram = "zmx";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
      "x86_64-darwin"
    ];
  };
}
