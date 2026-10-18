//! Fuzz the FREC v2 recording parser.
//!
//! Feeds arbitrary bytes to `parse_recording_from_bytes`, which decodes the
//! header, `MessagePack` metadata, the event stream, the seek index and the
//! footer from an in-memory slice. Recordings are user-supplied files, so the
//! parser must reject malformed input with a `ParseError` rather than panic.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use freminal_terminal_emulator::recording::parse_recording_from_bytes;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = parse_recording_from_bytes(data);
});
