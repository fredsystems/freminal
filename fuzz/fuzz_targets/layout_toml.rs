//! Fuzz saved-layout TOML parsing and the pure layout operations built on it.
//!
//! Layout files are user-authored TOML. Valid UTF-8 input is parsed with
//! `Layout::from_str_content`; a successful parse is then validated
//! (`validate`), resolved into a pane tree (`resolve`, which also validates)
//! and re-serialised (`to_toml_string`). Non-UTF-8 input is skipped.
//!
//! Deliberately NOT called: `apply_variables`, which expands `$ENV{NAME}` and
//! `~` from the process environment, and `from_file` / `save_to_file`, which
//! touch the filesystem. `resolve` and the others are pure.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use std::path::Path;

use freminal_common::layout::Layout;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(layout) = Layout::from_str_content(Path::new("fuzz-layout.toml"), text) else {
        return;
    };

    let _ = layout.validate();
    let _ = layout.resolve();
    let _ = layout.to_toml_string();
    let _ = layout.display_name();
});
