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
      in {
        # use oxalica/rust-overlay
        _module.args.pkgs = import nixpkgs {
          inherit system;
          overlays = [rust-overlay.overlays.default];
        };

        # relPath is empty to denote current dir
        nci.projects.${crateName}.relPath = "";

        nci.crates.${crateName} = {
          # export crate (packages and devshell) in flake outputs
          export = true;

          # overrides
          overrides = {
            add-inputs.overrideAttrs = old: {
              nativeBuildInputs = (old.nativeBuildInputs or []) ++ [pkgs.wayland-protocols pkgs.makeWrapper];
              buildInputs = (old.buildInputs or []) ++ [pkgs.pkg-config pkgs.openssl.dev pkgs.openssl pkgs.perl];
              postInstall = ''
                wrapProgram "$out/bin/wayper" --prefix LD_LIBRARY_PATH : "${libPath}"
              '';
            };
          };

          # dependency overrides
          depsOverrides = {
            add-inputs.overrideAttrs = old: {
              nativeBuildInputs = (old.nativeBuildInputs or []) ++ [pkgs.wayland-protocols];
              buildInputs = (old.buildInputs or []) ++ [pkgs.pkg-config pkgs.openssl.dev pkgs.openssl pkgs.perl];
            };
          };
        };

        nci.toolchains = {
          build = {
            package = pkgs.rust-bin.stable.latest.minimal;
          };
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
          ];

          packages = [
            (rust-bin.stable.latest.default.override {
              extensions = ["rust-src" "rust-analyzer"];
            })
            just
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
          ];
        };

        # export the release package of the crate as default package
        packages.default = crateOutputs.packages.release;

        # export overlay using easyOverlays
        overlayAttrs = {
          inherit (config.packages) wayper;
        };
        packages.wayper = crateOutputs.packages.release;
      };
    };
}
