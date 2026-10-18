//! Fuzz the `CompactRow` scrollback-row byte format.
//!
//! `CompactRow::from_bytes` decodes the format that scrollback compression
//! (`CompressedBlock`) stores, and its docs promise it "never panics or indexes
//! out of bounds on malformed input". When it decodes a row, this target also
//! rebuilds a `Row` with `to_row` and re-encodes the row.
//!
//! Invariants asserted (all stated by the module documentation):
//!
//! 1. The consumed length reported by `from_bytes` never exceeds the input.
//! 2. `from_bytes(&row.to_bytes())` succeeds and consumes every byte
//!    `to_bytes` wrote ("`from_bytes` must consume every byte `to_bytes`
//!    wrote", the module's byte round-trip contract).
//! 3. Re-encoding is stable: the row decoded from `row.to_bytes()` encodes to
//!    the same bytes. `CompactRow` does not derive `PartialEq`, so equality is
//!    checked on the encoding.
//!
//! The byte-level round trip is asserted only from a decoded row, not that
//! `to_bytes` reproduces the fuzz input: the decoder accepts non-canonical
//! encodings, which the module does not promise to preserve.

#![no_main]

use freminal_buffer::compact_row::CompactRow;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((row, consumed)) = CompactRow::from_bytes(data) else {
        return;
    };
    assert!(
        consumed <= data.len(),
        "from_bytes consumed {consumed} bytes of a {}-byte input",
        data.len()
    );

    let _ = row.to_row();

    let encoded = row.to_bytes();
    let Some((decoded, decoded_consumed)) = CompactRow::from_bytes(&encoded) else {
        panic!("a row produced by from_bytes failed to decode after to_bytes");
    };
    assert_eq!(
        decoded_consumed,
        encoded.len(),
        "from_bytes must consume every byte to_bytes wrote"
    );
    assert_eq!(
        decoded.to_bytes(),
        encoded,
        "re-encoding a decoded row changed its bytes"
    );
});
