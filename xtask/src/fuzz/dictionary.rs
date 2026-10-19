// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! libFuzzer dictionaries: which target uses which file under `fuzz/dict/`.
//!
//! A dictionary is a list of tokens (escape-sequence introducers, finals,
//! parameter prefixes) that libFuzzer splices into inputs, so the fuzzer does
//! not have to rediscover them byte by byte. Dictionaries are curated and
//! committed. Fuzz runs only read them; the repository is bound read-only
//! at its real path inside the bubblewrap sandbox, so the path is visible there.

use std::path::{Path, PathBuf};

/// Directory of dictionaries, relative to the workspace root.
const DICT_DIR: [&str; 2] = ["fuzz", "dict"];

/// Dictionary file name for `target`, or `None` when it has none.
///
/// The two byte-stream targets share one dictionary: `terminal_state` feeds the
/// same escape-sequence stream to the whole emulator that `ansi_parser` feeds to
/// the parser alone. A mapping (rather than a copy named after each target)
/// keeps a single file to maintain.
pub(super) fn dictionary_file_name(target: &str) -> Option<&'static str> {
    match target {
        "ansi_parser" | "terminal_state" => Some("ansi.dict"),
        _ => None,
    }
}

/// Absolute path of the dictionary for `target`, or `None` when the target has
/// no dictionary mapping or the mapped file does not exist.
///
/// `root` is the workspace root. A missing file silently yields `None` (a
/// target without a dictionary is normal; so is a checkout where it was
/// removed) rather than passing libFuzzer a path it would abort on.
pub(super) fn dictionary_for_target(root: &Path, target: &str) -> Option<PathBuf> {
    let name = dictionary_file_name(target)?;
    let path = DICT_DIR
        .iter()
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
        .join(name);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn byte_stream_targets_share_one_dictionary() {
        assert_eq!(dictionary_file_name("ansi_parser"), Some("ansi.dict"));
        assert_eq!(dictionary_file_name("terminal_state"), Some("ansi.dict"));
    }

    #[test]
    fn other_targets_have_no_dictionary() {
        for target in [
            "frec_parse",
            "sixel_parse",
            "config_toml",
            "base64",
            "no_such_target",
        ] {
            assert_eq!(dictionary_file_name(target), None, "{target}");
        }
    }

    #[test]
    fn existing_dictionary_resolves_under_fuzz_dict() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("fuzz").join("dict")).unwrap();
        fs::write(root.path().join("fuzz/dict/ansi.dict"), b"\"x\"\n").unwrap();
        let expected = root.path().join("fuzz").join("dict").join("ansi.dict");
        assert_eq!(
            dictionary_for_target(root.path(), "ansi_parser"),
            Some(expected.clone())
        );
        assert_eq!(
            dictionary_for_target(root.path(), "terminal_state"),
            Some(expected)
        );
        assert_eq!(dictionary_for_target(root.path(), "frec_parse"), None);
    }

    #[test]
    fn mapped_but_missing_file_yields_none() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(dictionary_for_target(root.path(), "ansi_parser"), None);
    }

    /// The committed dictionary exists and is well-formed enough that libFuzzer
    /// will not abort on it: every non-comment line is `"..."` or
    /// `name="..."`.
    #[test]
    fn committed_dictionary_exists_and_every_line_is_quoted() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let path = dictionary_for_target(&root, "ansi_parser").expect("fuzz/dict/ansi.dict");
        let text = fs::read_to_string(path).unwrap();
        let entries: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert!(entries.len() >= 90, "{} entries", entries.len());
        for line in entries {
            let value = if line.starts_with('"') {
                line
            } else {
                line.split_once('=').map_or(line, |(_, v)| v)
            };
            assert!(
                value.len() >= 2 && value.starts_with('"') && value.ends_with('"'),
                "malformed dictionary line: {line}"
            );
        }
    }
}
