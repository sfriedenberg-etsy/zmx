{
  description = "zmx - session persistence for terminal processes";

  inputs = {
    utils.url = "https://flakehub.com/f/numtide/flake-utils/0.1.102";
    nixpkgs-master.url = "github:NixOS/nixpkgs/e034e386767a6d00b65ac951821835bd977a08f7";
    nixpkgs.url = "github:NixOS/nixpkgs/3e20095fe3c6cbb1ddcef89b26969a69a1570776";
    zig2nix = {
      url = "github:Cloudef/zig2nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      zig2nix,
      nixpkgs,
      nixpkgs-master,
      utils,
      ...
    }:
    (utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ] (
      system:
      let
        env = zig2nix.outputs.zig-env.${system} {
          zig = zig2nix.outputs.packages.${system}.zig-0_15_2;
        };
        pkgs = env.pkgs;
      in
      let
        zmx-libvterm = env.package {
          src = pkgs.lib.cleanSource ./.;
          zigBuildFlags = [
            "-Doptimize=ReleaseSafe"
            "-Dbackend=libvterm"
          ];
          buildInputs = [ pkgs.libvterm-neovim ];
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # Remove ghostty dependency from zon file - not needed for libvterm backend
          postPatch = ''
            sed -i '/\.dependencies = \.{/,/},/{/\.ghostty/,/},/d;}' build.zig.zon
          '';
          postInstall = ''
            wrapProgram $out/bin/zmx \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ pkgs.libvterm-neovim ]} \
              --prefix DYLD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ pkgs.libvterm-neovim ]}
          '';
        };
      in
      {
        packages = {
          zmx-libvterm = zmx-libvterm;
          # ghostty backend requires network access for git dependency,
          # which is unavailable in the Nix sandbox
          default = zmx-libvterm;
        };

        apps = {
          default = {
            type = "app";
            program = "${zmx-libvterm}/bin/zmx";
          };

          build = env.app [ ] "zig build -Dbackend=libvterm \"$@\"";

          test = env.app [ ] "zig build -Dbackend=libvterm test -- \"$@\"";
        };

        devShells.default = env.mkShell {
          buildInputs = [
            pkgs.just
            pkgs.libvterm-neovim
          ];
        };
      }
    ));
}
