// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The embedded terminfo archive and the committed compiled tree (`res/terminfo/`)
//! must agree, and must carry the entry in both the letter and the hex layout.
//!
//! `build.rs` writes the tree from `tic -x` and packs the tar from it; these tests
//! pin the result, including that extraction (as the per-PTY extraction does) yields
//! a usable tree.

#![allow(clippy::expect_used)]

use std::io::Read;

use freminal_common::terminfo::TERMINFO;

/// Every path the archive must hold, with the bytes of the committed tree file
/// that it must equal.
const TREE: [(&str, &[u8]); 4] = [
    (
        "66/freminal",
        include_bytes!("../../res/terminfo/66/freminal"),
    ),
    (
        "78/xterm-freminal",
        include_bytes!("../../res/terminfo/78/xterm-freminal"),
    ),
    (
        "f/freminal",
        include_bytes!("../../res/terminfo/f/freminal"),
    ),
    (
        "x/xterm-freminal",
        include_bytes!("../../res/terminfo/x/xterm-freminal"),
    ),
];

fn tar_members() -> Vec<(String, u32, Vec<u8>, tar::EntryType)> {
    let mut archive = tar::Archive::new(TERMINFO);
    let mut members = Vec::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry
            .path()
            .expect("entry path")
            .to_string_lossy()
            .into_owned();
        let mode = entry.header().mode().expect("entry mode");
        let kind = entry.header().entry_type();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("entry data");
        members.push((path, mode, data, kind));
    }
    members
}

#[test]
fn tar_holds_exactly_the_four_paths_as_plain_files() {
    let members = tar_members();
    let paths: Vec<&str> = members.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(
        paths,
        [
            "66/freminal",
            "78/xterm-freminal",
            "f/freminal",
            "x/xterm-freminal"
        ]
    );
    for (path, mode, data, kind) in &members {
        assert_eq!(
            *kind,
            tar::EntryType::Regular,
            "{path} is not a regular file"
        );
        assert_eq!(*mode & 0o777, 0o644, "{path} mode");
        assert!(!data.is_empty(), "{path} is empty");
    }
}

#[test]
fn tar_members_equal_the_committed_tree_byte_for_byte() {
    let members = tar_members();
    for (path, expected) in TREE {
        let member = members
            .iter()
            .find(|m| m.0 == path)
            .unwrap_or_else(|| panic!("{path} missing from the tar"));
        assert!(
            member.2 == expected,
            "{path} differs from res/terminfo/{path}"
        );
    }
}

#[test]
fn letter_and_hex_copies_are_identical() {
    let by_path = |wanted: &str| {
        TREE.iter()
            .find(|(path, _)| *path == wanted)
            .map(|(_, bytes)| *bytes)
            .expect("known tree path")
    };
    assert!(by_path("x/xterm-freminal") == by_path("78/xterm-freminal"));
    assert!(by_path("f/freminal") == by_path("66/freminal"));
    // `tic` compiles one entry with both names, so the alias is the same bytes.
    assert!(by_path("x/xterm-freminal") == by_path("f/freminal"));
}

/// The source file is a public URL (linked from the README) and a release asset;
/// `include_str!` pins that the path exists.
const SOURCE: &str = include_str!("../../res/freminal.ti");

#[test]
fn source_compiles_from_stdin_to_the_committed_entry() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let dir = tempfile::tempdir().expect("temp dir");
    let mut child = match Command::new("tic")
        .args(["-x", "-o"])
        .arg(dir.path())
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("SKIPPED: `tic` is not available ({err}); stdin compile not checked");
            return;
        }
    };
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(SOURCE.as_bytes())
        .expect("write the source to tic");
    let output = child.wait_with_output().expect("wait for tic");
    assert!(
        output.status.success(),
        "tic rejected res/freminal.ti on stdin: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let committed = include_bytes!("../../res/terminfo/x/xterm-freminal");
    let compiled = ["x/xterm-freminal", "78/xterm-freminal"]
        .iter()
        .find_map(|rel| std::fs::read(dir.path().join(rel)).ok())
        .expect("tic wrote the entry in the letter or the hex layout");
    assert!(
        compiled == committed,
        "stdin compile differs from res/terminfo/x/xterm-freminal"
    );
}

#[test]
fn extraction_yields_the_four_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    tar::Archive::new(TERMINFO)
        .unpack(dir.path())
        .expect("unpack embedded terminfo");
    for (path, expected) in TREE {
        let extracted = std::fs::read(dir.path().join(path)).expect("extracted file");
        assert!(
            extracted == expected,
            "{path} extracted with different bytes"
        );
    }
}
