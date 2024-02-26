{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    nci.url = "github:yusdacra/nix-cargo-integration";
    nci.inputs.nixpkgs.follows = "nixpkgs";
    parts.url = "github:hercules-ci/flake-parts";
    parts.inputs.nixpkgs-lib.follows = "nixpkgs";
    devshell.url = "github:numtide/devshell";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = inputs @ {
    parts,
    nci,
    devshell,
    rust-overlay,
    nixpkgs,
    ...
  }:
    parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];
      imports = [nci.flakeModule parts.flakeModules.easyOverlay devshell.flakeModule];
      perSystem = {
        config,
        pkgs,
        system,
        inputs',
        lib,
        self',
        ...
      }: let
        crateName = "wayper";
        # shorthand for accessing this crate's outputs
        # you can access crate outputs under `config.nci.outputs.<crate name>` (see documentation)
        crateOutputs = config.nci.outputs.${crateName};
        libPath = with pkgs;
          lib.makeLibraryPath
          [
            libGL
            libxkbcommon
            wayland
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
          ];
      in rec {
        # use oxalica/rust-overlay
        _module.args.pkgs = import nixpkgs {
          inherit system;
          overlays = [rust-overlay.overlays.default];
        };

        # relPath is empty to denote current dir
        nci.projects.${crateName}.path = ./.;

        nci.crates.${crateName} = {
          # export crate (packages and devshell) in flake outputs
          export = true;

          # overrides
          drvConfig = {
            mkDerivation = {
              nativeBuildInputs = [pkgs.wayland-protocols pkgs.makeWrapper];
              buildInputs = [pkgs.pkg-config pkgs.openssl.dev pkgs.openssl pkgs.perl];
              # postInstall = ''
              #   wrapProgram "$out/bin/wayper" --prefix LD_LIBRARY_PATH : "${libPath}"
              # '';
            };
          };

          # dependency overrides
          depsDrvConfig = {
            mkDerivation = {
              nativeBuildInputs = [pkgs.wayland-protocols pkgs.libxkbcommon];
              buildInputs = [pkgs.pkg-config pkgs.openssl.dev pkgs.openssl pkgs.perl];
            };
          };
          runtimeLibs = with pkgs; [
            libGL
            libxkbcommon
            wayland
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
          ];
        };

        nci.toolchains = {
          build =
            pkgs.rust-bin.stable.latest.minimal;
        };

        # use numtide/devshell
        devshells.default = with pkgs; {
          motd = ''
            -----------------
            -wayper devshell-
            -----------------
            $(type -p menu &>/dev/null && menu)
          '';
          env = [
            {
              name = "RUST_SRC_PATH";
              value = rustPlatform.rustLibSrc;
            }
            {
              name = "LD_LIBRARY_PATH";
              value = libPath;
            }
            {
              name = "PKG_CONFIG_PATH";
              value = "${pkgs.libxkbcommon.dev}/lib/pkgconfig";
            }
          ];

          packages = [
            (rust-bin.stable.latest.default.override {
              extensions = ["rust-src" "rust-analyzer"];
            })
            just
            pkg-config
          ];

          commands = [
            {
              name = "run-${crateName}";
              command = "RUST_LOG=debug nix run .#${crateName}-dev";
              help = "Run ${crateName} (debug build)";
              category = "Run";
            }
            {
              name = "run-${crateName}-rel";
              command = "RUST_LOG=debug nix run .#${crateName}-rel";
              help = "Run ${crateName} (release build)";
              category = "Run";
            }
            {
              name = "build-${crateName}";
              command = "RUST_LOG=debug nix build .#${crateName}-dev";
              help = "Build ${crateName} (debug build)";
              category = "Build";
            }
            {
              name = "build-${crateName}-rel";
              command = "RUST_LOG=debug nix build .#${crateName}-rel";
              help = "Build ${crateName} (release build)";
              category = "Build";
            }
          ];
        };

        # export the release package of the crate as default package
        packages.default = crateOutputs.packages.release;

        # export overlay using easyOverlays
        overlayAttrs = {
          inherit (config.packages) wayper;
          /*
          inherit (inputs.rust-overlay.overlays) default;
          */
        };
        packages.wayper = crateOutputs.packages.release;
      };
      flake = {
        homeManagerModules = {
          wayper = import ./nix/hm-module.nix inputs.self;
          default = inputs.self.homeManagerModules.wayper;
        };
      };
    };
}
