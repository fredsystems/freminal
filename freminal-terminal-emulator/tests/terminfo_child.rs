// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A real PTY child, spawned through the production path with the production
//! environment, resolves the `xterm-freminal` terminfo entry (Task 151.9).
//!
//! The child is started by `FreminalPtyInputOutput::new`, so `TERM` and
//! `TERMINFO` are exactly what a pane gets: the entry is extracted per pane
//! (the test binary is never inside a macOS `.app`).  The child runs `infocmp` and
//! `tput` against that entry, which settles on macOS whether Apple's ncurses finds
//! it in the tree the archive carries.
//!
//! The child's `HOME` holds a stale `~/.terminfo/x/xterm-freminal` (8 colours), the
//! shape a per-user `tic -x` install leaves behind.  `TERMINFO` is searched before
//! `~/.terminfo`, so the pane's own entry must still win, and `TERMINFO` must not
//! hide other `TERM` values (ncurses falls through to the system database).
//!
//! Unix only: Windows has no terminfo database and the default `TERM` there is
//! `xterm-256color`.

#![cfg(unix)]
#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use freminal_common::{
    config::ShellConfig,
    pty_write::FreminalTerminalSize,
    term_name::{TermName, TermPlatform},
};
use freminal_terminal_emulator::io::{
    FreminalPtyInputOutput, PtyRead, PtySpawnConfig, PtyWrite, ReplyQueue, SudoTerminfo,
};

const END_MARKER: &str = "END_OF_CHILD_OUTPUT";
const SKIP_MARKER: &str = "SKIP_NO_TERMINFO_TOOLS";
const SKIP_XTERM_256COLOR_MARKER: &str = "SKIP_NO_XTERM_256COLOR";
const FREMINAL_TI: &str = include_str!("../../res/freminal.ti");
const TIMEOUT: Duration = Duration::from_secs(30);

/// Prints each fact on its own marked line so the assertions do not depend on the
/// layout of `infocmp` output.  `END_OF_CHILD_OUTPUT` is printed last, so the
/// reader can stop on it instead of racing the child's exit.
const SCRIPT: &str = r#"
if ! command -v infocmp >/dev/null 2>&1 || ! command -v tput >/dev/null 2>&1; then
    echo SKIP_NO_TERMINFO_TOOLS
    echo END_OF_CHILD_OUTPUT
    exit 0
fi
echo "TERM=<$TERM>"
echo "TERMINFO=<$TERMINFO>"
echo "STALE_COLORS=<$(env -u TERMINFO tput -T xterm-freminal colors 2>&1)>"
if ls "$TERMINFO/x/xterm-freminal" >/dev/null 2>&1; then
    echo "PANE_DIR_HAS_ENTRY"
else
    echo "PANE_DIR_MISSING_ENTRY"
fi
echo BEGIN_INFOCMP
infocmp -x xterm-freminal >"$HOME/infocmp.out"
echo "INFOCMP_STATUS=$?"
cat "$HOME/infocmp.out"
if grep -q "from file: $TERMINFO/" "$HOME/infocmp.out"; then
    echo "RESOLVED_FROM_PANE_DIR"
else
    echo "NOT_RESOLVED_FROM_PANE_DIR"
fi
echo BEGIN_TPUT
colors=$(tput colors)
echo "TPUT_COLORS=<$colors>"
bright=$(tput setaf 9)
echo "TPUT_STATUS=$?"
printf '%s' "$bright" | od -An -tx1
echo BEGIN_OTHER_TERM
if env -u TERMINFO infocmp -x xterm-256color >/dev/null 2>&1; then
    TERM=xterm-256color infocmp -x xterm-256color >"$HOME/other.out" 2>&1
    echo "OTHER_STATUS=$?"
    if grep -q "from file: $TERMINFO/" "$HOME/other.out"; then
        echo "OTHER_FROM_PANE_DIR"
    else
        echo "OTHER_FROM_ELSEWHERE"
    fi
    echo "OTHER_COLORS=<$(TERM=xterm-256color tput colors 2>&1)>"
else
    echo SKIP_NO_XTERM_256COLOR
fi
echo END_OF_CHILD_OUTPUT
"#;

/// Install a deliberately stale `xterm-freminal` (8 colours) under `home/.terminfo`.
///
/// Returns `false` when `tic` is not installed, so the caller can skip.
fn install_stale_entry(home: &Path) -> bool {
    let stale = FREMINAL_TI.replace("colors#256", "colors#8");
    assert_ne!(
        stale, FREMINAL_TI,
        "res/freminal.ti no longer has colors#256"
    );
    let spawned = Command::new("tic")
        .args(["-x", "-o"])
        .arg(home.join(".terminfo"))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return false;
    };
    child
        .stdin
        .take()
        .expect("tic stdin")
        .write_all(stale.as_bytes())
        .expect("write stale entry to tic");
    let output = child.wait_with_output().expect("wait for tic");
    assert!(
        output.status.success(),
        "tic rejected the stale entry: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        home.join(".terminfo/x/xterm-freminal").is_file(),
        "tic did not write x/xterm-freminal under the temp HOME"
    );
    true
}

/// Runs the child, or returns `None` when `tic` is missing.
fn run_child() -> Option<String> {
    // The child's `HOME` carries a stale per-user entry that ncurses would find
    // before `TERMINFO_DIRS`; the pane's `TERMINFO` must beat it.
    let home = tempfile::tempdir().expect("temp HOME");
    if !install_stale_entry(home.path()) {
        return None;
    }
    let pane_env = HashMap::from([(
        String::from("HOME"),
        home.path().to_string_lossy().into_owned(),
    )]);
    let term = TermName::resolve(&ShellConfig::default(), TermPlatform::current(), None);
    assert_eq!(term.as_str(), "xterm-freminal", "production default TERM");

    let (_write_tx, write_rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let (_reply_tx, reply_rx) = crossbeam_channel::bounded::<PtyWrite>(1);
    let (send_tx, send_rx) = crossbeam_channel::unbounded::<PtyRead>();
    let spawn_cfg = PtySpawnConfig {
        command: Some((
            String::from("/bin/sh"),
            vec![String::from("-c"), String::from(SCRIPT)],
        )),
        shell: None,
        cwd: None,
        extra_env: Some(&pane_env),
        term: &term,
        set_term_program: false,
        sudo_terminfo: SudoTerminfo::Unchanged,
    };
    // Holds the per-pane terminfo directory; it must outlive the child.
    let _pty = match FreminalPtyInputOutput::new(
        write_rx,
        ReplyQueue {
            rx: reply_rx,
            queued_bytes: Arc::new(AtomicUsize::new(0)),
            pending_resize: Arc::new(arc_swap::ArcSwapOption::const_empty()),
        },
        send_tx,
        spawn_cfg,
        &FreminalTerminalSize {
            width: 200,
            height: 50,
            pixel_width: 0,
            pixel_height: 0,
        },
        0,
    ) {
        Ok(pty) => pty,
        Err(e) => panic!("failed to spawn the PTY child: {e}"),
    };

    let mut collected = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match send_rx.recv_timeout(remaining) {
            Ok(read) => {
                collected.extend_from_slice(&read.buf);
                if String::from_utf8_lossy(&collected).contains(END_MARKER) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => panic!(
                "child did not finish within {TIMEOUT:?}; output so far:\n{}",
                String::from_utf8_lossy(&collected)
            ),
        }
    }
    Some(String::from_utf8_lossy(&collected).replace('\r', ""))
}

#[test]
fn a_real_child_resolves_xterm_freminal() {
    let Some(output) = run_child() else {
        eprintln!("SKIPPED: tic is not installed, cannot build a stale ~/.terminfo entry");
        return;
    };

    if output.contains(SKIP_MARKER) {
        eprintln!("SKIPPED: infocmp or tput is not installed, cannot resolve terminfo in a child");
        return;
    }

    assert!(
        output.contains(END_MARKER),
        "child ended before finishing; full child output:\n{output}"
    );
    assert!(
        output.contains("TERM=<xterm-freminal>"),
        "child TERM; full child output:\n{output}"
    );
    let terminfo = output
        .lines()
        .find_map(|l| {
            l.strip_prefix("TERMINFO=<")
                .and_then(|r| r.strip_suffix('>'))
        })
        .unwrap_or_default();
    assert!(
        !terminfo.is_empty(),
        "the child's $TERMINFO must be set; full child output:\n{output}"
    );
    assert!(
        output.contains("PANE_DIR_HAS_ENTRY"),
        "the directory $TERMINFO names must hold x/xterm-freminal (checked in the child, \
         because the pane directory is gone once the PTY drops); full child output:\n{output}"
    );
    assert!(
        output.contains("STALE_COLORS=<8>"),
        "the stale ~/.terminfo entry must be what the child would find without TERMINFO \
         (otherwise this test proves nothing); full child output:\n{output}"
    );
    assert!(
        output.contains("INFOCMP_STATUS=0"),
        "infocmp -x xterm-freminal failed; full child output:\n{output}"
    );

    assert!(
        output.contains("\nRESOLVED_FROM_PANE_DIR"),
        "infocmp must read the pane's $TERMINFO tree, not the stale ~/.terminfo entry; \
         full child output:\n{output}"
    );

    // infocmp escapes `:` inside a capability as `\:`; compare on the plain form.
    let infocmp = output
        .split("BEGIN_INFOCMP")
        .nth(1)
        .and_then(|rest| rest.split("RESOLVED_FROM_PANE_DIR").next())
        .unwrap_or_default()
        .replace("\\:", ":");
    for needle in ["xterm-freminal", "Smulx=\\E[4:%p1%dm", "Sync="] {
        assert!(
            infocmp.contains(needle),
            "infocmp output lacks {needle:?}; full child output:\n{output}"
        );
    }

    // `tput` cannot look up user-defined capabilities such as `Smulx` ("unknown
    // terminfo capability"), so `infocmp` above covers those; here `tput` proves a
    // child's ncurses resolves the entry for `$TERM` and expands its strings.
    assert!(
        output.contains("TPUT_COLORS=<256>"),
        "tput colors must report 256; full child output:\n{output}"
    );
    assert!(
        output.contains("TPUT_STATUS=0"),
        "tput setaf 9 failed; full child output:\n{output}"
    );
    let tput = output
        .split("BEGIN_TPUT")
        .nth(1)
        .and_then(|rest| rest.split("BEGIN_OTHER_TERM").next())
        .unwrap_or_default();
    let hex = tput
        .split("TPUT_STATUS=0")
        .nth(1)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // ESC [ 9 1 m
    assert_eq!(
        hex, "1b 5b 39 31 6d",
        "tput setaf 9 must produce ESC [ 9 1 m; full child output:\n{output}"
    );
}

#[test]
fn terminfo_does_not_hide_other_term_values() {
    let Some(output) = run_child() else {
        eprintln!("SKIPPED: tic is not installed, cannot build a stale ~/.terminfo entry");
        return;
    };
    if output.contains(SKIP_MARKER) {
        eprintln!("SKIPPED: infocmp or tput is not installed");
        return;
    }
    assert!(
        output.contains(END_MARKER),
        "child ended before finishing; full child output:\n{output}"
    );
    if output.contains(SKIP_XTERM_256COLOR_MARKER) {
        eprintln!("SKIPPED: the system has no xterm-256color terminfo entry");
        return;
    }
    assert!(
        output.contains("OTHER_STATUS=0"),
        "TERM=xterm-256color must still resolve with TERMINFO set; full child output:\n{output}"
    );
    assert!(
        output.contains("OTHER_FROM_ELSEWHERE"),
        "xterm-256color is not in the pane's tree, so it must come from the system database; \
         full child output:\n{output}"
    );
    assert!(
        output.contains("OTHER_COLORS=<256>"),
        "tput colors for xterm-256color; full child output:\n{output}"
    );
}
