{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    nci.url = "github:yusdacra/nix-cargo-integration";
    nci.inputs.nixpkgs.follows = "nixpkgs";
    parts.url = "github:hercules-ci/flake-parts";
    parts.inputs.nixpkgs-lib.follows = "nixpkgs";
    devshell.url = "github:numtide/devshell";
  };

  outputs =
    inputs @ { parts
    , nci
    , devshell
    , nixpkgs
    , ...
    }:
    parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" ];
      imports = [ nci.flakeModule ];
      perSystem = { config, pkgs, system, inputs', ... }:
        let
          crateName = "wayper";
          # shorthand for accessing this crate's outputs
          # you can access crate outputs under `config.nci.outputs.<crate name>` (see documentation)
          crateOutputs = config.nci.outputs.${crateName};
          libPath = with pkgs; lib.makeLibraryPath
            [
              libGL
              libxkbcommon
              wayland
              xorg.libX11
              xorg.libXcursor
              xorg.libXi
              xorg.libXrandr

            ];
        in
        {
          # declare projects
          # relPath is the relative path of a project to the flake root
          # TODO: change this to your crate's path
          nci.projects.${crateName}.relPath = "";
          # configure crates
          nci.crates.${crateName} = {
            # export crate (packages and devshell) in flake outputs
            # alternatively you can access the outputs and export them yourself (see below)
            export = true;
            # look at documentation for more options
            overrides = {
              add-inputs.overrideAttrs = old: {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.wayland-protocols pkgs.makeWrapper ];
                buildInputs = (old.buildInputs or [ ]) ++ [ pkgs.pkg-config pkgs.openssl.dev pkgs.openssl pkgs.perl ];
                postInstall = ''
                  wrapProgram "$out/bin/wayper" --prefix LD_LIBRARY_PATH : "${libPath}"
                '';
              };
            };
            depsOverrides = {
              add-inputs.overrideAttrs = old: {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.wayland-protocols ];
                buildInputs = (old.buildInputs or [ ]) ++ [ pkgs.pkg-config pkgs.openssl.dev pkgs.openssl pkgs.perl ];
              };
            };
          };
          # export the crate devshell as the default devshell
          devShells.default = with pkgs; mkShell {
            buildInputs = [
              cargo
              rust-analyzer
              rustc
              rustfmt
              just
            ];

            RUST_SRC_PATH = rustPlatform.rustLibSrc;
            LD_LIBRARY_PATH = libPath;

          };

          # export the release package of the crate as default package
          packages.default = crateOutputs.packages.release;
        };
    };
}

