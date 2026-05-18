# Freminal Shell Integration

These scripts enable **command-aware rendering** in Freminal by emitting
[OSC 133 (FinalTerm/FTCS)](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/preexec.md)
prompt-and-command lifecycle markers and [OSC 7](https://wezfurlong.org/wezterm/shell-integration.html#osc-7-escape-sequence-to-set-the-working-directory)
working-directory notifications. When the integration is active, Freminal can
identify where each command started, where its output begins, and whether it
succeeded — enabling command-block navigation, exit-status gutters, command
duration overlays, and copy-output actions.

The FTCS marker specification used by Freminal is documented in
`freminal-common/src/buffer_states/ftcs.rs` in the repository.

---

## Sourcing Instructions

### bash — add to `~/.bashrc`

```bash
if [ -f ~/.config/freminal/shell-integration/freminal.bash ]; then
    . ~/.config/freminal/shell-integration/freminal.bash
fi
```

### zsh — add to `~/.zshrc`

```zsh
if [ -f ~/.config/freminal/shell-integration/freminal.zsh ]; then
    . ~/.config/freminal/shell-integration/freminal.zsh
fi
```

### fish — add to `~/.config/fish/config.fish`

```fish
if test -f ~/.config/freminal/shell-integration/freminal.fish
    source ~/.config/freminal/shell-integration/freminal.fish
end
```

Each script is a **no-op outside Freminal** — it checks `$TERM_PROGRAM` and
exits immediately if it is not set to `"freminal"`. You can safely add these
lines unconditionally to your rc files; they will not affect other terminals
or SSH sessions.

---

## Auto-Install Note

These scripts are auto-installed to `~/.config/freminal/shell-integration/`
on first launch (subtask 72.8 wires this; not active in 72.7). Until 72.8
ships, copy them manually or symlink from the repository.

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

When one of these is active alongside Freminal's `freminal.{bash,zsh,fish}`,
OSC 7 will be emitted twice per prompt. **This is harmless** — OSC 7 is
idempotent: emitting `file://host/path` twice in a row simply re-sets the
cwd to the same value. The redundant string parse is microseconds and not
user-visible.

If you prefer to suppress Freminal's OSC 7 emission entirely (because your
existing setup already covers it), remove the `OSC 7` `printf` line from
the relevant script after auto-install — `~/.config/freminal/shell-integration/freminal.{bash,zsh,fish}`.

### OSC 133 has no such conflict

`OSC 133` (FinalTerm/FTCS) prompt markers are only emitted by terminal-
aware shell integrations: iTerm2's `iterm2_shell_integration.*`, WezTerm's
`wezterm.sh`, Kitty's `kitty-integration`, and ours. Each of these is
gated on its respective `$TERM_PROGRAM` value (`iTerm.app`, `WezTerm`,
`xterm-kitty`, `freminal`) and is dormant outside its host terminal.
Sourcing multiple sets unconditionally is safe.

---

## Verifying the Integration

After sourcing the appropriate script, run a command and check that Freminal
is receiving the markers:

1. Source the script in a new shell session inside Freminal.
2. Run any command, e.g. `echo hello`.
3. Freminal's PTY thread should receive the sequence:
   - `OSC 133 ; A ST` — prompt start
   - `OSC 133 ; B ST` — prompt end (user input begins)
   - `OSC 133 ; C ST` — command about to execute
   - `OSC 133 ; D ; 0 ST` — command finished (exit code 0)
   - `OSC 7 ; file://hostname/path ST` — cwd update

Command-block rendering in the scrollback (gutters, navigation, duration
overlays) is being implemented in subtasks 72.10 and 73. For now you can
confirm the bytes are flowing by running Freminal with
`--recording-path /tmp/test.frec`, running a few commands, then decoding
the recording:

```sh
python3 sequence_decoder.py --recording-path=/tmp/test.frec --convert-escape
```

Look for `OSC 133` sequences in the output.

---

## License

Same MIT license as Freminal.
