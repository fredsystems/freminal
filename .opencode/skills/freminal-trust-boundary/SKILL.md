---
name: freminal-trust-boundary
description: Use ONLY when working in the freminal repository AND changing code that acts on bytes a program running in the terminal (or a remote host via ssh) can produce -- escape-sequence parsing and handlers, anything that reads or deletes a local file, opens a URL, writes the clipboard, moves or resizes a window, runs a command, allocates from a sender-supplied size or count, or loops on a sender-supplied count -- or when fixing a security advisory, a fuzz finding, or adding a [security] config option. Codifies "terminal output is untrusted input": named caps with boundary tests, off-by-default policies for dangerous actions, uniform failure replies that leak nothing, path and URL rules, payload-free logs, and the reproducer-with-the-fix rule for fuzz findings.
---

# Freminal: terminal output is untrusted input

`cat`-ing a file or reading remote `ssh` output is enough to feed freminal arbitrary bytes.
Task 148 fixed thirteen published advisories that all came from forgetting that. The rules below
are what those fixes have in common. Disclosure policy is in `.github/SECURITY.md`; fuzzing
procedure is in `fuzz/README.md`.

## Rules

- **Every size, count and length from the stream is capped** by a named constant before it is
  used: allocations, repeat counts (REP), loop counts (CHT), item counts, chunk and total
  payload sizes, title lengths, stack depths, queued replies. Never `Vec::with_capacity` from a
  sender-supplied number. Arithmetic on stream values is checked or saturating
  (`freminal-numeric-conversions`). Follow xterm's or kitty's limit where one exists and cite
  it; otherwise state the reason for the number.
- **A loop driven by a stream count must make progress** each iteration, or stop.
- **Dangerous actions are policies under `[security]`, off or narrowest by default:**
  `allow_window_ops` (off), `allow_kitty_file_transfer` (off), `allow_clipboard_write`
  (`active_pane`), `allow_clipboard_read`. A new action of this kind gets a policy, wired per
  `freminal-config-options`, checked before any side effect. Report-only queries stay
  available.
- **Failures leak nothing.** Every failure in a file-backed path (missing, unreadable, wrong
  type, too large, bad format) gives the same reply as a failed open, so a remote party cannot
  probe the filesystem. The detailed reason goes to a `debug!` log.
- **Local files:** canonicalise, refuse forbidden prefixes, then open non-blocking
  (`O_NONBLOCK`, `O_NOFOLLOW` where the path came from the stream), require a regular file on
  the opened handle, read through a cap. Deleting a file needs a positive rule (known temp
  directory and marker), checked against the identity of the file that was read. On Windows,
  classify the path with `freminal_common::windows_path` before any filesystem call; UNC and
  device paths are refused (they leak NTLM credentials).
- **URLs** open only through `freminal/src/gui/link_open.rs` (scheme allow-list, local `file:`
  rules, launcher refusal). Do not call the OS opener anywhere else.
- **No payload in warn logs** (see `freminal-kitty-protocol-work`); bounded debug logs only.
- **Replies are bounded** (`freminal-reply-path`); a flood of queries must not grow memory or
  starve keystrokes.

## Fixing a finding

1. Reproduce it first: a unit test in the owning crate that fails on the old code.
2. Minimise the fuzz input and commit it under `fuzz/regressions/<target>/` **in the same commit
   as the fix**. `fuzz-replay` is a required CI gate, so a reproducer without its fix turns
   `main` red.
3. Fix the producer, not only the consumer; add defence in depth at the consumer when cheap.
4. Run `nix develop .#fuzz --command cargo xtask fuzz-replay`, and rerun the affected target for
   a few minutes (`cargo xtask fuzz <target> --time 300`). A new finding becomes a numbered
   cleanup entry, not a silent extra fix.
5. Update the escape-sequence docs for any behaviour change, and the advisory or issue.
   Editing a published advisory is the maintainer's action, never an agent's.

## When to stop and ask

- The right cap or default is a product decision (what a legitimate user needs).
- A fix would weaken an existing limit or policy to make a test pass.
- A fix needs a consent prompt (`freminal-kitty-protocol-work`, Task 133) that does not exist.
