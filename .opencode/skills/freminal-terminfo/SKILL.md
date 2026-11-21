---
name: freminal-terminfo
description: Use ONLY when working in the freminal repository AND touching anything that decides what a program running in freminal is told about the terminal -- editing res/freminal.ti, regenerating res/terminfo/ or res/terminfo.tar, adding or changing an arm of lookup_termcap (XTGETTCAP) in terminal_handler/dcs.rs, implementing or removing an escape sequence or key encoding that a terminfo capability describes, changing the default TERM, [shell] term, TermName::resolve, the TERMINFO / TERMINFO_DIRS child environment, or any packaging that installs the terminfo entry (rpm assets, the .deb repack script, the flake terminfo package, the NixOS module, the macOS .app copy in the release workflows). Codifies the "the entry declares exactly what freminal implements" rule, the table-of-truth test and its exception lists, and the cargo xtask terminfo regeneration step that building does not do.
---

# Freminal: the terminfo entry is a claim the code must back

Since Task 151, children run with `TERM=xterm-freminal` (Windows: `xterm-256color`), and the
terminfo entry freminal ships is what ncurses, `tput`, tmux, vim and readline read. A
capability that is declared but not implemented is worse than a missing one: programs trust
it and emit sequences freminal then mangles. The durable rationale is
`Documents/DESIGN_DECISIONS.md`, "TERM Strategy (Task 151)". This skill is the procedure.

## The moving parts

| Piece                                                        | Role                                                                |
| ------------------------------------------------------------ | ------------------------------------------------------------------- |
| `res/freminal.ti`                                            | Source of truth. Public path (README links its raw URL): never move |
| `res/terminfo/{x,f,78,66}/…`                                 | Committed compiled tree, letter and hex layouts, identical bytes    |
| `res/terminfo.tar`                                           | Packed from the tree; embedded in the binary, extracted per pane    |
| `cargo xtask terminfo` (alias `ti`, `xtask/src/terminfo.rs`) | The only thing that writes the tree and the tar (`tic -x`)          |
| `lookup_termcap` in `terminal_handler/dcs.rs`                | XTGETTCAP answers; must agree with the entry                        |
| `terminal_handler/dcs/terminfo_truth_tests.rs`               | The table of truth: entry vs XTGETTCAP vs key encoder               |
| `freminal-terminal-emulator/tests/terminfo_tree.rs`          | Tree/tar consistency; `.ti` compiles to the committed entry         |
| `freminal-terminal-emulator/tests/terminfo_child.rs`         | A real PTY child resolves the entry through the production env      |
| `freminal_common::terminfo_source` (`test-utils` feature)    | Parser for `.ti`, used by the GUI-side `kent` test                  |
| `TermName::resolve` (`freminal-common/src/term_name.rs`)     | Pane env `TERM` > `[shell] term` > platform default; one value      |

## Checklist: changing a capability

1. **Implement first, declare second.** Never add a capability to `res/freminal.ti` for
   behaviour freminal does not have. Check the parser, handler and (for `k*`) the input
   encoder, and cite the reference entry the value came from (`infocmp -x xterm-256color`,
   `xterm-kitty`, `foot`, `xterm-ghostty`) in the `.ti` header comment where the existing
   comments do.
2. **Edit `res/freminal.ti`.** Keep it standalone (no `use=`). Extended capabilities need
   `tic -x`, which the xtask already passes.
3. **Mirror XTGETTCAP.** If `lookup_termcap` should answer the name, add a one-line arm with
   the **byte-identical** string (the table-of-truth scan understands only one-line
   `"a" | "b" => value,` arms and panics on any other shape -- do not "tidy" it into a
   multi-line arm). Termcap aliases go in the same arm and in `TERMCAP_ALIASES`.
4. **Classify anything one-sided** in `terminfo_truth_tests.rs`, each with a reason string:
   - `XTGETTCAP_ONLY` -- answered by XTGETTCAP but not a capability of the entry (`TN`,
     `query-os-name`, ...);
   - `TERMINFO_ONLY` -- declared in the entry, deliberately not answered over XTGETTCAP;
   - `NOT_IN_ENTRY` -- deliberately absent (unimplemented or unverified); keep the `.ti`
     header's "Deliberately absent" paragraph in step;
   - `DECLARED_NOT_SENT` -- a `k*` capability the encoder cannot send yet;
   - `CHECKED_IN_GUI` -- a `k*` capability the GUI-side encoder sends, with the name of the
     GUI test that compares it against the parsed entry.

   Every other `k*` value is compared with `TerminalInput::to_payload` output. When an
   encoder change makes a `DECLARED_NOT_SENT` key sendable (e.g. `kcbt` once Task 137 sends
   `CSI Z`), move it to the checked set in the same change.

5. **Regenerate:** `cargo xtask terminfo`. Building does **not** regenerate the tree or the
   tar (a build script may write only to `OUT_DIR`). Commit `res/freminal.ti`,
   `res/terminfo/` and `res/terminfo.tar` together. The stdin-compile test fails with a
   message naming the xtask if you forget.
6. **Escape-sequence docs.** A capability change is usually an escape-sequence change too;
   `freminal-escape-sequence-docs` applies (COVERAGE and GAPS, including the XTGETTCAP rows).
7. **Verify:** `cargo xtask test` (runs the truth, tree and real-child tests; on CI a missing
   `tic`/`infocmp` fails instead of skipping) and `infocmp -x -A res/terminfo xterm-freminal`
   to eyeball the compiled result.

## Checklist: changing TERM resolution or the child environment

- There is **one** effective `TERM` per pane, computed once at spawn. The child's
  environment, XTGETTCAP `TN` and the FREC `term` header all use it; never compute a second
  value anywhere else.
- The child gets `TERMINFO=<pane dir>` (the `.app` bundle's `Contents/Resources/terminfo` on
  macOS when present, else the per-pane extraction) and `FREMINAL_TERMINFO=<pane dir>`. An
  inherited user `TERMINFO` is kept reachable at the front of `TERMINFO_DIRS`, except when it
  is a parent freminal pane's directory. Read `DESIGN_DECISIONS.md` before changing any of
  this: the ncurses search order (`TERMINFO`, `~/.terminfo`, `TERMINFO_DIRS`, default) is
  why `TERMINFO` and not `TERMINFO_DIRS` names the pane's directory.
- A new spawn path must go through the same environment builder; `terminfo_child.rs` is the
  proof, so extend it rather than adding an untested path.

## Checklist: packaging

The entry must reach every package that installs freminal. When adding a package format or
changing an existing one, update all of: `freminal/Cargo.toml` rpm `assets` (letter layout
under `/usr/share/terminfo`), `assets/ci/fix-linux-icon-metadata.sh` (the `.deb` repack; the
AppImage deliberately ships nothing extra), `flake.nix` (`packages.<system>.terminfo` and the
main package's `$out/share/terminfo`, plus the Darwin `.app` copy), `nix/nixos-module.nix`
(`programs.freminal.terminfo`), and the macOS staging step in `.github/workflows/deploy.yaml`
and `nightly.yml` (copy into the `.app` before `codesign --deep`). Packaging copies the
committed tree; no package build may need `tic`. Releases also attach `res/freminal.ti` as
`freminal.ti`.

## When to stop and ask

- You want to declare a capability whose behaviour you cannot verify in freminal's code or
  whose value you would have to guess. Leave it out (add it to `NOT_IN_ENTRY` with
  "unverified") and ask.
- A change would make children get a `TERM` other than the effective value, or would change
  the platform defaults. That reverses a recorded decision.
- A table-of-truth failure tempts you to add the name to an exception list rather than make
  the two sides agree. The exception lists are for deliberate asymmetries, each with a real
  reason -- not for silencing drift.
- `res/freminal.ti` would move or be renamed. Its raw URL is published.
