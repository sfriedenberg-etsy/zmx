{
  description = "zmx - session persistence for terminal processes";

  inputs = {
    utils.url = "https://flakehub.com/f/numtide/flake-utils/0.1.102";
    nixpkgs-master.url = "github:NixOS/nixpkgs/5b7e21f22978c4b740b3907f3251b470f466a9a2";
    nixpkgs.url = "github:NixOS/nixpkgs/6d41bc27aaf7b6a3ba6b169db3bd5d6159cfaa47";
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
