{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    devshell.url = "github:numtide/devshell";
    naersk.url = "github:nix-community/naersk";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    parts,
    devshell,
    nixpkgs,
    naersk,
    fenix,
    ...
  }:
    parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux" "aarch64-linux"];
      imports = [parts.flakeModules.easyOverlay devshell.flakeModule];
      perSystem = {
        config,
        pkgs,
        system,
        lib,
        ...
      }: let
        crateName = "wayper";

        toolchain = fenix.packages.${system}.stable.toolchain;

        naersk' = pkgs.callPackage naersk {
          cargo = toolchain;
          rustc = toolchain;
        };

        wayper = {release ? true}:
          naersk'.buildPackage {
            src = ./.;
            nativeBuildInputs = with pkgs; [pkg-config];
            buildInputs = with pkgs; [libxkbcommon mpv];
            inherit release;
          };
      in rec {
        _module.args.pkgs = import nixpkgs {
          inherit system;
          overlays = [fenix.overlays.default];
          config.allowUnfree = true;
          config.permittedInsecurePackages = ["tightvnc-1.3.10"];
        };

        packages.default = packages.wayper;
        packages.wayper = packages.release;
        packages.release = wayper {release = true;};
        packages.debug = wayper {release = false;};

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
              name = "PKG_CONFIG_PATH";
              value = "${pkgs.libxkbcommon.dev}/lib/pkgconfig:${pkgs.wayland.dev}/lib/pkgconfig:${pkgs.mpv}/lib/pkgconfig";
            }
          ];

          packages = [
            # (rust-bin.stable.latest.default.override {
            #   extensions = ["rust-src" "rust-analyzer"];
            # })
            toolchain
            just
            pkg-config
            ripgrep
            stdenv.cc
          ];

          packagesFrom = [packages.default];

          commands = [
            {
              name = "nix-run-${crateName}";
              command = "RUST_LOG=debug nix run .#${crateName}-dev";
              help = "Run ${crateName} (debug build)";
              category = "Run";
            }
            {
              name = "nix-run-${crateName}-rel";
              command = "RUST_LOG=debug nix run .#${crateName}-rel";
              help = "Run ${crateName} (release build)";
              category = "Run";
            }
            {
              name = "nix-build-${crateName}";
              command = "RUST_LOG=debug nix build .#${crateName}-dev";
              help = "Build ${crateName} (debug build)";
              category = "Build";
            }
            {
              name = "nix-build-${crateName}-rel";
              command = "RUST_LOG=debug nix build .#${crateName}-rel";
              help = "Build ${crateName} (release build)";
              category = "Build";
            }
            {
              name = "headless";
              command = ''
                #!/usr/bin/env bash

                 hyprctl monitors | rg HEADLESS | cut -d ' ' -f 2
              '';
            }
          ];
        };

        # export the release package of the crate as default package
        # packages.default = crateOutputs.packages.release;

        # export overlay using easyOverlays
        overlayAttrs = {
          inherit (config.packages) wayper;
          /*
          inherit (inputs.rust-overlay.overlays) default;
          */
        };
      };
      flake = {
        homeManagerModules = {
          wayper = import ./nix/hm-module.nix inputs.self;
          default = inputs.self.homeManagerModules.wayper;
        };
      };
    };
}
