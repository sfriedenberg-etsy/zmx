{
  description = "zmx - session persistence for terminal processes";

  inputs = {
    utils.url = "https://flakehub.com/f/numtide/flake-utils/0.1.102";
    nixpkgs-master.url = "github:NixOS/nixpkgs/e2dde111aea2c0699531dc616112a96cd55ab8b5";
    nixpkgs.url = "github:NixOS/nixpkgs/3e20095fe3c6cbb1ddcef89b26969a69a1570776";
  };

  outputs =
    {
      self,
      nixpkgs,
      nixpkgs-master,
      utils,
      ...
    }:
    let
      # Burnt into the binary via -Dversion / -Dcommit. Single source of
      # truth for the release version; bump this line and tag.
      zmxVersion = "0.4.1";
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
          callZmx =
            args:
            pkgs.callPackage ./package.nix (
              {
                version = zmxVersion;
                commit = zmxCommit;
              }
              // args
            );
        in
        {
          packages = {
            zmx-libvterm = callZmx { useLibvterm = true; };
            default = callZmx { useLibvterm = true; };
          };

          devShells.default = pkgs.mkShell {
            buildInputs = [
              pkgs.just
              pkgs.zig_0_15
              pkgs.libvterm-neovim
            ];
          };
        }
      )
    );
}
