-- wezterm.lua — Task 125.2 hermetic parity fixture (template).
--
-- Rendered by workloads.sh's `render_fixture` into an isolated per-run
-- directory before every capture, and loaded via `wezterm --config-file
-- <rendered-copy> start --always-new-process`. Never edited in place and
-- never the operator's real `~/.wezterm.lua`. Every option below is
-- documented in the installed WezTerm 0-unstable-2026-09-17 (re-pinned 2026-10-04 from 2026-08-12)'s own config
-- reference (wezterm/config/src/config.rs); see Documents/PROFILING.md,
-- "Task 125 parity fixtures" for the verification trail.
--
-- Tokens (`@..@`) are substituted at render time.

local wezterm = require("wezterm")
local config = wezterm.config_builder()

-- Geometry request matching the tiled-slot PTY gate. Hyprland owns the final
-- outer bounds; the runner still verifies the live PTY before every sample.
config.initial_cols = 124
config.initial_rows = 31

-- Font: same family/size/line-height family as the other two terminals.
config.font = wezterm.font("CaskaydiaCove Nerd Font")
config.font_size = 12.0
config.line_height = 1.05

-- Ligatures on (calt/clig/liga), matching Freminal's `font.ligatures = true`
-- and Ghostty's unset (default-enabled) `font-feature`.
config.harfbuzz_features = { "calt=1", "clig=1", "liga=1" }

-- Cursor: block shape, no trail-equivalent animation exists in WezTerm to
-- disable (there is none by default). Blink is the one axis the workload
-- matrix varies; @CURSOR_BLINK_STYLE@ is substituted with "SteadyBlock" or
-- "BlinkingBlock" per rendered copy.
config.default_cursor_style = "@CURSOR_BLINK_STYLE@"

-- Opaque background, no background image, no window shader equivalent.
config.window_background_opacity = 1.0

-- Avoid decoration-dependent size drift; the OS-drawn border/title, not
-- WezTerm's own tab bar, is what this disables.
config.window_decorations = "NONE"

-- Determinism / isolation: no config hot-reload mid-capture, no update
-- checks (network I/O this task must not attribute to rendering cost), no
-- close-confirmation dialogs blocking automated teardown.
config.automatically_reload_config = false
config.check_for_updates = false
config.window_close_confirmation = "NeverPrompt"
config.default_prog = {
  "/run/current-system/sw/bin/bash",
  "--noprofile",
  "--rcfile",
  "@SHELL_RC@",
  "-i",
}

config.keys = {
  { key = "F6", action = wezterm.action.SpawnTab("CurrentPaneDomain") },
  { key = "F7", action = wezterm.action.SplitHorizontal({ domain = "CurrentPaneDomain" }) },
  { key = "F8", action = wezterm.action.SplitVertical({ domain = "CurrentPaneDomain" }) },
  { key = "F9", action = wezterm.action.ActivatePaneDirection("Left") },
}

-- Scrollback large enough for workload 6 (10,000 preloaded lines) without
-- silently truncating it.
config.scrollback_lines = 20000

return config
