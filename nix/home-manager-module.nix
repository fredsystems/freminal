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

      tabsSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.tabs) show_single_tab position;
      };

      bellSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.bell) mode;
      };

      securitySection = lib.filterAttrs (_: v: v != null) {
        inherit (s.security) allow_clipboard_read;
      };

      keybindingsSection = s.keybindings;

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
      // lib.optionalAttrs (uiSection != { }) { ui = uiSection; }
      // lib.optionalAttrs (tabsSection != { }) { tabs = tabsSection; }
      // lib.optionalAttrs (bellSection != { }) { bell = bellSection; }
      // lib.optionalAttrs (securitySection != { }) { security = securitySection; }
      // lib.optionalAttrs (keybindingsSection != { }) { keybindings = keybindingsSection; };
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

      tabs = {
        show_single_tab = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Whether to show the tab bar when only one tab is open.
            Null uses the default (false).
          '';
        };

        position = mkOption {
          type = types.nullOr (
            types.enum [
              "top"
              "bottom"
            ]
          );
          default = null;
          description = ''
            Position of the tab bar: "top" or "bottom".
            Null uses the default ("top").
          '';
        };
      };

      bell = {
        mode = mkOption {
          type = types.nullOr (
            types.enum [
              "visual"
              "none"
            ]
          );
          default = null;
          description = ''
            How the terminal responds to a bell character.
            "visual" flashes the terminal area; "none" ignores it.
            Null uses the default ("visual").
          '';
        };
      };

      security = {
        allow_clipboard_read = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Allow applications to read the system clipboard via OSC 52 query.
            When true, OSC 52 queries return the clipboard contents base64-encoded.
            Null uses the default (false).
          '';
        };
      };

      keybindings = mkOption {
        type = types.attrsOf types.str;
        default = { };
        example = lib.literalExpression ''
          {
            copy = "Ctrl+Shift+C";
            paste = "Ctrl+Shift+V";
            new_tab = "Ctrl+Shift+T";
            zoom_in = "Ctrl+Plus";
          }
        '';
        description = ''
          Key binding overrides. Each key is an action name (snake_case) and
          each value is a combo string like "Ctrl+Shift+T". Set a value to
          "" or "none" to unbind an action. Only overridden actions need to
          be listed — all others keep their defaults.

          Available actions: new_tab, close_tab, next_tab, prev_tab,
          switch_to_tab_1 through switch_to_tab_9, move_tab_left,
          move_tab_right, rename_tab, copy, paste, select_all, open_search,
          zoom_in, zoom_out, zoom_reset, toggle_menu_bar, open_settings,
          scroll_page_up, scroll_page_down, scroll_to_top, scroll_to_bottom,
          scroll_line_up, scroll_line_down.
        '';
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
