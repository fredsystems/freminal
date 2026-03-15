# Home-manager module for Freminal terminal emulator.
#
# Usage (in your home-manager config):
#
#   imports = [ freminal.homeManagerModules.default ];
#
#   programs.freminal = {
#     enable = true;
#     settings = {
#       font.family = "JetBrainsMono Nerd Font";
#       font.size = 14.0;
#       cursor.shape = "bar";
#       theme.name = "catppuccin-mocha";
#     };
#   };
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib)
    mkEnableOption
    mkOption
    mkIf
    types
    ;
  cfg = config.programs.freminal;

  # Use pkgs.formats.toml to convert a Nix attrset into a TOML derivation.
  tomlFormat = pkgs.formats.toml { };

  # Build the full config attrset from the user's settings.
  # We always inject `version = 1` and only include sections that the user
  # has explicitly configured (omitting empty/default subsections keeps the
  # generated file clean).
  configAttrset =
    let
      s = cfg.settings;

      # Only include font section keys that are set.
      fontSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.font) family size ligatures;
      };

      cursorSection = {
        inherit (s.cursor) shape blink;
      };

      themeSection = {
        inherit (s.theme) name;
      };

      # Only include shell section if path is set.
      shellSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.shell) path;
      };

      # Only include logging section keys that are set.
      loggingSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.logging) level;
      };

      scrollbackSection = {
        inherit (s.scrollback) limit;
      };

      result = {
        version = 1;
        cursor = cursorSection;
        theme = themeSection;
        scrollback = scrollbackSection;
      }
      // lib.optionalAttrs (fontSection != { }) { font = fontSection; }
      // lib.optionalAttrs (shellSection != { }) { shell = shellSection; }
      // lib.optionalAttrs (loggingSection != { }) { logging = loggingSection; };
    in
    result;
in
{
  options.programs.freminal = {
    enable = mkEnableOption "Freminal terminal emulator";

    package = mkOption {
      type = types.package;
      default = pkgs.freminal;
      defaultText = lib.literalExpression "pkgs.freminal";
      description = "The Freminal package to install.";
    };

    settings = {
      font = {
        family = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = ''
            Primary terminal font family.
            When null, the bundled MesloLGS Nerd Font Mono is used.
          '';
        };

        size = mkOption {
          type = types.nullOr (types.addCheck types.float (x: x >= 4.0 && x <= 96.0));
          default = null;
          description = "Font size in points (4.0–96.0). Null uses the default (12.0).";
        };

        ligatures = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Enable OpenType ligatures (liga, clig).
            Null uses the default (true).
          '';
        };
      };

      cursor = {
        shape = mkOption {
          type = types.enum [
            "block"
            "underline"
            "bar"
          ];
          default = "block";
          description = "Cursor shape: block, underline, or bar.";
        };

        blink = mkOption {
          type = types.bool;
          default = true;
          description = "Whether the cursor should blink.";
        };
      };

      theme = {
        name = mkOption {
          type = types.enum [
            "catppuccin-mocha"
            "catppuccin-macchiato"
            "catppuccin-frappe"
            "catppuccin-latte"
            "dracula"
            "nord"
            "solarized-dark"
            "solarized-light"
            "gruvbox-dark"
            "gruvbox-light"
            "one-dark"
            "one-light"
            "tokyo-night"
            "tokyo-night-storm"
            "kanagawa"
            "rose-pine"
            "rose-pine-moon"
            "rose-pine-dawn"
            "monokai-pro"
            "ayu-dark"
            "ayu-light"
            "everforest-dark"
            "everforest-light"
            "material-dark"
            "xterm-default"
          ];
          default = "catppuccin-mocha";
          description = "Color theme name (must be a recognized built-in slug).";
        };
      };

      shell = {
        path = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = ''
            Default shell to launch.
            When null, the system default shell is used.
          '';
        };
      };

      logging = {
        level = mkOption {
          type = types.nullOr (
            types.enum [
              "trace"
              "debug"
              "info"
              "warn"
              "error"
            ]
          );
          default = null;
          description = ''
            Log level for file output.
            Null uses the default ("debug").
          '';
        };
      };

      scrollback = {
        limit = mkOption {
          type = types.ints.between 1 100000;
          default = 4000;
          description = "Maximum number of scrollback lines (1–100000).";
        };
      };
    };
  };

  config = mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."freminal/config.toml" = {
      source = tomlFormat.generate "freminal-config" configAttrset;
    };
  };
}
