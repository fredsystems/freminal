# Freminal

> A modern, GPU-accelerated terminal emulator built for speed, correctness, and the way people actually work in a terminal.

[![CI](https://github.com/fredsystems/freminal/actions/workflows/ci.yml/badge.svg)](https://github.com/fredsystems/freminal/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-MIT-blue)

---

## Why Freminal?

- **It is fast.** Every character you see is drawn on the GPU. The terminal and the renderer never wait on each other, so scrolling, resizing, and heavy output stay smooth even under load.

- **It gets the details right.** Vim, tmux, htop, fzf, yazi, nvim — the programs that tend to expose terminal bugs just work. Compatibility isn't a marketing claim here; it's enforced by an integration test suite that fails the build if behavior drifts.

- **It has a real multiplexer built in.** Split panes, navigate with the keyboard, resize, zoom, tabs, and multiple windows. You don't need tmux to work the way tmux users work.

- **It remembers your workspace.** Define a project's entire layout — which panes, which commands, which directories, which environment variables — in a single file. Load it and your whole session appears ready to go. Save your current session out to a layout file with a keystroke.

- **It looks good out of the box.** Twenty-seven hand-tuned themes — Catppuccin, Dracula, Nord, Solarized, Gruvbox, Tokyo Night, Kanagawa, Rose Pine, and more — previewed live as you pick them. Ligature support for programming fonts. Color emoji. Adjustable window transparency.

- **It records everything.** Start a session with one flag and every keystroke, every byte of output, every pane, every window is captured to a single file for replay, debugging, or post-mortems. Great for bug reports, teaching, and figuring out what on earth that script did last Tuesday.

- **It respects its own foundation.** The next release is entirely devoted to closing every correctness and polish gap before a single new feature ships. When Freminal says a feature works, it works.

---

## Features at a glance

**Look and feel**
Ligature-aware programming fonts, color emoji, 27 built-in themes with a live-preview picker, adjustable background opacity, cursor styles with blink, and smooth GPU rendering on every platform.

**Terminal compatibility**
Full modern escape sequence support — true-color, mouse tracking, bracketed paste, focus reporting, Kitty keyboard protocol, alternate screens, scroll regions, left/right margins, blinking text, OSC 8 hyperlinks, and even VT52 for the retro crowd.

**Inline images**
iTerm2 and Kitty graphics protocols plus Sixel. Tools like `yazi`, `timg`, and image-aware shells display images directly in the terminal.

**Search**
Live search across the visible buffer and scrollback with next/previous navigation, case sensitivity toggle, and configurable keybindings.

**Tabs, panes, and windows**
Split any direction, navigate with the keyboard, resize, zoom, close, reorder, and spawn additional windows. Every shortcut is rebindable.

**Saved layouts**
Reusable TOML-defined workspaces with per-pane working directory, startup command, shell override, and environment variables. Variable substitution for cross-project templates. Auto-save on exit, auto-restore on launch.

**Session recording**
Capture an entire multi-window, multi-pane session — output, input, and topology changes — into a single time-indexed file. Replay externally with the included decoder.

**Configuration**
One `config.toml` file, layered sensibly: system → user → environment → CLI flag. Every setting is also reachable from an in-app settings window that persists your changes. Platform-appropriate defaults — `Cmd` on macOS, `Ctrl` on Linux and Windows.

**Packaging**
Nix flake (with home-manager module), Debian and Ubuntu packages for amd64 and arm64, a Windows executable, and a macOS app bundle for Apple silicon.

---

## Installation

### Nix flake

```bash
# Run directly
nix run github:fredsystems/freminal

# Or install into your profile
nix profile install github:fredsystems/freminal
```

### Nix home-manager

```nix
{
  inputs.freminal.url = "github:fredsystems/freminal";

  outputs = { nixpkgs, freminal, ... }: {
    homeConfigurations."user" = home-manager.lib.homeManagerConfiguration {
      modules = [
        freminal.homeManagerModules.default
        {
          programs.freminal = {
            enable = true;
            settings = {
              font.family = "JetBrainsMono Nerd Font";
              font.size  = 14.0;
              theme.name = "catppuccin-mocha";
            };
          };
        }
      ];
    };
  };
}
```

### Debian / Ubuntu

Grab the `.deb` from the [latest release](https://github.com/fredsystems/freminal/releases):

```bash
sudo dpkg -i freminal-<version>-linux-amd64.deb
```

ARM64 `.deb` packages are also available.

### Windows

Download `freminal-<version>-windows-amd64.exe` from the [latest release](https://github.com/fredsystems/freminal/releases).

### macOS

Download `freminal-<version>-macos-arm64.app.zip` from the [latest release](https://github.com/fredsystems/freminal/releases).

### From source

```bash
cargo install --git https://github.com/fredsystems/freminal.git
```

---

## Configuration

Copy [`config_example.toml`](./config_example.toml) to your platform's config directory:

| Platform | Path                                                 |
| -------- | ---------------------------------------------------- |
| Linux    | `~/.config/freminal/config.toml`                     |
| macOS    | `~/Library/Application Support/Freminal/config.toml` |
| Windows  | `%APPDATA%\Freminal\config.toml`                     |

All settings are optional and can also be edited in the Settings window from the menu bar. Layouts live in `~/.config/freminal/layouts/` (or your platform's equivalent) — see the example file for annotated samples.

---

## Command line

```text
freminal [OPTIONS] [COMMAND]...
```

| Flag / Argument               | Description                                                            |
| ----------------------------- | ---------------------------------------------------------------------- |
| `[COMMAND]...`                | Program to run instead of the default shell (exits when program exits) |
| `--shell <PATH>`              | Shell to run (overrides config file and default shell)                 |
| `--config <PATH>`             | Path to a TOML config file (overrides default config search)           |
| `--recording-path <PATH>`     | Path to write a session recording                                      |
| `--write-logs-to-file[=BOOL]` | Write logs to a file in the current directory (default: false)         |
| `--show-all-debug`            | Show all debug output                                                  |
| `-h, --help`                  | Print help                                                             |
| `-V, --version`               | Print version                                                          |

Examples:

```bash
freminal                              # launch default shell
freminal yazi                         # launch yazi; exit when it exits
freminal -- nvim -u NONE file.txt     # launch nvim with arguments
freminal --shell /bin/zsh             # override shell
freminal --recording-path ~/rec.frec  # record this session to disk
```

---

## Roadmap

Freminal is under active development with a public, versioned roadmap. Every version below has a written plan; stubs fill in as we get closer.

| Version | Theme                      | Status           | Highlights                                                                                                                                    |
| ------- | -------------------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| v0.8.0  | Correctness & Polish       | Next (hard gate) | No new features. The whole version is devoted to finishing every rough edge before v0.9.0 adds anything new.                                  |
| v0.9.0  | Modern Workflow Terminal   | Planned          | Shell-aware command blocks with exit-status gutters, broadcast-to-panes, project-scoped environments, system notifications, smart paste guard |
| v0.10.0 | Power-User Toolkit         | Planned          | Named profiles, live theme editor, fast scrollback regex search, hint mode, command palette                                                   |
| v0.11.0 | Platform Play              | Planned          | User scripting, a powerline-capable status bar, first-class SSH with session roaming, optional AI command assist                              |
| v0.12.0 | Completeness & Credibility | Planned          | CJK / IME input, accessibility hooks, Windows quality pass, opt-in crash reporting, terminfo auto-install, config import from other terminals |

For the full task list and dependencies, see [`Documents/MASTER_PLAN.md`](./Documents/MASTER_PLAN.md).

**Why pause for v0.8.0?** Because a terminal is the program you trust to not surprise you. Before adding command blocks, notifications, and scripting, the existing foundation gets the full treatment: every rough corner smoothed, every advertised feature verified, every error path made visible. When v0.9.0 starts adding the flashy stuff, it's built on something solid.

---

## Contributing

Contributions, bug reports, and feedback are welcome. The project operates under a strict set of rules described in [`agents.md`](./agents.md). Read that before opening a PR.

The Nix dev shell is the recommended setup — it provides the Rust toolchain and everything you need to build and test:

```bash
git clone https://github.com/fredsystems/freminal.git
cd freminal
direnv allow   # or: nix develop
```

Every PR must pass the full verification suite:

```bash
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo-machete
```

Pre-commit hooks run these automatically. `--no-verify` is forbidden.

---

## License

Licensed under the [MIT License](LICENSE).

---

© 2024–2026 Fred Clausen — MIT License.
