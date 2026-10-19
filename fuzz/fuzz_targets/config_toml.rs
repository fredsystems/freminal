//! Fuzz the user configuration TOML deserialiser.
//!
//! Valid UTF-8 input is fed to `freminal_common::config::Config::from_toml_str`,
//! which runs the production pipeline minus the filesystem: parse into the
//! private `ConfigPartial`, `Config::apply_partial`, then `Config::validate`.
//! Non-UTF-8 input is skipped.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = freminal_common::config::Config::from_toml_str(text);
});
