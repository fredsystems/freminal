// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A real PTY child, spawned through the production path with the production
//! environment, resolves the `xterm-freminal` terminfo entry (Task 151.9).
//!
//! The child is started by `FreminalPtyInputOutput::new`, so `TERM` and
//! `TERMINFO_DIRS` are exactly what a pane gets: the entry is extracted per pane
//! (the test binary is never inside a macOS `.app`).  The child runs `infocmp` and
//! `tput` against that entry, which settles on macOS whether Apple's ncurses finds
//! it in the tree the archive carries.
//!
//! Unix only: Windows has no terminfo database and the default `TERM` there is
//! `xterm-256color`.

#![cfg(unix)]
#![allow(clippy::expect_used)]

use std::collections::HashMap;
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
    FreminalPtyInputOutput, PtyRead, PtySpawnConfig, PtyWrite, ReplyQueue,
};

const END_MARKER: &str = "END_OF_CHILD_OUTPUT";
const SKIP_MARKER: &str = "SKIP_NO_TERMINFO_TOOLS";
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
echo "TERMINFO_DIRS=<$TERMINFO_DIRS>"
first="${TERMINFO_DIRS%%:*}"
if ls "$first/x/xterm-freminal" >/dev/null 2>&1; then
    echo "FIRST_DIR_HAS_ENTRY"
else
    echo "FIRST_DIR_MISSING_ENTRY"
fi
echo BEGIN_INFOCMP
infocmp -x xterm-freminal >"$HOME/infocmp.out"
echo "INFOCMP_STATUS=$?"
cat "$HOME/infocmp.out"
if grep -q "from file: $first/" "$HOME/infocmp.out"; then
    echo "RESOLVED_FROM_FIRST_DIR"
else
    echo "NOT_RESOLVED_FROM_FIRST_DIR"
fi
echo BEGIN_TPUT
colors=$(tput colors)
echo "TPUT_COLORS=<$colors>"
bright=$(tput setaf 9)
echo "TPUT_STATUS=$?"
printf '%s' "$bright" | od -An -tx1
echo END_OF_CHILD_OUTPUT
"#;

fn run_child() -> String {
    // ncurses searches `~/.terminfo` before `TERMINFO_DIRS`, so an entry in the real
    // home would hide the extracted tree; give the child an empty `HOME` through the
    // pane `env`, as a layout could.
    let home = tempfile::tempdir().expect("temp HOME");
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
    String::from_utf8_lossy(&collected).replace('\r', "")
}

#[test]
fn a_real_child_resolves_xterm_freminal() {
    let output = run_child();

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
    assert!(
        output.contains("FIRST_DIR_HAS_ENTRY"),
        "the first TERMINFO_DIRS entry must be the extracted tree holding x/xterm-freminal; \
         full child output:\n{output}"
    );
    assert!(
        output.contains("INFOCMP_STATUS=0"),
        "infocmp -x xterm-freminal failed; full child output:\n{output}"
    );

    assert!(
        output.contains("\nRESOLVED_FROM_FIRST_DIR"),
        "infocmp must read the extracted per-pane tree, not another entry; \
         full child output:\n{output}"
    );

    // infocmp escapes `:` inside a capability as `\:`; compare on the plain form.
    let infocmp = output
        .split("BEGIN_INFOCMP")
        .nth(1)
        .and_then(|rest| rest.split("RESOLVED_FROM_FIRST_DIR").next())
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
        .and_then(|rest| rest.split(END_MARKER).next())
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
