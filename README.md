# Freminal

> A modern terminal emulator written in Rust -- built for precision, performance, and beauty.

[![CI](https://github.com/fredsystems/freminal/actions/workflows/ci.yml/badge.svg)](https://github.com/fredsystems/freminal/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-MIT-blue)

---

## Features

- **Comprehensive ANSI/DEC/xterm escape sequence support** --
  Full SGR (256-color + TrueColor), CSI, OSC, and DEC private mode coverage.

- **Custom OpenGL rendering pipeline** --
  Glyph atlas with shelf-based packing, text shaping via rustybuzz, glyph rasterization
  via swash (including color emoji), and two-pass GPU rendering (background fills +
  foreground glyph quads) through glow. egui handles only the chrome (menu bar, settings
  modal).

- **Font ligatures** --
  OpenType `liga` and `calt` features via rustybuzz. Works with programming fonts like
  JetBrains Mono, Fira Code, and Cascadia Code. Toggleable in config or the settings modal.

- **25 built-in color themes** --
  Catppuccin (Mocha, Macchiato, Frappe, Latte), Dracula, Nord, Solarized, Gruvbox, One
  Dark/Light, Tokyo Night, Kanagawa, Rose Pine, Monokai Pro, Ayu, Everforest, Material
  Dark, and XTerm Default. Selectable live via the settings modal with color-swatch preview.
  Selection persists to `config.toml`.

- **Inline image display** --
  iTerm2 inline images (single-shot and multi-part chunked), Kitty graphics protocol
  (RGB/RGBA/PNG, chunked transmission, query/response, delete), and Sixel.

- **Mouse and input handling** --
  Mouse tracking modes (1000-1006), bracketed paste, full keyboard interaction, and
  focus reporting.

- **Primary screen scrollback** --
  Configurable scrollback buffer (up to 100,000 lines). Scroll offset is pure GUI-side
  view state with no lock contention.

- **Session recording and playback** --
  Record raw PTY output with `--recording-path`. Replay with `--with-playback-file` in
  three modes: instant, real-time (with play/pause), or frame-stepping.

- **TOML configuration system** --
  Layered config from system, user, env var, `--config` override, and CLI flags.
  Covers font, cursor, theme, shell, logging, and scrollback settings.

- **Settings modal with live preview** --
  Font family/size, cursor shape/blink, theme picker, ligature toggle, and scrollback
  limit -- all applied immediately.

- **Lock-free architecture** --
  The PTY thread owns the terminal emulator exclusively and publishes snapshots via
  ArcSwap. The GUI thread reads snapshots atomically with no locks or contention.

- **Reproducible Nix development environment** --
  Flake with devshell, overlay, and home-manager module.

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
              font.size = 14.0;
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

Download the `.deb` package from the
[latest release](https://github.com/fredsystems/freminal/releases):

```bash
sudo dpkg -i freminal-<version>-linux-amd64.deb
```

ARM64 `.deb` packages are also available.

### Windows

Download `freminal-<version>-windows-amd64.exe` from the
[latest release](https://github.com/fredsystems/freminal/releases).

### macOS

Download `freminal-<version>-macos-arm64.app.zip` from the
[latest release](https://github.com/fredsystems/freminal/releases).

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

All fields are optional. Available sections: `[font]`, `[cursor]`, `[theme]`, `[shell]`,
`[logging]`, `[scrollback]`.

---

## CLI Options

```text
freminal [OPTIONS] [COMMAND]...
```

| Flag / Argument               | Description                                                            |
| ----------------------------- | ---------------------------------------------------------------------- |
| `[COMMAND]...`                | Program to run instead of the default shell (exits when program exits) |
| `--shell <PATH>`              | Shell to run (overrides config file and default shell)                 |
| `--config <PATH>`             | Path to a TOML config file (overrides default config search)           |
| `--recording-path <PATH>`     | Path to write session recordings to                                    |
| `--with-playback-file <PATH>` | Replay a recorded session file instead of launching a PTY              |
| `--write-logs-to-file[=BOOL]` | Write logs to a file in the current directory (default: false)         |
| `--show-all-debug`            | Show all debug output (disables default log filtering)                 |
| `-h, --help`                  | Print help                                                             |
| `-V, --version`               | Print version                                                          |

Examples:

```bash
freminal                              # launch default shell
freminal yazi                         # launch yazi; exit when it exits
freminal -- nvim -u NONE file.txt     # launch nvim with arguments
freminal --shell /bin/zsh             # override shell
```

---

## Development

### Nix dev shell (recommended)

```bash
git clone https://github.com/fredsystems/freminal.git
cd freminal
direnv allow     # or: nix develop
```

This provides the Rust toolchain, cargo-llvm-cov, cargo-machete, benchmarking tools, and
all required system libraries.

### Build and run

```bash
cargo run --release
```

### Verification suite

```bash
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo-machete
```

### Benchmarks

```bash
cargo bench --all
```

---

## Architecture

```text
PTY Processing Thread (owns TerminalEmulator exclusively)
  |-- Receives PtyRead from OS PTY reader thread
  |-- Receives InputEvent from GUI (keyboard, resize, focus)
  |-- After each batch: publishes Arc<TerminalSnapshot> via ArcSwap
  '-- Sends WindowCommand to GUI for viewport / report handling

GUI Thread (eframe update() -- pure render, no terminal mutation)
  |-- Loads TerminalSnapshot from ArcSwap (atomic, lock-free)
  |-- Sends InputEvent through crossbeam channel
  |-- Renders terminal text via custom OpenGL pipeline (glyph atlas + shaders)
  '-- Owns ViewState (scroll offset, mouse, focus -- never shared)
```

---

## Contributing

Contributions, feedback, and bug reports are welcome.

If you use Nix, your environment is already set up:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

---

## License

Licensed under the [MIT License](LICENSE).

---

2024-2026 Fred Clausen -- MIT License.
