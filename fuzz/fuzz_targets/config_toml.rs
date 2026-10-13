//! Fuzz the user configuration TOML deserialiser.
//!
//! Valid UTF-8 input is deserialised straight into
//! `freminal_common::config::Config`. Non-UTF-8 input is skipped.
//!
//! Limitation: the production loader goes through the private `ConfigPartial`
//! type, `Config::apply_partial` and the private `Config::validate`. None of
//! those are reachable from outside `freminal-common`, so this target covers
//! the `Config` deserialisation derive and the `toml` parser only.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use freminal_common::config::Config;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = toml::from_str::<Config>(text);
});
