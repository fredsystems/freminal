---
name: freminal-shell-integration
description: Use ONLY when working in the freminal repository AND editing anything under shell-integration/ (bash/freminal-init.bash, zsh/.zshenv, zsh/freminal-integration, fish/vendor_conf.d/freminal.fish, README.md), freminal/src/shell_integration.rs or its tests, the spawn-time injection in freminal-terminal-emulator/src/io/pty.rs (ENV, ZDOTDIR, XDG_DATA_DIRS, FREMINAL_SHELL_FEATURES, TERM_PROGRAM), the [shell_integration] config section, or the parsing of freminal's OSC 133 / OSC 1338 / OSC 7 emissions. Codifies the version-marker bump, the freminal=1;fid= gate, cross-shell parity and tests, the clean-room rule for third-party shell scripts, the sudo wrapper, and the fail-graceful spawn semantics.
---

# Freminal: shell integration

freminal injects its own bash, zsh and fish scripts at spawn time (Ghostty-style environment
injection, no on-system install). The architecture and its rejected alternatives are in
`Documents/DESIGN_DECISIONS.md`, "Shell Integration Architecture" and "Command Blocks, Gutters
and Fold". This skill is the procedure for changing it.

## Checklist for a script change

1. **Change all three shells**, or say in the change why one is exempt. bash, zsh and fish must
   emit the same markers with the same semantics.
2. **Bump the version marker.** Every shipped script starts with
   `# freminal-shell-integration v<N>`, and `FREMINAL_SHELL_INTEGRATION_VERSION` in
   `freminal/src/shell_integration.rs` must equal it. The test only checks they agree, not that
   you bumped them, so an edit without a bump passes CI. Bump both for every behavioural edit.
3. **Keep the emission protocol.** Command-block markers are OSC 133 A/B/C/D carrying
   `freminal=1` and a per-command `fid=`; freminal ignores untagged OSC 133 by design (other
   FTCS emitters stay harmless). D is matched to its A by `fid` only, with no "most recent open
   block" fallback. `HISTFILE` is reported with OSC 1338 after the rc files ran. Changing any of
   this is a parser change too: update the handler, its tests and the escape-sequence docs
   (`freminal-escape-sequence-docs`).
4. **Respect the shells' gotchas** already documented in the scripts' comments (bash PS1
   escaping, `$(...)` capturing the fid counter, bash-preexec under `--posix`, zsh
   `KSH_ARRAYS` via `emulate -L zsh`, fish `case` globbing). Read the surrounding comments
   before editing.
5. **Test every shell.** Script behaviour tests run the real shells against fakes (see
   `freminal/src/shell_integration/sudo_wrapper_tests.rs` for the pattern); missing `zsh` or
   `fish` skips visibly. Generated fixtures name interpreters found on `PATH`, because the Nix
   build sandbox has no `/usr/bin/env`.
6. **Update `shell-integration/README.md`** when user-visible behaviour changes.

## Clean-room rule

Ghostty's bash and zsh integration files and kitty's shell integration are GPLv3; freminal is
MIT. Never copy from them, and do not read them while writing the replacement. Write from a
behaviour specification (as Task 151.C5 did for the sudo wrapper). Ghostty's fish file is MIT
and may be used with attribution in `ATTRIBUTIONS.md`. A provenance question about an existing
script is a READ-ONLY audit first (Task 147.C4).

## Spawn-time injection

- Injection is gated by `[shell_integration] set_term_program`, which gates both
  `TERM_PROGRAM` and the scripts because they are coupled. Features inside the scripts are
  switched by `FREMINAL_SHELL_FEATURES` (a comma-separated list matched by whole item, e.g.
  `sudo`), set only when injection is on and removed from an inherited environment otherwise.
- Bash injection must keep the shell as `args[0]` (`replace_default_prog([shell, "--posix"])`);
  pushing `--posix` as the first argument makes bash unspawnable.
- **Fail graceful.** An unknown shell, an explicit command, or a failed script sync spawns the
  child normally without injection; it never fails the spawn.
- New config keys follow `freminal-config-options`.

## The sudo wrapper

On by default (`sudo_preserve_terminfo`), defined only when the `sudo` feature is on and
`TERMINFO` is non-empty. It adds `--preserve-env=TERMINFO` unless the invocation is an edit
(`-e` / `--edit` / sudoedit), found by scanning arguments with sudo's own option table (value
options, unique long-option prefixes, standalone `-h` host). Any change needs the cross-shell
case table extended in all three shells. Terminfo context is in `freminal-terminfo`.

## When to stop and ask

- A change would accept untagged OSC 133 or kitty-format shell integration (Task 145; a
  recorded decision would be reversed).
- A fix seems to need code from a GPL script.
- One shell cannot express the behaviour the others have.
