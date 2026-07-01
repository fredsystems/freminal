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
#       theme.dark_name = "catppuccin-mocha";
#       theme.light_name = "catppuccin-latte";
#       theme.mode = "auto";
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
        inherit (s.cursor)
          shape
          blink
          trail
          trail_duration_ms
          ;
      };

      themeSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.theme)
          dark_name
          light_name
          mode
          name
          ;
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
        inherit (s.ui)
          hide_menu_bar
          background_opacity
          background_image
          background_image_mode
          background_image_opacity
          ;
      };

      chromeSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.chrome) profile;
      };

      tabsSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.tabs)
          show_single_tab
          position
          confirm_broadcast
          focus_follows_mouse
          ;
      };

      bellSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.bell) mode;
      };

      securitySection = lib.filterAttrs (_: v: v != null) {
        inherit (s.security) allow_clipboard_read;
      };

      pasteGuardSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.paste_guard)
          enabled
          multiline
          control_chars
          patterns
          pattern_list
          ;
      };

      closeGuardSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.close_guard)
          enabled
          unknown_blocks
          guard_app_quit
          ;
      };

      tabTitleSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.tab_title) policy separator;
      };

      shellIntegrationSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.shell_integration) set_term_program;
      };

      commandBlocksSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.command_blocks)
          enabled
          show_duration
          duration_threshold_secs
          gutter
          ;
      };

      notificationsSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.notifications)
          enabled
          osc_9
          osc_777
          osc_99
          on_command_finished
          command_finished_threshold_secs
          routing_error
          routing_info
          routing_command_finished
          ;
      };

      keybindingsSection = s.keybindings;

      shaderSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.shader) path hot_reload;
      };

      startupSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.startup) layout restore_last_session;
      };

      onboardingSection = lib.filterAttrs (_: v: v != null) {
        inherit (s.onboarding) first_run_complete;
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
      // lib.optionalAttrs (uiSection != { }) { ui = uiSection; }
      // lib.optionalAttrs (chromeSection != { }) { chrome = chromeSection; }
      // lib.optionalAttrs (shaderSection != { }) { shader = shaderSection; }
      // lib.optionalAttrs (tabsSection != { }) { tabs = tabsSection; }
      // lib.optionalAttrs (bellSection != { }) { bell = bellSection; }
      // lib.optionalAttrs (securitySection != { }) { security = securitySection; }
      // lib.optionalAttrs (pasteGuardSection != { }) { paste_guard = pasteGuardSection; }
      // lib.optionalAttrs (closeGuardSection != { }) { close_guard = closeGuardSection; }
      // lib.optionalAttrs (tabTitleSection != { }) { tab_title = tabTitleSection; }
      // lib.optionalAttrs (shellIntegrationSection != { }) {
        shell_integration = shellIntegrationSection;
      }
      // lib.optionalAttrs (commandBlocksSection != { }) { command_blocks = commandBlocksSection; }
      // lib.optionalAttrs (notificationsSection != { }) { notifications = notificationsSection; }
      // lib.optionalAttrs (startupSection != { }) { startup = startupSection; }
      // lib.optionalAttrs (onboardingSection != { }) { onboarding = onboardingSection; }
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
            When null, the bundled CaskaydiaCove Nerd Font is used.
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

        trail = mkOption {
          type = types.bool;
          default = false;
          description = "Enable smooth cursor trail animation.";
        };

        trail_duration_ms = mkOption {
          type = types.ints.unsigned;
          default = 150;
          description = "Duration of the cursor trail animation in milliseconds.";
        };
      };

      theme = {
        dark_name = mkOption {
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
          description = "Dark theme slug (used when mode = \"dark\" or as the dark variant for \"auto\").";
        };

        light_name = mkOption {
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
          default = "catppuccin-latte";
          description = "Light theme slug (used when mode = \"light\" or as the light variant for \"auto\").";
        };

        mode = mkOption {
          type = types.enum [
            "dark"
            "light"
            "auto"
          ];
          default = "dark";
          description = ''
            How to select between dark_name and light_name.
            "dark" always uses dark_name; "light" always uses light_name;
            "auto" follows the OS light/dark preference.
          '';
        };

        name = mkOption {
          # Deprecated alias for dark_name.  Kept for backward compatibility.
          type = types.nullOr (
            types.enum [
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
            ]
          );
          default = null;
          description = ''
            Deprecated. Use dark_name instead.
            When set, this value is written as the TOML `name` field for
            backward compatibility with older Freminal versions.
          '';
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
            Null uses the default ("info").
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

        background_image = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = ''
            Path to a background image displayed behind the terminal grid.
            Supports PNG, JPEG, and WebP. Null disables the background image.
          '';
        };

        background_image_mode = mkOption {
          type = types.nullOr (
            types.enum [
              "fill"
              "fit"
              "cover"
              "tile"
            ]
          );
          default = null;
          description = ''
            How to fit the background image within the terminal viewport.
            "fill" stretches to fill (ignores aspect ratio);
            "fit" letterboxes (preserves aspect ratio, may show empty areas);
            "cover" crops to fill (preserves aspect ratio, default);
            "tile" repeats the image.
            Null uses the default ("cover").
          '';
        };

        background_image_opacity = mkOption {
          type = types.nullOr (types.addCheck types.float (x: x >= 0.0 && x <= 1.0));
          default = null;
          description = ''
            Opacity of the background image (0.0–1.0). Applied on top of the
            image itself; background_opacity then layers over that.
            Null uses the default (0.5).
          '';
        };
      };

      chrome = {
        profile = mkOption {
          type = types.nullOr (
            types.enum [
              "modern"
              "retro"
            ]
          );
          default = null;
          description = ''
            Visual style profile for the non-terminal chrome (menu bar, tabs,
            modals, dialogs, toasts). "modern" uses rounded corners, soft
            borders, and roomier spacing; "retro" uses square corners, crisp
            borders, and denser spacing. Both are fully theme-driven.
            Null uses the default ("modern").
          '';
        };
      };

      shader = {
        path = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = ''
            Path to a custom GLSL fragment shader for post-processing.
            The shader receives these uniforms:
              uniform sampler2D u_terminal  — the terminal framebuffer texture
              uniform vec2      u_resolution — viewport size in pixels
              uniform float     u_time       — elapsed time in seconds
            Null disables the post-processing pass (default — no FBO overhead).
          '';
        };

        hot_reload = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, reload and recompile the shader automatically when the
            file on disk changes.
            Null uses the default (true).
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

        confirm_broadcast = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Whether enabling broadcast input (sending keystrokes to every
            pane in a tab) requires a confirmation dialog the first time it
            is turned on for a tab. Null uses the default (false).
          '';
        };

        focus_follows_mouse = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Whether keyboard focus follows the mouse across split panes.
            When true, mousing into a pane focuses it without a click; when
            false, panes are focused only by clicking. Tab switching is
            always click-to-focus. Null uses the default (true).
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

      paste_guard = {
        enabled = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Master switch for the paste guard. When false, no paste is ever
            intercepted regardless of the per-trigger toggles below.
            Null uses the default (true).
          '';
        };

        multiline = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Confirm any paste containing a newline.
            Null uses the default (true).
          '';
        };

        control_chars = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Confirm any paste containing control characters (ESC, BEL, etc.)
            other than the newline covered by multiline and the bracketed-paste
            markers Freminal wraps the payload with.
            Null uses the default (true).
          '';
        };

        patterns = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, additionally match the payload against pattern_list and
            escalate the warning when a dangerous pattern is found.
            Null uses the default (true).
          '';
        };

        pattern_list = mkOption {
          type = types.nullOr (types.listOf types.str);
          default = null;
          description = ''
            Regex patterns (Rust regex syntax) treated as dangerous. Matched
            only when patterns is true. Malformed patterns are reported and
            skipped at match time.
            Null uses the default dangerous-command pattern set.
          '';
        };
      };

      close_guard = {
        enabled = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Master switch for the close-on-running-command guard. When true,
            closing a pane / tab / window that contains a running foreground
            command (per OSC 133 markers) shows a confirmation dialog.
            Null uses the default (true).
          '';
        };

        unknown_blocks = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, also block close for panes whose command status is
            unknown (no OSC 133 markers ever received).
            Null uses the default (false).
          '';
        };

        guard_app_quit = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, the app-quit path runs the close guard. When false, app
            quit bypasses it and only individual close actions are guarded.
            Null uses the default (true).
          '';
        };
      };

      tab_title = {
        policy = mkOption {
          type = types.nullOr (
            types.enum [
              "prefix"
              "suffix"
              "custom_wins"
              "osc_wins"
            ]
          );
          default = null;
          description = ''
            How a user-assigned custom tab name combines with a shell-set
            (OSC 0/1/2) title.
            "prefix"      shows "{custom}{separator}{osc}";
            "suffix"      shows "{osc}{separator}{custom}";
            "custom_wins" shows only the custom name when set;
            "osc_wins"    lets the shell title clear the custom name.
            Null uses the default ("prefix").
          '';
        };

        separator = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = ''
            Separator inserted between the custom name and the shell title
            under the "prefix"/"suffix" policies.
            Null uses the default (": ").
          '';
        };
      };

      shell_integration = {
        set_term_program = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, Freminal sets TERM_PROGRAM=freminal and
            TERM_PROGRAM_VERSION in the PTY environment and injects its
            shell-integration scripts (OSC 133 command blocks, OSC 7 cwd).
            Null uses the default (true).
          '';
        };
      };

      command_blocks = {
        enabled = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Master switch for OSC 133 command-block visualization. When false,
            markers are still parsed but no gutters, duration overlays, or
            fold/collapse affordances appear.
            Null uses the default (true).
          '';
        };

        show_duration = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Display the duration of long-running commands next to the gutter.
            Null uses the default (true).
          '';
        };

        duration_threshold_secs = mkOption {
          type = types.nullOr types.float;
          default = null;
          description = ''
            Minimum command duration (seconds) before a duration label is
            rendered, to avoid flicker on fast commands.
            Null uses the default (2.0).
          '';
        };

        gutter = mkOption {
          type = types.nullOr (
            types.enum [
              "left"
              "off"
            ]
          );
          default = null;
          description = ''
            Where the per-command status gutter is drawn. "left" reserves a
            thin colored strip on the left edge; "off" disables it.
            Null uses the default ("left").
          '';
        };
      };

      notifications = {
        enabled = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            Master switch for the notification system. When false, no
            notifications (toast or desktop) are produced.
            Null uses the default (false — opt-in).
          '';
        };

        osc_9 = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, OSC 9 (iTerm2/WezTerm) text payloads create
            notifications.
            Null uses the default (true).
          '';
        };

        osc_777 = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, OSC 777 ("notify;TITLE;BODY", urxvt) payloads create
            notifications.
            Null uses the default (true).
          '';
        };

        osc_99 = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, OSC 99 (kitty stateful) notifications are honoured.
            Null uses the default (true).
          '';
        };

        on_command_finished = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, an OSC 133 D (command finished) event fires a
            notification.
            Null uses the default (true).
          '';
        };

        command_finished_threshold_secs = mkOption {
          type = types.nullOr types.float;
          default = null;
          description = ''
            Minimum command duration (seconds) before a command-finished
            notification fires, to avoid spamming on fast commands.
            Null uses the default (10.0).
          '';
        };

        routing_error = mkOption {
          type = types.nullOr (
            types.enum [
              "toast"
              "system"
              "both"
              "system_when_unfocused"
            ]
          );
          default = null;
          description = ''
            Routing for error-category notifications: "toast" (in-app only),
            "system" (desktop only), "both", or "system_when_unfocused"
            (desktop when unfocused, toast when focused).
            Null uses the default ("both").
          '';
        };

        routing_info = mkOption {
          type = types.nullOr (
            types.enum [
              "toast"
              "system"
              "both"
              "system_when_unfocused"
            ]
          );
          default = null;
          description = ''
            Routing for informational notifications (see routing_error for the
            value meanings).
            Null uses the default ("toast").
          '';
        };

        routing_command_finished = mkOption {
          type = types.nullOr (
            types.enum [
              "toast"
              "system"
              "both"
              "system_when_unfocused"
            ]
          );
          default = null;
          description = ''
            Routing for command-finished notifications (see routing_error for
            the value meanings).
            Null uses the default ("system_when_unfocused").
          '';
        };
      };

      startup = {
        layout = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = ''
            Name or path of a layout file to load on startup.
            If a plain name (no path separators), Freminal looks for
            ~/.config/freminal/layouts/<name>.toml.
            Overridden by the --layout CLI flag.
            Null uses the default (single pane, no layout).
          '';
        };

        restore_last_session = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When true, Freminal saves the current layout on exit and restores
            it on the next launch (unless --layout is given on the CLI).
            Null uses the default (false).
          '';
        };
      };

      onboarding = {
        first_run_complete = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = ''
            When false (or unset), Freminal shows the first-run welcome
            overlay on launch. It is automatically set to true once the user
            skips or completes the overlay. Users can re-trigger the overlay
            at any time via Help -> Show Welcome.
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

    # On Linux, Freminal reads $XDG_CONFIG_HOME/freminal/config.toml.
    # On macOS, it reads ~/Library/Application Support/Freminal/config.toml.
    xdg.configFile."freminal/config.toml" = lib.mkIf (!pkgs.stdenv.isDarwin) {
      source = tomlFormat.generate "freminal-config" configAttrset;
    };
    home.file."Library/Application Support/Freminal/config.toml" = lib.mkIf pkgs.stdenv.isDarwin {
      source = tomlFormat.generate "freminal-config" configAttrset;
    };

    # On macOS, Home Manager automatically copies .app bundles from
    # home.packages into ~/Applications/Home Manager Apps/. No custom
    # activation script is needed.
  };
}
