<!-- freminal-shell-integration v4 -->

# Freminal Shell Integration

These scripts enable **command-aware rendering** in Freminal by emitting
[OSC 133 (FinalTerm/FTCS)](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/preexec.md)
prompt-and-command lifecycle markers and [OSC 7](https://wezfurlong.org/wezterm/shell-integration.html#osc-7-escape-sequence-to-set-the-working-directory)
working-directory notifications. Each marker carries a `freminal=1;fid=<id>`
payload that lets Freminal distinguish its own integration from foreign
OSC 133 emitters (system zsh, Starship, oh-my-zsh, GNOME VTE, etc.).

In addition, the scripts emit **OSC 1338 `HISTFILE=<path>`** once per
session, after the user's rc files have run. This tells Freminal which
shell-history file to seed the Quick Command History Palette
(`Ctrl+Shift+M`) from when the parent-environment `$HISTFILE` is unset
or stale (e.g. zsh users who set `HISTFILE` as a shell variable inside
`.zshrc` rather than exporting it). Empty `$HISTFILE` is suppressed —
Freminal falls back to the per-shell default in that case.

The FTCS marker specification used by Freminal is documented in
`freminal-common/src/buffer_states/ftcs.rs` in the repository.

---

## Architecture: Spawn-Time Injection

Freminal injects shell integration **automatically** when it spawns a child
shell. You do **not** source these scripts from your own rc files — Freminal
arranges for them to be loaded by manipulating shell-specific startup
mechanisms:

| Shell | Mechanism                                                                                            |
| ----- | ---------------------------------------------------------------------------------------------------- |
| bash  | Launched with `bash --posix` + `ENV=<bash/freminal-init.bash>`                                       |
| zsh   | Launched with `ZDOTDIR=<zsh/>`; original ZDOTDIR preserved via sentinel env var                      |
| fish  | Resources directory prepended to `$XDG_DATA_DIRS`; fish autoloads `fish/vendor_conf.d/freminal.fish` |

After our integration runs, control returns to your normal shell startup
(`~/.bashrc`, `~/.zshenv` + `~/.zshrc`, fish's vendor-confd chain), so your
existing prompt theme, aliases, and functions work as usual.

---

## Opting Out

To disable injection entirely, set the following in
`~/.config/freminal/config.toml`:

```toml
[shell_integration]
set_term_program = false
```

This single flag controls both `TERM_PROGRAM` announcement and
shell-integration injection — they are coupled because they are part of the
same feature surface.

---

## User-Edit Warning

These files are **overwritten on every freminal launch** — Freminal compares
the on-disk bytes to the embedded copies and rewrites when they differ. Do
not edit them; your changes will not survive a freminal launch.

If you need to customise behaviour, do it in your own rc files
(`~/.bashrc`, `~/.zshrc`, `~/.config/fish/config.fish`). Those run **after**
our integration installs its hooks, so you can compose with us freely.

---

## Detecting Freminal in Downstream Scripts

```sh
if [ "${TERM_PROGRAM:-}" = "freminal" ]; then
    # Freminal-specific behaviour here
fi
```

In fish:

```fish
if test "$TERM_PROGRAM" = "freminal"
    # Freminal-specific behaviour here
end
```

---

## Desktop Notifications (OSC 9 / OSC 777)

Freminal raises a desktop notification (and/or an in-app toast) when a
program emits an [OSC 9](https://iterm2.com/documentation-escape-codes.html)
(iTerm2/WezTerm) or [OSC 777](https://github.com/tmux/tmux/wiki/Modifier-Keys)
(urxvt `notify;TITLE;BODY`) sequence, and on command completion when
`[notifications] on_command_finished` is enabled (see the `[notifications]`
section of `config_example.toml`).

These sequences are **one-way and fire-and-forget** — there is no
capability-query handshake to test for. The portable way to decide whether
to emit them is the `TERM_PROGRAM` environment variable, which Freminal sets
to `freminal`:

```sh
if [ "${TERM_PROGRAM:-}" = "freminal" ]; then
    notify_via_osc9() { printf '\e]9;%s\a' "$1"; }
    notify_via_osc777() { printf '\e]777;notify;%s;%s\a' "$1" "$2"; }
fi
```

OSC 9 takes a single body string; OSC 777 takes a title and a body. Both are
ignored unless `[notifications] enabled = true`.

---

## Compatibility Notes

### OSC 7 double-emission is harmless

Several environments emit `OSC 7` (working-directory updates) independently
of these scripts:

- **macOS system zsh** (`/etc/zshrc`) sets up `chpwd_functions` that emit
  OSC 7 unconditionally.
- **Starship, Powerlevel10k, oh-my-zsh, prezto** and similar prompt
  frameworks frequently include OSC 7 cwd tracking that fires in any
  terminal.
- **GNOME VTE's `/etc/profile.d/vte.sh`** emits OSC 7 when `$VTE_VERSION`
  is set (Freminal does not set it, so this path is dormant).

When one of these is active alongside Freminal's integration, OSC 7 will be
emitted twice per prompt. **This is harmless** — OSC 7 is idempotent.

### OSC 133 from foreign integrations is filtered

Freminal's OSC 133 parser distinguishes our own markers from foreign ones by
checking for `freminal=1` in the payload. Foreign OSC 133 markers (without
the freminal payload) are parsed but not used to build command blocks, so
having system or theme-level OSC 133 sources active alongside Freminal is
safe.

---

## Verifying the Integration

After launching Freminal, run a command and use the recording feature to
verify markers are flowing:

```sh
freminal --recording-path /tmp/test.frec
# run a few commands inside, then quit
python3 sequence_decoder.py --recording-path=/tmp/test.frec --convert-escape
```

Look for `OSC 133 ; A`, `B`, `C`, `D` sequences each carrying
`freminal=1;fid=…` payloads.

---

## License

Same MIT license as Freminal.
