//! Fuzz the kitty graphics protocol APC parser.
//!
//! Feeds arbitrary bytes to `parse_kitty_graphics`, which strips the optional
//! `_` ... `ESC \` envelope, splits the control data from the base64 payload and
//! decodes the `key=value` control keys. Both outcomes are `Debug`-formatted to
//! exercise those derives.
//!
//! This target covers parsing only; it never touches the filesystem. The
//! side-effecting transmission paths are reached through the `terminal_state`
//! target.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use freminal_common::buffer_states::kitty_graphics::parse_kitty_graphics;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = format!("{:?}", parse_kitty_graphics(data));
});
