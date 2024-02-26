self: {
  config,
  pkgs,
  lib,
  ...
}: let
  inherit (builtins) toString;
  inherit (lib.types) int str package listOf submodule;
  inherit (lib.modules) mkIf;
  inherit (lib.options) mkOption mkEnableOption;
  cfg = config.programs.wayper;
in {
  options.programs.wayper = {
    enable = mkEnableOption "Wayper, the homebrewed wallpaper daemon";
    package = mkOption {
      description = "The wayper package";
      type = package;
      default =
        self.packages.${pkgs.stdenv.hostPlatform.system}.wayper;
    };
    monitorConfigs = mkOption {
      description = "list of monitors with their respective configuration";
      type = listOf (submodule {
        name = mkOption {
          description = "the name of the monitor";
          type = str;
          default = "eDP-1";
        };
        duration = mkOption {
          description = "the interval between wallpaper cycling (will be ignored if only one file is given)";
          type = int;
          default = 30;
        };
        path = mkOption {
          description = "path to wallpaper file(s)";
          type = str;
          default = "/home/example/wallpapers";
        };
      });
      default = [{}];
    };
  };

  config = mkIf cfg.enable {
    home.packages = [cfg.package];

    xdg.configFile."wayper/config.toml".text = ''
      ${builtins.concatStringsSep "\n" (map (monitor: ''
          [${monitor.name}]
          name = "${monitor.name}"
          duration = ${monitor.duration}
          path = "${monitor.path}"
        '')
        cfg.monitorConfigs)}
    '';
  };
}
