{
  description = "zmx - session persistence for terminal processes";

  inputs = {
    utils.url = "https://flakehub.com/f/numtide/flake-utils/0.1.102";
    nixpkgs-master.url = "github:NixOS/nixpkgs/567a49d1913ce81ac6e9582e3553dd90a955875f";
    nixpkgs.url = "github:NixOS/nixpkgs/3e20095fe3c6cbb1ddcef89b26969a69a1570776";
    # amarbel-llc/bats exposes `batsLane` (lifted from the
    # amarbel-llc/nixpkgs overlay) so consumers don't need to pull
    # the fork's nixpkgs just for the lane builder. We follow our
    # main nixpkgs into the bats flake's nixpkgs slot.
    bats.url = "github:amarbel-llc/bats";
  };

  outputs =
    {
      self,
      nixpkgs,
      nixpkgs-master,
      bats,
      utils,
      ...
    }:
    let
      # Burnt into the binary via ZMX_VERSION / ZMX_COMMIT (see build.rs).
      # Single source of truth for the release version; bump this line
      # (and Cargo.toml, via `just bump-version`) and tag.
      zmxVersion = "0.16.2";
      # shortRev for clean builds, dirty-prefixed dirtyShortRev for dirty
      # working trees so devshell builds don't masquerade as clean
      # releases. "unknown" as a last-resort fallback.
      zmxCommit =
        if self ? shortRev then
          self.shortRev
        else if self ? dirtyShortRev then
          # dirtyShortRev is "<sha>-dirty"; rewrite to "dirty-<sha>" so
          # the dirty marker reads as a prefix in the version output.
          "dirty-${nixpkgs.lib.removeSuffix "-dirty" self.dirtyShortRev}"
        else
          "unknown";
    in
    (utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          zmx = pkgs.callPackage ./package.nix {
            version = zmxVersion;
            commit = zmxCommit;
          };

          batsLib = import ./bats.nix {
            inherit pkgs;
            batsLane = bats.lib.${system}.batsLane;
            bats-libs = bats.packages.${system}.bats-libs;
            zmxBin = zmx;
            batsSrc = pkgs.lib.cleanSourceWith {
              src = ./zz-tests_bats;
              filter =
                path: type:
                type == "directory"
                || pkgs.lib.hasSuffix ".bats" path
                || baseNameOf path == "common.bash"
                || baseNameOf path == "setup_suite.bash";
            };
          };
        in
        {
          packages = batsLib.batsLaneOutputs // {
            inherit zmx;
            default = zmx;
          };

          checks = {
            bats-default = batsLib.batsLaneOutputs.bats-default;
          };

          devShells.default = pkgs.mkShell {
            buildInputs = [
              pkgs.just
              pkgs.cargo
              pkgs.rustc
              pkgs.rustfmt
              pkgs.clippy
            ];
          };
        }
      )
    );
}
