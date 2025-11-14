self: {
  config,
  pkgs,
  lib,
  ...
}: let
  inherit (builtins) toString;
  inherit (lib.types) int str nullOr package listOf submodule bool float enum either;
  inherit (lib.modules) mkIf mkMerge;
  inherit (lib.options) mkOption mkEnableOption;
  inherit (lib.meta) getExe;
  cfg = config.services.wayper;

  # Helper to convert transition type to TOML value
  transitionTypeToToml = type:
    if builtins.isList type
    then "[${builtins.concatStringsSep ", " (map (t: "\"${t}\"") type)}]"
    else "\"${type}\"";

  # Sweep direction enum
  sweepDirections = enum [
    "left-to-right"
    "right-to-left"
    "top-to-bottom"
    "bottom-to-top"
    "top-left-to-bottom-right"
    "top-right-to-bottom-left"
    "bottom-left-to-top-right"
    "bottom-right-to-top-left"
  ];

  # Transition type enum
  transitionTypeEnum = enum ["crossfade" "sweep"];

  # Transition configuration submodule
  transitionSubmodule = submodule {
    options = {
      type = mkOption {
        description = "Transition type (single value or list for random selection)";
        type = nullOr (either transitionTypeEnum (listOf transitionTypeEnum));
        default = null;
      };
      duration_ms = mkOption {
        description = "Transition duration in milliseconds";
        type = nullOr int;
        default = null;
      };
      fps = mkOption {
        description = "Transition frames per second";
        type = nullOr int;
        default = null;
      };
      sweep = {
        direction = mkOption {
          description = "Sweep direction";
          type = nullOr sweepDirections;
          default = null;
        };
        edge_width = mkOption {
          description = "Sweep edge width (0.0-1.0)";
          type = nullOr float;
          default = null;
        };
      };
    };
  };

  # Generate transition block TOML
  # prefix is like "transition" or "profile.output.transition"
  generateTransitionBlock = prefix: transition: let
    typeStr =
      if transition.type != null
      then "type = ${transitionTypeToToml transition.type}\n"
      else "";
    durationStr =
      if transition.duration_ms != null
      then "duration_ms = ${toString transition.duration_ms}\n"
      else "";
    fpsStr =
      if transition.fps != null
      then "fps = ${toString transition.fps}\n"
      else "";
    sweepStr =
      if transition.sweep.direction != null || transition.sweep.edge_width != null
      then let
        dirStr =
          if transition.sweep.direction != null
          then "  direction = \"${transition.sweep.direction}\"\n"
          else "";
        edgeStr =
          if transition.sweep.edge_width != null
          then "  edge_width = ${toString transition.sweep.edge_width}\n"
          else "";
      in "\n[${prefix}.sweep]\n${dirStr}${edgeStr}"
      else "";
  in "${typeStr}${durationStr}${fpsStr}${sweepStr}";
in {
  options.services.wayper = {
    enable = mkEnableOption "Wayper, the homebrewed wallpaper daemon";
    package = mkOption {
      description = "The wayper package";
      type = package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.wayper;
    };
    enableFuzzelIntegration = mkEnableOption "Use the fuzzel launcher to control the daemon. Enables the fuzzel hm option.";
    enableFishCompletions = mkEnableOption "Enables installing the fish completions";

    config = {
      default-profile = mkOption {
        description = "The default profile";
        type = str;
      };

      transitions_enabled = mkEnableOption "Globally enable transitions";

      transition = mkOption {
        description = "Global transition configuration";
        type = transitionSubmodule;
        default = {};
      };

      monitorConfigs = mkOption {
        description = "List of monitors with their respective configuration";
        type = listOf (submodule {
          options = {
            name = mkOption {
              description = "The name of the monitor";
              type = str;
              default = "eDP-1";
            };
            profile = mkOption {
              description = "Profile to apply this config";
              type = str;
              default = "default";
            };
            duration = mkOption {
              description = "The interval between wallpaper cycling in seconds (ignored if only one file is given)";
              type = int;
              default = 30;
            };
            path = mkOption {
              description = "Path to wallpaper file(s)";
              type = str;
              default = "/home/example/wallpapers";
            };
            run_command = mkOption {
              description = "Command to run when image is switched";
              type = nullOr str;
              default = null;
            };
            transitions_enabled = mkOption {
              description = "Enable transitions for this monitor (overrides global setting)";
              type = nullOr bool;
              default = null;
            };
            transition = mkOption {
              description = "Per-monitor transition configuration";
              type = transitionSubmodule;
              default = {};
            };
          };
        });
        default = [{}];
      };
    };
  };

  config = mkMerge [
    (mkIf cfg.enable {
      home.packages = [cfg.package];

      programs.fish.interactiveShellInit = mkIf cfg.enableFishCompletions ''source (COMPLETE=fish wayperctl | psub)'';

      xdg.configFile."wayper/config.toml".text = ''
        # generated by hm wayper module
        default_profile = "${cfg.config.default-profile}"
        transitions_enabled = ${lib.boolToString cfg.config.transitions_enabled}

        ${
          # Global transition block
          let
            hasGlobalTransition =
              cfg.config.transition.type != null
              || cfg.config.transition.duration_ms != null
              || cfg.config.transition.fps != null
              || cfg.config.transition.sweep.direction != null
              || cfg.config.transition.sweep.edge_width != null;
          in
            if hasGlobalTransition
            then "[transition]\n${generateTransitionBlock "transition" cfg.config.transition}"
            else ""
        }
        ${builtins.concatStringsSep "\n" (map (monitor: let
            # Per-monitor transition block generation
            hasMonitorTransition =
              monitor.transition.type != null
              || monitor.transition.duration_ms != null
              || monitor.transition.fps != null
              || monitor.transition.sweep.direction != null
              || monitor.transition.sweep.edge_width != null;
            transitionBlock =
              if hasMonitorTransition
              then "\n[${monitor.profile}.${monitor.name}.transition]\n${generateTransitionBlock "${monitor.profile}.${monitor.name}.transition" monitor.transition}"
              else "";
          in ''
            [${monitor.profile}.${monitor.name}]
            duration = ${toString monitor.duration}
            path = "${monitor.path}"
            ${
              if monitor.run_command != null
              then "run_command = \"${monitor.run_command}\""
              else ""
            }${
              if monitor.transitions_enabled != null
              then "\ntransitions_enabled = ${lib.boolToString monitor.transitions_enabled}"
              else ""
            }${transitionBlock}'')
          cfg.config.monitorConfigs)}
      '';

      systemd.user.services.wayper = {
        Unit = {
          Description = "Wayland wallpaper setter";
          After = ["graphical-session.target"];
        };

        Service = {
          ExecStart = "${getExe cfg.package}";
          Restart = "on-failure";
          RestartSec = 3;
        };

        Install = {
          WantedBy = ["default.target"];
        };
      };
    })
    (mkIf (cfg.enable && cfg.enableFuzzelIntegration) {
      home.packages = [
        self.packages.${pkgs.stdenv.hostPlatform.system}.wayper-launcher
      ];

      programs.fuzzel.enable = true;
    })
  ];
}
