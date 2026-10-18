//! Fuzz the kitty desktop-notification (OSC 99) parser.
//!
//! `parse_osc_99` takes the metadata and payload regions of
//! `ESC ] 99 ; <metadata> ; <payload> ST` separately. The input is split at its
//! first `;`: everything before is metadata and everything after is payload. An
//! input with no `;` is all metadata with an empty payload.
//!
//! Both outcomes are `Debug`-formatted to exercise those derives.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use freminal_common::buffer_states::osc_notify_99::parse_osc_99;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (metadata, payload) = data.iter().position(|&b| b == b';').map_or_else(
        || (data, &[][..]),
        |at| {
            let (metadata, rest) = data.split_at(at);
            (metadata, rest.get(1..).unwrap_or_default())
        },
    );
    let _ = format!("{:?}", parse_osc_99(metadata, payload));
});
