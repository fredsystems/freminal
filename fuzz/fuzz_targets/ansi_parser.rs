//! Fuzz the ANSI escape-sequence parser in isolation.
//!
//! Drives `FreminalAnsiParser::push` with a single parser instance across
//! several chunks, so state carried between calls (a half-received CSI/OSC/DCS/
//! APC sequence, a split UTF-8 sequence) is exercised. The first input byte
//! seeds the chunk lengths and is not itself fed to the parser; see
//! `freminal_fuzz::split_chunks`.
//!
//! Every returned `TerminalOutput` is formatted with `Display`, which
//! exercises the formatting code reachable from untrusted bytes.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".
//! The parser makes no output-equivalence promise between chunkings that this
//! harness could check.

#![no_main]

use freminal_fuzz::split_chunks;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&seed, payload)) = data.split_first() else {
        return;
    };

    let mut parser = FreminalAnsiParser::new();
    for chunk in split_chunks(seed, 1, payload) {
        for output in parser.push(chunk) {
            let _ = output.to_string();
        }
    }
});
