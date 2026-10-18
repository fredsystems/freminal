//! Fuzz the Sixel image decoder.
//!
//! Feeds arbitrary bytes to `parse_sixel` as the inner content of a DCS
//! sequence (`P1;P2;P3 q <sixel-data>`, envelope already stripped). Sixel data
//! carries attacker-controlled raster dimensions, repeat counts and colour
//! registers, so this target is the one most likely to find a runaway
//! allocation.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use freminal_common::buffer_states::sixel::parse_sixel;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = parse_sixel(data);
});
