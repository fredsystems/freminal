// Copyright (C) 2026 Fred Clausen
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shell-history seeding for the Quick Command History Palette (Task 72.15).
//!
//! At pane spawn time, freminal reads the user's existing shell history file
//! and pre-populates a per-pane "history seed" so the palette presents a
//! merged view of (a) historical commands from prior sessions and (b) live
//! commands captured via OSC 133 D in the current session.
//!
//! ## Data flow
//!
//! ```text
//! spawn_pty_tab
//!   ├── resolves the shell program (--shell / layout override / $SHELL)
//!   ├── creates an empty Arc<OnceLock<Vec<String>>> ("history seed")
//!   ├── spawns a background loader thread that:
//!   │     1. detects the shell kind from the program path
//!   │     2. resolves the history file path (HISTFILE env or default)
//!   │     3. reads + parses the file
//!   │     4. caps at HISTORY_SEED_CAP and calls slot.set(vec)
//!   └── returns the slot in TabChannels
//!
//! Pane.history_seed = channels.history_seed (clone of Arc)
//!
//! Palette (Task 72.15 commit 2)
//!   ├── reads slot.get() — Option<&Vec<String>>
//!   ├── if Some, presents seed entries (no timestamps/exit codes)
//!   ├── interleaves Pane.recent_commands (live, with timestamps/exit codes)
//!   └── if None (still loading or no history), just shows live commands
//! ```
//!
//! ## Privacy considerations
//!
//! This module reads commands that the user already chose to save to their
//! shell history file.  No new information is exposed -- the palette
//! surfaces what `bash`/`zsh`/`fish` themselves would surface via their
//! own history-search builtins.  `HISTIGNORE` / `HIST_IGNORE_*` rules are
//! transitively respected (we read the saved file; the shell already
//! filtered before writing).
//!
//! ## Known limitations
//!
//! - **Runtime `HISTFILE` overrides set in user `.bashrc` / `.zshrc` /
//!   `config.fish` are not visible to freminal.**  We only see the env we
//!   spawned the shell with.  Documented; not a bug.
//! - **`exec`-ing a different shell mid-pane** does not trigger a re-load
//!   of the new shell's history.  Acceptable; documented.
//! - **Non-shell programs** (e.g. spawning `python` directly) have no
//!   history file to read -- the seed remains empty.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tracing::{debug, trace, warn};

/// Maximum number of history entries loaded into the seed per pane.
///
/// Chosen to comfortably cover typical `$HISTSIZE` / `$SAVEHIST` defaults
/// (bash 500, zsh 1000, fish 256-2048) while keeping the in-memory footprint
/// trivial (~tens of KB per pane).  Power users with huge history files get
/// the most recent 1000 entries.
pub const HISTORY_SEED_CAP: usize = 1000;

/// The shell kind detected from a program path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// GNU Bash. History file: `$HISTFILE` or `~/.bash_history`.
    Bash,
    /// Z shell. History file: `$HISTFILE` or `~/.zsh_history`.
    Zsh,
    /// Friendly Interactive Shell. History file:
    /// `~/.local/share/fish/fish_history` (or `$XDG_DATA_HOME/fish/...`).
    Fish,
    /// Any other program (`sh`, `dash`, `python`, custom binaries, ...).
    /// No history loading is attempted.
    Other,
}

/// Classify a program path by its file name (case-sensitive on Unix).
///
/// Symlink resolution is intentionally NOT performed -- `/usr/bin/sh` is
/// often a symlink to `bash` or `dash`, and users expect `--shell sh` to
/// mean "POSIX sh" not "interactive bash with history loading".  Only the
/// basename of the explicit path is consulted.
#[must_use]
pub fn detect_shell_kind(program: &Path) -> ShellKind {
    let Some(name) = program.file_name().and_then(|n| n.to_str()) else {
        return ShellKind::Other;
    };
    match name {
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        "fish" => ShellKind::Fish,
        _ => ShellKind::Other,
    }
}

/// Resolve the history file path for the given shell kind, consulting an
/// environment lookup closure.
///
/// Falls back to per-shell default paths if the shell's documented `HISTFILE`
/// env var is unset.  Returns `None` for [`ShellKind::Other`] or when
/// `$HOME` is unset and no default can be constructed.
#[must_use]
pub fn resolve_history_path(
    kind: ShellKind,
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    match kind {
        ShellKind::Bash => {
            if let Some(path) = get_env("HISTFILE").filter(|s| !s.is_empty()) {
                return Some(PathBuf::from(path));
            }
            get_env("HOME").map(|h| PathBuf::from(h).join(".bash_history"))
        }
        ShellKind::Zsh => {
            if let Some(path) = get_env("HISTFILE").filter(|s| !s.is_empty()) {
                return Some(PathBuf::from(path));
            }
            get_env("HOME").map(|h| PathBuf::from(h).join(".zsh_history"))
        }
        ShellKind::Fish => {
            // Fish does not honor HISTFILE; it uses XDG_DATA_HOME with a
            // hard-coded subdirectory.  Default session name is "fish".
            let base = get_env("XDG_DATA_HOME")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    get_env("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
                })?;
            Some(base.join("fish").join("fish_history"))
        }
        ShellKind::Other => None,
    }
}

/// Parse a bash history file.
///
/// Each line is one command.  Lines starting with `#` are treated as
/// `HISTTIMEFORMAT` timestamps and skipped.  Empty lines are skipped.
/// Multi-line commands stored via `cmdhist` / `lithist` are NOT reassembled
/// (bash stores them as one logical line with literal `\n` characters or
/// as separate physical lines depending on shopts -- we present them as
/// the user's file presents them, which matches what bash itself would
/// recall).
#[must_use]
pub fn parse_bash_history(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

/// Parse a zsh history file.
///
/// Supports both the plain format (one command per line) and the extended
/// format (`: <timestamp>:<duration>;<command>`) selected by
/// `setopt extended_history`.
///
/// Multi-line commands are reassembled.  zsh stores a multi-line command
/// as several physical lines where every line except the last ends with
/// an unescaped backslash; without reassembly, each continuation line
/// would appear in the palette as a phantom entry the user does not
/// recognise.  Continuations are joined with a single space so the
/// reassembled command displays compactly and is suitable to re-inject
/// at the prompt verbatim (matching the wezterm / iTerm2 Recall
/// convention).
#[must_use]
pub fn parse_zsh_history(content: &str) -> Vec<String> {
    // Phase 1: collapse trailing-backslash continuations into single
    // logical lines.
    let mut logical_lines: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in content.lines() {
        if let Some(stripped) = line.strip_suffix('\\') {
            // Continuation marker.  Drop the trailing backslash and any
            // whitespace immediately before it so the joined form does
            // not carry a stray space at the seam.
            let part = stripped.trim_end();
            match current.as_mut() {
                Some(buf) => {
                    buf.push(' ');
                    buf.push_str(part);
                }
                None => current = Some(part.to_owned()),
            }
        } else {
            let combined = current.take().map_or_else(
                || line.to_owned(),
                |mut buf| {
                    buf.push(' ');
                    buf.push_str(line.trim_start());
                    buf
                },
            );
            logical_lines.push(combined);
        }
    }
    if let Some(buf) = current {
        // File ended mid-continuation; salvage what we have so the user
        // still sees the start of the command.
        logical_lines.push(buf);
    }

    // Phase 2: parse each logical line according to format.
    logical_lines
        .into_iter()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            if line.starts_with(": ") {
                // Extended format: ": <ts>:<dur>;<cmd>" -- everything after
                // the first ";" is the command.
                line.split_once(';').map(|(_, cmd)| cmd.to_owned())
            } else {
                Some(line)
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse a fish history file.
///
/// Fish stores history as a sequence of YAML-like blocks:
///
/// ```text
/// - cmd: ls -la
///   when: 1700000000
///   paths:
///     - /tmp
/// - cmd: cat foo
///   when: 1700000010
/// ```
///
/// We only extract `cmd:` lines.  Embedded escape sequences in the cmd
/// value (`\\n`, `\\\\`) are decoded to their literal characters.  The
/// `paths:` and `when:` fields are ignored.
#[must_use]
pub fn parse_fish_history(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("- cmd: ") {
            out.push(decode_fish_cmd(rest));
        }
    }
    out
}

/// Decode fish's history-file escape sequences in a `cmd:` value.
///
/// Fish escapes `\n`, `\r`, `\t`, `\\` in the on-disk form to keep each
/// block on its own line.  We expand them back.  Unknown escapes are
/// passed through literally.
fn decode_fish_cmd(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars();
    while let Some(c) = iter.next() {
        if c == '\\' {
            match iter.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') | None => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Load the shell history for the given program path, returning at most
/// [`HISTORY_SEED_CAP`] of the most-recent entries.
///
/// Synchronous; intended to be invoked from [`spawn_loader`]'s background
/// thread, not from the GUI thread.  Returns an empty vec on any failure
/// (unrecognised shell, missing file, permission denied, parse error) --
/// failures are logged at `debug!`/`warn!` level but never surfaced to the
/// user, because shell history is a best-effort convenience.
#[must_use]
pub fn load_for_program(program: &Path, get_env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let kind = detect_shell_kind(program);
    if matches!(kind, ShellKind::Other) {
        trace!("shell_history: not a recognised shell ({program:?}); skipping seed");
        return Vec::new();
    }
    let Some(path) = resolve_history_path(kind, get_env) else {
        debug!("shell_history: could not resolve history path for {kind:?}");
        return Vec::new();
    };
    let content = match std::fs::read(&path) {
        Ok(bytes) => {
            // Shell history files are not guaranteed to be valid UTF-8.
            // zsh in particular stores command bytes >= 0x80 in a
            // "metafied" encoding, and bash will faithfully record
            // whatever bytes the user typed (Latin-1, Shift-JIS, paste
            // residue, etc.).  Strict `read_to_string` rejects the
            // entire file on a single stray byte, which produced the
            // `stream did not contain valid UTF-8` warning at pane
            // spawn time.  Lossy decoding preserves all valid UTF-8 and
            // substitutes U+FFFD for invalid sequences -- acceptable
            // for a best-effort palette seed.
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            trace!("shell_history: no history file at {path:?}; first-time shell?");
            return Vec::new();
        }
        Err(e) => {
            warn!("shell_history: failed to read {path:?}: {e}");
            return Vec::new();
        }
    };
    let mut entries = match kind {
        ShellKind::Bash => parse_bash_history(&content),
        ShellKind::Zsh => parse_zsh_history(&content),
        ShellKind::Fish => parse_fish_history(&content),
        ShellKind::Other => Vec::new(),
    };
    // Keep only the last HISTORY_SEED_CAP entries.
    let len = entries.len();
    if len > HISTORY_SEED_CAP {
        entries.drain(..len - HISTORY_SEED_CAP);
    }
    debug!(
        "shell_history: loaded {} entries from {path:?} for {kind:?}",
        entries.len()
    );
    entries
}

/// Spawn a background thread that loads the shell history and writes the
/// result into `slot`.
///
/// The thread is named `freminal-history-loader` for diagnostic visibility.
/// If `slot.set(...)` fails (slot already populated), the result is
/// discarded silently -- this is fine because the slot is per-pane and a
/// double-set would only occur if this function were called twice on the
/// same slot, which is a caller-side bug.
///
/// Cost: one thread per pane spawn, exits within milliseconds for typical
/// history-file sizes.  Loader does not hold any GUI-side locks; the only
/// shared state is the `OnceLock` slot.
pub fn spawn_loader<S: ::std::hash::BuildHasher + Send + 'static>(
    program: PathBuf,
    env_snapshot: HashMap<String, String, S>,
    slot: Arc<OnceLock<Vec<String>>>,
) {
    let builder = std::thread::Builder::new().name("freminal-history-loader".to_string());
    if let Err(e) = builder.spawn(move || {
        let entries = load_for_program(&program, &|key| env_snapshot.get(key).cloned());
        // Silently discard if already set (caller-side double-spawn bug).
        let _ = slot.set(entries);
    }) {
        warn!("shell_history: failed to spawn loader thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::HashMap;

    fn env(map: &[(&str, &str)]) -> HashMap<String, String> {
        map.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn lookup(map: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |k| map.get(k).cloned()
    }

    // ---------- detect_shell_kind ----------

    #[test]
    fn detect_bash_zsh_fish_by_basename() {
        assert_eq!(detect_shell_kind(Path::new("/bin/bash")), ShellKind::Bash);
        assert_eq!(
            detect_shell_kind(Path::new("/usr/local/bin/zsh")),
            ShellKind::Zsh
        );
        assert_eq!(
            detect_shell_kind(Path::new("/etc/profiles/per-user/fred/bin/fish")),
            ShellKind::Fish
        );
    }

    #[test]
    fn detect_other_for_unknown_shells() {
        assert_eq!(detect_shell_kind(Path::new("/bin/sh")), ShellKind::Other);
        assert_eq!(
            detect_shell_kind(Path::new("/usr/bin/python")),
            ShellKind::Other
        );
        assert_eq!(detect_shell_kind(Path::new("/bin/dash")), ShellKind::Other);
    }

    #[test]
    fn detect_other_when_no_filename() {
        assert_eq!(detect_shell_kind(Path::new("/")), ShellKind::Other);
        assert_eq!(detect_shell_kind(Path::new("")), ShellKind::Other);
    }

    // ---------- resolve_history_path ----------

    #[test]
    fn bash_uses_histfile_env_when_set() {
        let e = env(&[("HISTFILE", "/custom/bash_hist"), ("HOME", "/home/u")]);
        let p = resolve_history_path(ShellKind::Bash, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/custom/bash_hist"));
    }

    #[test]
    fn bash_falls_back_to_home_dot_bash_history() {
        let e = env(&[("HOME", "/home/u")]);
        let p = resolve_history_path(ShellKind::Bash, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/home/u/.bash_history"));
    }

    #[test]
    fn bash_returns_none_when_no_home_and_no_histfile() {
        let e = env(&[]);
        assert!(resolve_history_path(ShellKind::Bash, &lookup(&e)).is_none());
    }

    #[test]
    fn bash_ignores_empty_histfile() {
        let e = env(&[("HISTFILE", ""), ("HOME", "/h")]);
        let p = resolve_history_path(ShellKind::Bash, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/h/.bash_history"));
    }

    #[test]
    fn zsh_uses_histfile_env_when_set() {
        let e = env(&[("HISTFILE", "/zhist"), ("HOME", "/h")]);
        let p = resolve_history_path(ShellKind::Zsh, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/zhist"));
    }

    #[test]
    fn zsh_falls_back_to_home_dot_zsh_history() {
        let e = env(&[("HOME", "/h")]);
        let p = resolve_history_path(ShellKind::Zsh, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/h/.zsh_history"));
    }

    #[test]
    fn fish_uses_xdg_data_home_when_set() {
        let e = env(&[("XDG_DATA_HOME", "/xdg"), ("HOME", "/h")]);
        let p = resolve_history_path(ShellKind::Fish, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/xdg/fish/fish_history"));
    }

    #[test]
    fn fish_falls_back_to_home_local_share() {
        let e = env(&[("HOME", "/h")]);
        let p = resolve_history_path(ShellKind::Fish, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/h/.local/share/fish/fish_history"));
    }

    #[test]
    fn fish_ignores_empty_xdg_data_home() {
        let e = env(&[("XDG_DATA_HOME", ""), ("HOME", "/h")]);
        let p = resolve_history_path(ShellKind::Fish, &lookup(&e)).unwrap();
        assert_eq!(p, PathBuf::from("/h/.local/share/fish/fish_history"));
    }

    #[test]
    fn other_shell_kind_returns_none() {
        let e = env(&[("HOME", "/h"), ("HISTFILE", "/x")]);
        assert!(resolve_history_path(ShellKind::Other, &lookup(&e)).is_none());
    }

    // ---------- parse_bash_history ----------

    #[test]
    fn bash_plain_one_command_per_line() {
        let content = "ls\ncat foo\necho hi\n";
        assert_eq!(
            parse_bash_history(content),
            vec!["ls", "cat foo", "echo hi"]
        );
    }

    #[test]
    fn bash_skips_timestamp_comment_lines() {
        let content = "#1700000000\nls\n#1700000005\ncat foo\n";
        assert_eq!(parse_bash_history(content), vec!["ls", "cat foo"]);
    }

    #[test]
    fn bash_skips_empty_lines() {
        let content = "\nls\n\n\ncat foo\n\n";
        assert_eq!(parse_bash_history(content), vec!["ls", "cat foo"]);
    }

    #[test]
    fn bash_empty_input_yields_empty_vec() {
        assert!(parse_bash_history("").is_empty());
        assert!(parse_bash_history("\n\n\n").is_empty());
    }

    // ---------- parse_zsh_history ----------

    #[test]
    fn zsh_plain_format() {
        let content = "ls\ncat foo\n";
        assert_eq!(parse_zsh_history(content), vec!["ls", "cat foo"]);
    }

    #[test]
    fn zsh_extended_format() {
        let content = ": 1700000000:0;ls\n: 1700000005:2;cat foo\n";
        assert_eq!(parse_zsh_history(content), vec!["ls", "cat foo"]);
    }

    #[test]
    fn zsh_extended_with_semicolons_in_command() {
        let content = ": 1700000000:0;echo hi; echo bye\n";
        // Split on first `;` only -- the rest of the command is preserved.
        assert_eq!(parse_zsh_history(content), vec!["echo hi; echo bye"]);
    }

    #[test]
    fn zsh_mixed_plain_and_extended() {
        let content = ": 1700000000:0;ls\nplain command\n: 1700000005:0;echo done\n";
        assert_eq!(
            parse_zsh_history(content),
            vec!["ls", "plain command", "echo done"]
        );
    }

    #[test]
    fn zsh_skips_empty_lines() {
        let content = "\n: 1700000000:0;ls\n\n";
        assert_eq!(parse_zsh_history(content), vec!["ls"]);
    }

    #[test]
    fn zsh_extended_with_empty_command_is_dropped() {
        // ": <ts>:<dur>;" with no command after the semicolon
        let content = ": 1700000000:0;\n: 1700000005:0;real cmd\n";
        assert_eq!(parse_zsh_history(content), vec!["real cmd"]);
    }

    #[test]
    fn zsh_multi_line_command_reassembled_in_extended_format() {
        // zsh stores `echo first \<NL>second` as two physical lines where
        // the first ends with an unescaped backslash.  Without reassembly
        // `second` shows up as a phantom history entry.
        let content = ": 1700000000:0;echo first \\\nsecond\n";
        assert_eq!(parse_zsh_history(content), vec!["echo first second"]);
    }

    #[test]
    fn zsh_multi_line_command_three_lines_reassembled() {
        // Chained continuations: every line except the last ends with
        // a backslash.  All three pieces should collapse into one entry
        // joined by single spaces.
        let content = ": 1700000000:0;for i in 1 2 3\\\ndo echo $i\\\ndone\n";
        assert_eq!(
            parse_zsh_history(content),
            vec!["for i in 1 2 3 do echo $i done"]
        );
    }

    #[test]
    fn zsh_multi_line_command_eof_mid_continuation_salvaged() {
        // File ends with a trailing-backslash line and no follow-up.
        // Rather than discard the partial command, salvage what we have
        // so the user still sees the start in the palette.
        let content = ": 1700000000:0;echo unfinished\\\n";
        assert_eq!(parse_zsh_history(content), vec!["echo unfinished"]);
    }

    #[test]
    fn zsh_multi_line_continuation_leading_whitespace_trimmed() {
        // Continuation lines indented for readability should not carry
        // their leading whitespace into the reassembled command.
        let content = ": 1700000000:0;echo first\\\n  indented_part\n";
        assert_eq!(parse_zsh_history(content), vec!["echo first indented_part"]);
    }

    // ---------- parse_fish_history ----------

    #[test]
    fn fish_basic_cmd_blocks() {
        let content = "- cmd: ls -la\n  when: 1700000000\n- cmd: cat foo\n  when: 1700000010\n";
        assert_eq!(parse_fish_history(content), vec!["ls -la", "cat foo"]);
    }

    #[test]
    fn fish_with_paths_ignored() {
        let content = "- cmd: ls\n  when: 1700000000\n  paths:\n    - /tmp\n    - /home\n- cmd: cat\n  when: 1700000001\n";
        assert_eq!(parse_fish_history(content), vec!["ls", "cat"]);
    }

    #[test]
    fn fish_decodes_newline_escapes() {
        let content = "- cmd: echo a\\nb\n  when: 1\n";
        assert_eq!(parse_fish_history(content), vec!["echo a\nb"]);
    }

    #[test]
    fn fish_decodes_tab_and_backslash_escapes() {
        let content = "- cmd: printf a\\tb\\\\c\n  when: 1\n";
        assert_eq!(parse_fish_history(content), vec!["printf a\tb\\c"]);
    }

    #[test]
    fn fish_passes_through_unknown_escapes() {
        let content = "- cmd: echo \\q\n  when: 1\n";
        assert_eq!(parse_fish_history(content), vec!["echo \\q"]);
    }

    #[test]
    fn fish_trailing_backslash_preserved() {
        let content = "- cmd: foo\\\n  when: 1\n";
        assert_eq!(parse_fish_history(content), vec!["foo\\"]);
    }

    #[test]
    fn fish_empty_input_yields_empty_vec() {
        assert!(parse_fish_history("").is_empty());
    }

    // ---------- load_for_program (end-to-end via TempDir) ----------

    #[test]
    fn load_for_program_reads_bash_history() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let hist = tmp.path().join(".bash_history");
        std::fs::write(&hist, "ls\necho hi\n").expect("write");
        let e = env(&[("HOME", tmp.path().to_str().expect("utf8"))]);
        let v = load_for_program(Path::new("/bin/bash"), &lookup(&e));
        assert_eq!(v, vec!["ls", "echo hi"]);
    }

    #[test]
    fn load_for_program_reads_zsh_with_histfile_env() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let hist = tmp.path().join("custom_zhist");
        std::fs::write(&hist, ": 1700000000:0;cd /tmp\nls\n").expect("write");
        let e = env(&[("HISTFILE", hist.to_str().expect("utf8"))]);
        let v = load_for_program(Path::new("/usr/bin/zsh"), &lookup(&e));
        assert_eq!(v, vec!["cd /tmp", "ls"]);
    }

    #[test]
    fn load_for_program_reads_fish_history() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let fish_dir = tmp.path().join("fish");
        std::fs::create_dir_all(&fish_dir).expect("mkdir");
        let hist = fish_dir.join("fish_history");
        std::fs::write(&hist, "- cmd: ls\n  when: 1\n- cmd: cat foo\n  when: 2\n").expect("write");
        let e = env(&[("XDG_DATA_HOME", tmp.path().to_str().expect("utf8"))]);
        let v = load_for_program(Path::new("/usr/bin/fish"), &lookup(&e));
        assert_eq!(v, vec!["ls", "cat foo"]);
    }

    #[test]
    fn load_for_program_returns_empty_for_other_shell() {
        let e = env(&[("HOME", "/anything")]);
        assert!(load_for_program(Path::new("/bin/sh"), &lookup(&e)).is_empty());
        assert!(load_for_program(Path::new("/usr/bin/python"), &lookup(&e)).is_empty());
    }

    #[test]
    fn load_for_program_returns_empty_when_file_missing() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // No history file created.
        let e = env(&[("HOME", tmp.path().to_str().expect("utf8"))]);
        let v = load_for_program(Path::new("/bin/bash"), &lookup(&e));
        assert!(v.is_empty());
    }

    #[test]
    fn load_for_program_caps_at_history_seed_cap() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let hist = tmp.path().join(".bash_history");
        // Write 1500 lines; expect only the last 1000.
        let mut content = String::new();
        for i in 0..1500 {
            use std::fmt::Write as _;
            writeln!(content, "cmd_{i}").expect("write to String");
        }
        std::fs::write(&hist, &content).expect("write");
        let e = env(&[("HOME", tmp.path().to_str().expect("utf8"))]);
        let v = load_for_program(Path::new("/bin/bash"), &lookup(&e));
        assert_eq!(v.len(), HISTORY_SEED_CAP);
        // The first kept entry is cmd_500 (1500 - 1000 = 500).
        assert_eq!(v.first().map(String::as_str), Some("cmd_500"));
        assert_eq!(v.last().map(String::as_str), Some("cmd_1499"));
    }

    #[test]
    fn load_for_program_returns_empty_when_home_unset_for_bash() {
        let e = env(&[]);
        assert!(load_for_program(Path::new("/bin/bash"), &lookup(&e)).is_empty());
    }

    #[test]
    fn load_for_program_zsh_history_with_invalid_utf8_bytes_is_lossy_not_dropped() {
        // Regression for the warning observed at pane spawn:
        //
        //   shell_history: failed to read "~/.zsh_history":
        //     stream did not contain valid UTF-8
        //
        // zsh stores metafied bytes for any byte >= 0x80 (and bash will
        // faithfully record arbitrary bytes the user typed).  A strict
        // `read_to_string` rejects the entire file on the first stray
        // byte, dropping every valid entry above and below.  We now
        // decode lossily so valid entries survive; the offending bytes
        // become U+FFFD inside their own entry.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let hist = tmp.path().join("zhist_with_bad_bytes");
        // Three zsh-extended-format entries: valid, invalid byte, valid.
        // 0xC3 0x28 is an invalid UTF-8 continuation sequence.
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b": 1700000000:0;echo first\n");
        bytes.extend_from_slice(b": 1700000001:0;echo ");
        bytes.push(0xC3);
        bytes.push(0x28);
        bytes.extend_from_slice(b"middle\n");
        bytes.extend_from_slice(b": 1700000002:0;echo last\n");
        std::fs::write(&hist, &bytes).expect("write");
        let e = env(&[("HISTFILE", hist.to_str().expect("utf8"))]);
        let v = load_for_program(Path::new("/usr/bin/zsh"), &lookup(&e));
        // First and last entries must survive verbatim; middle entry
        // exists in some U+FFFD-substituted form (we don't assert its
        // exact text -- the point is the file no longer drops to []).
        assert_eq!(v.first().map(String::as_str), Some("echo first"));
        assert_eq!(v.last().map(String::as_str), Some("echo last"));
        assert_eq!(v.len(), 3, "all three entries should load, got {v:?}");
    }

    // ---------- spawn_loader (end-to-end) ----------

    #[test]
    fn spawn_loader_populates_slot() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let hist = tmp.path().join(".bash_history");
        std::fs::write(&hist, "alpha\nbeta\n").expect("write");
        let env_snapshot: HashMap<String, String> = std::iter::once((
            "HOME".to_owned(),
            tmp.path().to_str().expect("utf8").to_owned(),
        ))
        .collect();
        let slot: Arc<OnceLock<Vec<String>>> = Arc::new(OnceLock::new());
        spawn_loader(PathBuf::from("/bin/bash"), env_snapshot, Arc::clone(&slot));
        // Wait up to ~1s for the loader thread to populate.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while slot.get().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let loaded = slot.get().expect("loader did not populate within 1s");
        assert_eq!(loaded, &vec!["alpha".to_owned(), "beta".to_owned()]);
    }

    #[test]
    fn spawn_loader_populates_empty_for_unknown_shell() {
        let env_snapshot: HashMap<String, String> = HashMap::new();
        let slot: Arc<OnceLock<Vec<String>>> = Arc::new(OnceLock::new());
        spawn_loader(
            PathBuf::from("/usr/bin/python"),
            env_snapshot,
            Arc::clone(&slot),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while slot.get().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let loaded = slot.get().expect("loader did not populate within 1s");
        assert!(loaded.is_empty());
    }
}
