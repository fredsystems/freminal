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
