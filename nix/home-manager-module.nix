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
{ freminal-flake }:
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

  # Resolve the package from the flake directly — no overlay required.
  defaultPackage = freminal-flake.packages.${pkgs.stdenv.hostPlatform.system}.freminal;

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

      uiSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.ui) hide_menu_bar background_opacity;
      };

      result = {
        version = 1;
        managed_by = "home-manager";
        cursor = cursorSection;
        theme = themeSection;
        scrollback = scrollbackSection;
      }
      // lib.optionalAttrs (fontSection != { }) { font = fontSection; }
      // lib.optionalAttrs (shellSection != { }) { shell = shellSection; }
      // lib.optionalAttrs (loggingSection != { }) { logging = loggingSection; }
      // lib.optionalAttrs (uiSection != { }) { ui = uiSection; };
    in
    result;
in
{
  options.programs.freminal = {
    enable = mkEnableOption "Freminal terminal emulator";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "freminal.packages.\${pkgs.stdenv.hostPlatform.system}.freminal";
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
          # Sorted alphabetically by slug. Keep in sync with ALL_THEMES in
          # freminal-common/src/themes.rs.
          type = types.enum [
            "ayu-dark"
            "ayu-light"
            "catppuccin-frappe"
            "catppuccin-latte"
            "catppuccin-macchiato"
            "catppuccin-mocha"
            "dracula"
            "everforest-dark"
            "everforest-light"
            "ghostty-default"
            "gruvbox-dark"
            "gruvbox-light"
            "kanagawa"
            "material-dark"
            "monokai-pro"
            "nord"
            "one-dark"
            "one-light"
            "rose-pine"
            "rose-pine-dawn"
            "rose-pine-moon"
            "solarized-dark"
            "solarized-light"
            "tokyo-night"
            "tokyo-night-storm"
            "wezterm-default"
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

      ui = {
        hide_menu_bar = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Hide the menu bar at the top of the window.
            Null uses the default (false).
          '';
        };

        background_opacity = mkOption {
          type = types.nullOr (types.addCheck types.float (x: x >= 0.0 && x <= 1.0));
          default = null;
          description = ''
            Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
            Null uses the default (1.0).
          '';
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
