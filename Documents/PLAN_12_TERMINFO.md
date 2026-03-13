# PLAN_12 — Terminfo Audit and Strategy

## Overview

Freminal ships a custom terminfo entry (`res/freminal.ti`) but sets `TERM=xterm-256color` at
runtime, meaning the custom entry is never actually used by child processes. This task audits the
current terminfo state, decides on a strategy (keep lying as xterm-256color vs. use a custom
TERM), fixes known bugs in `freminal.ti`, and cleans up the build pipeline.

**Dependencies:** None
**Dependents:** Task 4 (Deployment Flake) — terminfo distribution strategy affects packaging
**Primary crates:** `freminal-common`, `freminal-terminal-emulator`
**Estimated scope:** Medium (audit + targeted fixes)

---

## Current State

### Build Pipeline

1. `freminal-common/buildback.rs` (build.rs) compiles `res/freminal.ti` via `tic -o -x` into a
   tarball.
2. `freminal-common/src/terminfo.rs` embeds the tarball via `include_bytes!("../../res/terminfo.tar")`.
   - **BUG:** The build.rs `rerun-if-changed` detection is broken. The actual binary uses a
     pre-compiled `res/terminfo.tar` that must be manually recompiled if `freminal.ti` changes.
3. `freminal-terminal-emulator/src/io/pty.rs` extracts the tarball to a `TempDir`, sets
   `TERMINFO` pointing to it, and sets `TERM=xterm-256color` (NOT `xterm-freminal`).

### The TERM=xterm-256color Decision

The comment at `pty.rs:68-71` explains the rationale: "nvim, and probably others, lose their
fucking mind and don't send all escapes we support if they don't know the terminal." This is a
real problem — programs like neovim, tmux, and many TUI apps have hardcoded behavior based on
TERM value. Setting TERM to an unknown value causes them to fall back to minimal capabilities.

### freminal.ti vs xterm-256color Differences

**Things freminal.ti adds over xterm-256color:**

- `hs` (has status line), `dsl`/`fsl`/`tsl` — status line manipulation for window title via OSC 2
- `ich1=\E[@]` — insert single character

**Things freminal.ti is missing vs xterm-256color:**

- `is2`, `ka1/ka3/kb2/kbeg/kc1/kc3` (keypad keys), `mc0/mc4/mc5` (printer)
- `meml/memu`, `mgc` (margin clear), `nel`, `rmm/smm` (meta mode), `rs2`
- `smglp/smglr/smgrp` (left/right margins)
- `smcup`/`rmcup` use simpler forms (no `\E[22;0;0t`/`\E[23;0;0t` for title stack save/restore)

**Known bugs in freminal.ti:**

- `sgr` string is missing `%?%p5%t;2%;` (dim attribute) — dim IS supported by Freminal
- `rs1=\E]\E\\\Ec` vs xterm's `rs1=\Ec\E]104\007` — different reset sequences (may or may not
  matter)

### XTGETTCAP Responses

`lookup_termcap` in `terminal_handler.rs` reports `TN=xterm-256color` (not freminal) and
supports: RGB, Tc, setrgbf, setrgbb, colors, Ms, Se, Ss, Smulx, Setulc.

---

## Strategy Analysis

### Option A: Stay with TERM=xterm-256color (Recommended)

**Pros:**

- Maximum compatibility. Programs recognize xterm-256color and enable their full feature sets.
- No terminfo installation headaches (SSH, containers, NixOS, system packages).
- This is what WezTerm, Alacritty, and most modern terminals actually do.
- XTGETTCAP and DA responses can advertise extra capabilities beyond what xterm-256color
  declares (modern programs query these).

**Cons:**

- Programs may assume capabilities Freminal doesn't have (e.g., left/right margins, printer).
  In practice this is rarely an issue — programs that use these check for the specific
  capability string, not just the TERM value.
- Can't declare capabilities that xterm-256color doesn't have via terminfo alone. But XTGETTCAP
  covers this gap for modern programs.

### Option B: Use TERM=xterm-freminal (Custom Entry)

**Pros:**

- Precisely describes Freminal's actual capabilities.
- Could declare new capabilities (like status line support) that xterm-256color doesn't have.

**Cons:**

- Programs that don't recognize the TERM value fall back to minimal mode.
- Requires installing the terminfo entry on every system where Freminal runs, including remote
  SSH hosts, containers, and NixOS. This is a significant distribution burden.
- Kitty tried this approach and had to build an entire SSH kitten to copy terminfo to remote
  hosts. We don't want that complexity.

### Recommendation

**Stay with TERM=xterm-256color.** Fix the bugs in `freminal.ti` for correctness (in case we
ever want to ship it), but don't change TERM. Use XTGETTCAP to advertise extra capabilities.
Clean up the build pipeline so the tarball is correctly rebuilt when `freminal.ti` changes.
The terminfo tarball continues to be extracted at runtime so `TERMINFO` points to a valid
directory (some programs check `TERMINFO` exists even with a standard TERM value).

---

## Implementation Checklist

> **Agent instructions:** Follow the Multi-Step Task Protocol from `agents.md`.

---

- [x] **12.1 — Remove dead `buildback.rs` file**
  - `freminal-common/buildback.rs` was not wired into `Cargo.toml` (no `build = "buildback.rs"`
    entry), so it was completely inert dead code. Removed the file entirely.
  - The pre-compiled `res/terminfo.tar` continues to be embedded via `include_bytes!` in
    `freminal-common/src/terminfo.rs`. If `freminal.ti` changes, the tarball must be manually
    regenerated with `tic`.
  - **Verified:** `cargo test --all` passes. `cargo build --all` succeeds.

---

- [x] **12.2 — Fix bugs in freminal.ti**
  - Added the missing dim attribute (`%?%p5%t;2%;`) to the `sgr` string.
  - Updated `smcup`/`rmcup` to include title-stack save/restore sequences matching
    xterm-256color: `\E[?1049h\E[22;0;0t` / `\E[?1049l\E[23;0;0t`.
  - Fixed `rs1` to match xterm-256color: `\Ec\E]104\007` (was `\E]\E\\\Ec`).
  - Regenerated `res/terminfo.tar` via `tic -x -o`.
  - **Verified:** `infocmp -x xterm-freminal` shows all corrected entries.
    `cargo build --all` succeeds. `cargo test --all` passes.

---

- [ ] **12.3 — Clean up XTGETTCAP responses**
  - Audit `lookup_termcap` in `freminal-buffer/src/terminal_handler.rs` against Freminal's
    actual capabilities.
  - Ensure all supported capabilities are advertised (check for any missing ones).
  - Add tests for `lookup_termcap` responses — each capability should have a test that verifies
    the response format and content.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **12.4 — Document the terminfo strategy**
  - Add a section to `Documents/` (or a comment block in `pty.rs`) explaining the TERM strategy:
    - Why we use xterm-256color.
    - What XTGETTCAP does for us.
    - When/why someone might want the custom terminfo entry.
  - Update this plan document with completion notes.
  - **Verify:** Documentation is clear and accurate.

---

## Affected Files

| File                                       | Change Type                      |
| ------------------------------------------ | -------------------------------- |
| `freminal-common/buildback.rs`             | Fix rerun detection              |
| `res/freminal.ti`                          | Fix bugs (dim, smcup/rmcup, rs1) |
| `res/terminfo.tar`                         | Regenerated                      |
| `freminal-buffer/src/terminal_handler.rs`  | Audit and test XTGETTCAP         |
| `freminal-terminal-emulator/src/io/pty.rs` | Add documentation comment        |

---

## Risk Assessment

| Risk                               | Likelihood | Impact | Mitigation                                                |
| ---------------------------------- | ---------- | ------ | --------------------------------------------------------- |
| Breaking XTGETTCAP format          | Low        | High   | Add tests before changing                                 |
| Build.rs change breaks CI          | Low        | Medium | Test on clean build                                       |
| Terminfo changes cause regressions | Very Low   | Low    | TERM stays xterm-256color; ti changes are safety net only |
