{
  description = "zmx - session persistence for terminal processes";

  inputs = {
    utils.url = "https://flakehub.com/f/numtide/flake-utils/0.1.102";
    nixpkgs-master.url = "github:NixOS/nixpkgs/b28c4999ed71543e71552ccfd0d7e68c581ba7e9";
    nixpkgs.url = "github:NixOS/nixpkgs/23d72dabcb3b12469f57b37170fcbc1789bd7457";
    zig2nix.url = "github:Cloudef/zig2nix";
  };

  outputs =
    {
      zig2nix,
      nixpkgs,
      nixpkgs-master,
      utils,
      ...
    }:
    (utils.lib.eachDefaultSystem (
      system:
      let
        env = zig2nix.outputs.zig-env.${system} {
          zig = zig2nix.outputs.packages.${system}.zig-0_15_2;
        };
        pkgs = env.pkgs;
      in
      with builtins;
      with pkgs.lib;
      let
        zmx-package = env.package {
          src = cleanSource ./.;
          zigBuildFlags = [ "-Doptimize=ReleaseSafe" ];
        };
        zmx-libvterm = env.package {
          src = cleanSource ./.;
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
          zmx = zmx-package;
          zmx-libvterm = zmx-libvterm;
          default = zmx-package;
        };

        apps = {
          zmx = {
            type = "app";
            program = "${zmx-package}/bin/zmx";
          };
          default = {
            type = "app";
            program = "${zmx-package}/bin/zmx";
          };

          build = env.app [ ] "zig build \"$@\"";

          test = env.app [ ] "zig build test -- \"$@\"";
        };

        devShells.default = env.mkShell {
          buildInputs = [ pkgs.libvterm-neovim ];
        };
      }
    ));
}
