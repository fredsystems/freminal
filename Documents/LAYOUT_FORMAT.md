# Freminal Layout File Format

Reference for Freminal saved layouts — TOML files that define complete
multi-window, multi-tab, multi-pane workspace configurations. Layouts are
hand-editable and designed to be checked into version control alongside
projects.

Layouts live in `~/.config/freminal/layouts/` and are discovered automatically.
The menu bar lists them under "Layouts"; the Settings Modal's "Layouts" tab
shows name, description, and a topology preview.

---

## Example (Multi-Window)

```toml
# ~/.config/freminal/layouts/dev.toml

[layout]
name = "Development"
description = "Standard dev workspace: editor, server, logs"

# Variables — can be overridden from CLI: freminal --layout dev.toml ~/projects/myapp
# $1, $2, etc. are positional args. Named vars use ${VAR_NAME}.
[layout.variables]
project_dir = "~/projects/default"  # default value, overridden by $1 if provided

# --- Window definitions ---
# Each [[windows]] entry defines a separate OS window.
# A layout with no [[windows]] section uses a single default window.

[[windows]]
size = [1200, 800]          # width, height in pixels (optional)
position = [100, 200]       # x, y in pixels (optional, ignored on Wayland)
monitor = 0                 # preferred monitor index (optional, best-effort)

  # --- Tab definitions within this window ---

  [[windows.tabs]]
  title = "Editor"
  active = true  # this tab is focused on launch

    # Pane tree for this tab. The tree is defined as a flat list of nodes.
    # Each node has an "id" (local to this tab) and optionally a "parent" + "position".
    # The root node has no parent.

    [[windows.tabs.panes]]
    id = "root"
    split = "vertical"   # "vertical" (left/right) or "horizontal" (top/bottom)
    ratio = 0.65

    [[windows.tabs.panes]]
    id = "editor"
    parent = "root"
    position = "first"   # "first" (left/top) or "second" (right/bottom)
    directory = "${project_dir}"
    command = "nvim ."
    active = true         # this pane has focus within the tab

    [[windows.tabs.panes]]
    id = "sidebar"
    parent = "root"
    position = "second"
    split = "horizontal"
    ratio = 0.5

    [[windows.tabs.panes]]
    id = "terminal"
    parent = "sidebar"
    position = "first"
    directory = "${project_dir}"

    [[windows.tabs.panes]]
    id = "git"
    parent = "sidebar"
    position = "second"
    directory = "${project_dir}"
    command = "lazygit"

  [[windows.tabs]]
  title = "Server"

    [[windows.tabs.panes]]
    id = "server"
    directory = "${project_dir}"
    command = "cargo watch -x run"

[[windows]]
size = [800, 600]
position = [1350, 200]

  [[windows.tabs]]
  title = "Logs"

    [[windows.tabs.panes]]
    id = "logs"
    directory = "/var/log"
    command = "tail -f syslog"
```

## Single-Window Shorthand

A layout with no `[[windows]]` section and top-level `[[tabs]]` entries is
treated as a single-window layout. Simple layouts stay simple:

```toml
[layout]
name = "Simple"

[[tabs]]
title = "Main"

  [[tabs.panes]]
  id = "main"
  directory = "~/projects"
```

---

## Window Properties

| Field      | Type       | Required | Description                                       |
| ---------- | ---------- | -------- | ------------------------------------------------- |
| `size`     | [u32, u32] | No       | Width, height in pixels (default: system default) |
| `position` | [i32, i32] | No       | X, Y in pixels (ignored on Wayland, see below)    |
| `monitor`  | u32        | No       | Preferred monitor index, 0-based (best-effort)    |

### Platform Behavior for `position`

- **X11:** Full support via `winit::Window::set_outer_position()`.
- **macOS:** Full support (same API, Y is from top-left).
- **Windows:** Full support (same API).
- **Wayland:** `set_outer_position()` is a no-op — the compositor manages
  window placement. Position is still _saved_ (via `outer_position()`, which
  works on some compositors like KDE/KWin) so that a layout saved on Wayland
  can be restored on X11. When restoring on Wayland, the position field is
  silently ignored. Size restoration works on all platforms.

Window geometry is best-effort: the file format stores it unconditionally
(portable) and restoration degrades silently where the compositor disallows
it. As Wayland compositors add positioning protocols (e.g.
xdg-toplevel-position-v1), Freminal can adopt them without format changes.

---

## Pane Node Properties

The pane tree is a flat list of nodes with parent references. Each
`[[windows.tabs.panes]]` entry is either a **split node** (has `split` and
`ratio`) or a **leaf node** (has no `split`). Leaf nodes represent actual
terminal panes.

| Field       | Type   | Required | Description                                              |
| ----------- | ------ | -------- | -------------------------------------------------------- |
| `id`        | String | Yes      | Unique within the tab (for parent references)            |
| `parent`    | String | No       | ID of the parent split node (absent for root)            |
| `position`  | String | No       | `"first"` or `"second"` within the parent split          |
| `split`     | String | No       | `"vertical"` or `"horizontal"` — makes this a split node |
| `ratio`     | Float  | No       | Split ratio (0.0-1.0), default 0.5                       |
| `directory` | String | No       | Working directory (supports `~` and variables)           |
| `command`   | String | No       | Command to run after shell starts                        |
| `shell`     | String | No       | Override the default shell for this pane                 |
| `env`       | Table  | No       | Extra environment variables: `env = { FOO = "bar" }`     |
| `title`     | String | No       | Initial pane title (before shell OSC overrides)          |
| `active`    | Bool   | No       | If true, this pane/tab has focus on launch               |

A tab with a single pane omits `parent`, `position`, and `split` — just one
pane entry with the leaf properties.

The flat-list-with-parent-refs representation is used because deeply nested
inline TOML tables for trees are ugly and hard to edit. Each node is
self-contained and easy to add, remove, or reorder.

---

## Variable Substitution

Layouts support variable substitution for reusability across projects. One
layout file can target many projects:

```sh
freminal --layout dev.toml ~/projects/frontend
freminal --layout dev.toml ~/projects/backend
```

### Substitution Forms

- **Positional args:** `$1`, `$2`, etc. from the command line (in the example
  above, `~/projects/frontend` is `$1`).
- **Named variables:** `${VAR_NAME}` references `[layout.variables]` defaults,
  overridable via `--var NAME=VALUE`.
- **Environment variables:** `$ENV{HOME}`, `$ENV{USER}` for system env vars.
- **Tilde expansion:** `~` is expanded to `$HOME` in `directory` fields.

Without variables, layouts would be project-specific (hardcoded paths). The
`$1` positional convention follows shell scripting.

---

## Save Current Layout

"Save Layout" captures the running session:

- Window count, positions, and sizes
- Tab count and order per window
- Per-tab pane tree structure (split directions, ratios)
- Per-pane working directory (read from `/proc/<pid>/cwd` on Linux)
- Per-pane foreground process (read from `/proc/<pid>/cmdline` on Linux —
  best-effort, may be empty or show the shell)
- Per-pane title (current OSC title)

The output is a valid layout TOML file. Directories are written as absolute
paths; the user can edit the file to use variables afterward.

Save captures topology + geometry + CWD but **not** running programs, because
we cannot reliably restart an arbitrary program the user was running. Saving
CWDs and window geometry covers 90% of the restore value. If `command` was
specified in the original layout, the saved layout preserves it — but the
_detected_ foreground process from `/proc` is stored as a comment, not as an
auto-run command, to avoid surprising behavior.

**Platform note:** CWD and process detection via `/proc` is Linux-specific. On
other platforms these fields are omitted and the saved layout captures
topology and geometry only.

---

## Layout Application Modes

Layouts can be applied in three ways:

1. **Startup (replace):** `freminal --layout dev.toml` or
   `startup.layout = "dev"` in config. Creates the workspace from scratch on
   launch. This is the primary use case.

2. **On-demand replace:** Menu bar > "Layouts" > "Load Layout..." replaces the
   current session with the selected layout. Prompts for confirmation.

3. **On-demand append:** Menu bar > "Layouts" > "Load Layout in New Tab..."
   creates the layout's tabs as additional tabs in the current session —
   "I want my dev layout alongside my existing work."

---

## Auto-Save and Restore

```toml
[startup]
# Save the current layout on exit and restore it on next launch.
# The layout is saved to ~/.config/freminal/layouts/_last_session.toml
restore_last_session = false
```

When enabled, Freminal saves the current layout to `_last_session.toml` on
clean exit. On next launch (if no `--layout` flag is given), it restores from
this file. Topology, geometry, and CWDs are restored; running programs are
not (they have exited).

---

## Layout Library

Layouts in `~/.config/freminal/layouts/` are discovered automatically. The
menu bar shows them under "Layouts" with their `layout.name` field. The
Settings Modal's "Layouts" tab lists all discovered layouts with name,
description, and a preview of the topology.
