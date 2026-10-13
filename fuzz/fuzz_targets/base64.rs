//! Fuzz the base64 decoders (`decode`, `decode_strict`, `StreamDecoder`).
//!
//! The raw input is decoded three ways: lenient, strict, and through a
//! `StreamDecoder` fed in chunks (chunk lengths seeded by the first input
//! byte). Decoding hostile text must return an error, never panic.
//!
//! Invariants asserted, each mirroring a property already stated by the
//! module's own proptests in `base64.rs`:
//!
//! - `strict_success_implies_lenient_agrees`: if `decode_strict(input)`
//!   succeeds, `decode(input)` succeeds with the same bytes.
//! - `round_trip_padded` / `round_trip_unpadded`: for `data` = the raw input,
//!   `decode(encode(data))` and `decode_strict(encode(data))` return `data`, and
//!   `decode(encode_unpadded(data))` returns `data`.
//! - `stream_equals_one_shot`: the `StreamDecoder` fed `encode(data)` or
//!   `encode_unpadded(data)` (picked by the parity of the first byte) in chunks
//!   returns exactly what one-shot `decode` returns. This is asserted only on
//!   encoder output, as in the proptest: for arbitrary text the stream decoder
//!   deliberately differs from `decode` (interior `=` closes a quantum instead
//!   of being rejected), so no equivalence is asserted for raw input there.

#![no_main]

use freminal_common::base64::{StreamDecoder, decode, decode_strict, encode, encode_unpadded};
use freminal_fuzz::split_chunks;
use libfuzzer_sys::fuzz_target;

/// Decode `chunks` through a fresh `StreamDecoder`, as the module's tests do.
fn stream_decode(chunks: &[&[u8]]) -> Option<Vec<u8>> {
    let mut decoder = StreamDecoder::new();
    let mut out = Vec::new();
    for chunk in chunks {
        decoder.feed(chunk, &mut out).ok()?;
    }
    decoder.finish(&mut out).ok()?;
    Some(out)
}

fuzz_target!(|data: &[u8]| {
    let seed = data.first().copied().unwrap_or(0);

    // Hostile text: must not panic, whatever the outcome.
    let lenient = decode(data);
    let strict = decode_strict(data);
    let _ = stream_decode(&split_chunks(seed, 1, data));
    let _ = stream_decode(&[data]);

    if let Ok(strict_bytes) = strict {
        assert_eq!(
            lenient.ok().as_deref(),
            Some(strict_bytes.as_slice()),
            "strict decode succeeded but lenient decode disagreed"
        );
    }

    // Encoder output: the documented round-trip and stream-equals-one-shot
    // properties.
    let padded = encode(data);
    assert_eq!(decode(padded.as_bytes()).ok().as_deref(), Some(data));
    assert_eq!(decode_strict(padded.as_bytes()).ok().as_deref(), Some(data));

    let unpadded = encode_unpadded(data);
    assert!(!unpadded.contains('='), "encode_unpadded emitted padding");
    assert_eq!(decode(unpadded.as_bytes()).ok().as_deref(), Some(data));

    let text = if seed % 2 == 0 { &padded } else { &unpadded };
    let expected = decode(text.as_bytes()).ok();
    let streamed = stream_decode(&split_chunks(seed, 1, text.as_bytes()));
    assert_eq!(
        streamed, expected,
        "stream decode of encoder output differs from one-shot decode"
    );
});
