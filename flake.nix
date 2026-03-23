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
      with builtins;
      with pkgs.lib;
      let
        isDarwin = pkgs.stdenv.isDarwin;
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
        defaultPackage = if isDarwin then zmx-libvterm else zmx-package;
      in
      {
        packages = {
          zmx-libvterm = zmx-libvterm;
          default = defaultPackage;
        }
        // optionalAttrs (!isDarwin) {
          zmx = zmx-package;
        };

        apps = {
          default = {
            type = "app";
            program = "${defaultPackage}/bin/zmx";
          };

          build = env.app [ ] (
            if isDarwin then "zig build -Dbackend=libvterm \"$@\"" else "zig build \"$@\""
          );

          test = env.app [ ] (
            if isDarwin then "zig build -Dbackend=libvterm test -- \"$@\"" else "zig build test -- \"$@\""
          );
        }
        // optionalAttrs (!isDarwin) {
          zmx = {
            type = "app";
            program = "${zmx-package}/bin/zmx";
          };
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
