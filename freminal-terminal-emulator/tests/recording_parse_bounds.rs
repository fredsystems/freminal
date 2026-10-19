//! Bounds tests for `parse_recording_from_bytes` (issue #556).
//!
//! Every count, length and offset in a `.frec` file is attacker-controlled. Each
//! test here crafts a file whose declared size disagrees with the bytes present
//! and asserts the parser returns a typed [`ParseError`] instead of panicking or
//! asking the allocator for an absurd amount of memory.

#![allow(clippy::expect_used)]

use freminal_terminal_emulator::recording::{
    EVENT_HEADER_SIZE, EventPayload, FOOTER_SIZE, FREC_MAGIC, FREC_VERSION, HEADER_FIXED_SIZE,
    ParseError, RecordingMetadata, SEEK_INDEX_ENTRY_SIZE, TopologySnapshot,
    parse_recording_from_bytes,
};

fn metadata() -> RecordingMetadata {
    RecordingMetadata {
        freminal_version: "0.13.0".to_string(),
        created_at: 1,
        term: "xterm-256color".to_string(),
        initial_topology: TopologySnapshot { windows: vec![] },
        scrollback_limit: 10,
    }
}

/// Serialise `value` the way the recorder does.
fn pack<T: serde::Serialize>(value: &T) -> Vec<u8> {
    rmp_serde::to_vec_named(value).expect("serialize test value")
}

/// A file assembled from raw parts, so each test can lie about one of them.
struct Frame {
    /// Value written for the header's `metadata_length` field.
    declared_meta_len: u32,
    metadata: Vec<u8>,
    /// Raw bytes between the metadata and the seek index.
    events: Vec<u8>,
    /// Value written for the footer's `seek_index_offset`; `None` means "where the
    /// seek index really starts".
    declared_seek_offset: Option<u64>,
    /// Value written for the seek index's `entry_count`.
    declared_entry_count: u64,
    /// Raw seek index entries following `entry_count`.
    table: Vec<u8>,
}

impl Frame {
    fn valid() -> Self {
        let metadata = pack(&metadata());
        Self {
            declared_meta_len: u32::try_from(metadata.len()).expect("small metadata"),
            metadata,
            events: Vec::new(),
            declared_seek_offset: None,
            declared_entry_count: 0,
            table: Vec::new(),
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(FREC_MAGIC);
        out.push(FREC_VERSION);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&self.declared_meta_len.to_le_bytes());
        out.extend_from_slice(&self.metadata);
        out.extend_from_slice(&self.events);

        let real_seek_offset = u64::try_from(out.len()).expect("small file");
        out.extend_from_slice(&self.declared_entry_count.to_le_bytes());
        out.extend_from_slice(&self.table);

        let seek_offset = self.declared_seek_offset.unwrap_or(real_seek_offset);
        out.extend_from_slice(&seek_offset.to_le_bytes());
        out.extend_from_slice(&0_u64.to_le_bytes());
        out.extend_from_slice(&0_u64.to_le_bytes());
        out.extend_from_slice(FREC_MAGIC);
        out
    }
}

/// One event record: header (timestamp, type byte, declared length) plus payload bytes.
fn event_record(declared_payload_len: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0_u64.to_le_bytes());
    out.push(0x01);
    out.extend_from_slice(&declared_payload_len.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn assert_invalid_format(result: Result<impl std::fmt::Debug, ParseError>, expected: &str) {
    match result {
        Err(ParseError::InvalidFormat(message)) => assert!(
            message.contains(expected),
            "expected InvalidFormat containing {expected:?}, got {message:?}"
        ),
        other => panic!("expected InvalidFormat({expected:?}), got {other:?}"),
    }
}

#[test]
fn well_formed_file_still_parses() {
    let mut frame = Frame::valid();
    let payload = pack(&EventPayload::PaneClose { pane_id: 7 });
    frame.events = event_record(u32::try_from(payload.len()).expect("small"), &payload);
    frame.declared_entry_count = 1;
    frame.table = [3_u64.to_le_bytes(), 9_u64.to_le_bytes()].concat();

    let parsed = parse_recording_from_bytes(&frame.build()).expect("valid file parses");
    assert_eq!(parsed.metadata, metadata());
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(parsed.seek_index.len(), 1);
    assert_eq!(parsed.seek_index[0].timestamp_us, 3);
    assert_eq!(parsed.seek_index[0].file_offset, 9);
}

#[test]
fn seek_index_entry_count_far_beyond_file_is_rejected() {
    // The #556 reproducer's count: asked the allocator for 0x2000000000060 bytes.
    let mut frame = Frame::valid();
    frame.declared_entry_count = 0x2000_0000_0006;
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "truncated seek index",
    );
}

#[test]
fn seek_index_entry_count_whose_byte_size_overflows_is_rejected() {
    // 2^60 * 16 overflows usize; the old code panicked with `capacity overflow`.
    let mut frame = Frame::valid();
    frame.declared_entry_count = 1 << 60;
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "truncated seek index",
    );

    frame.declared_entry_count = u64::MAX;
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "truncated seek index",
    );
}

#[test]
fn seek_index_entry_count_one_past_the_table_is_rejected() {
    let mut frame = Frame::valid();
    frame.declared_entry_count = 2;
    frame.table = vec![0; SEEK_INDEX_ENTRY_SIZE];
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "truncated seek index",
    );
}

#[test]
fn seek_index_entries_may_fill_the_space_up_to_the_footer_exactly() {
    let mut frame = Frame::valid();
    frame.declared_entry_count = 2;
    frame.table = vec![0; 2 * SEEK_INDEX_ENTRY_SIZE];
    let parsed = parse_recording_from_bytes(&frame.build()).expect("exact fit parses");
    assert_eq!(parsed.seek_index.len(), 2);
}

#[test]
fn seek_index_offset_that_overflows_when_advanced_is_rejected() {
    for offset in [u64::MAX, u64::MAX - 7, u64::MAX - 8] {
        let mut frame = Frame::valid();
        frame.declared_seek_offset = Some(offset);
        assert!(
            parse_recording_from_bytes(&frame.build()).is_err(),
            "seek_index_offset {offset:#x} must be rejected"
        );
    }
}

#[test]
fn seek_index_that_starts_inside_the_footer_is_rejected() {
    // One event whose declared payload runs 4 bytes into the seek index area (the
    // payload decoder ignores trailing bytes). The event loop ends at
    // `footer_start - 4`, which is also where the footer claims the seek index
    // starts, so the count field would straddle the footer.
    let mut frame = Frame::valid();
    let payload = pack(&EventPayload::PaneClose { pane_id: 7 });
    let declared = u32::try_from(payload.len() + 4).expect("small");
    frame.events = event_record(declared, &payload);
    let real_seek_offset = HEADER_FIXED_SIZE + frame.metadata.len() + frame.events.len();
    frame.declared_seek_offset = Some(u64::try_from(real_seek_offset + 4).expect("small"));
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "seek index overlaps footer",
    );
}

#[test]
fn metadata_length_past_end_of_file_is_rejected() {
    for declared in [u32::MAX, u32::MAX - 12] {
        let mut frame = Frame::valid();
        frame.declared_meta_len = declared;
        assert_invalid_format(
            parse_recording_from_bytes(&frame.build()),
            "metadata length exceeds file size",
        );
    }
}

#[test]
fn event_payload_length_past_end_of_file_is_rejected() {
    let mut frame = Frame::valid();
    let payload = pack(&EventPayload::PaneClose { pane_id: 7 });
    frame.events = event_record(u32::MAX, &payload);
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "truncated event payload",
    );
}

#[test]
fn truncated_event_header_is_rejected() {
    // One event whose declared payload swallows all but the last 5 bytes of the
    // file (the payload decoder ignores trailing bytes), and a footer that claims
    // the seek index lies further on. The event loop then wants another record
    // header with only 5 bytes left, which is fewer than EVENT_HEADER_SIZE.
    let mut frame = Frame::valid();
    let payload = pack(&EventPayload::PaneClose { pane_id: 7 });
    // After the events come 8 bytes of entry_count and FOOTER_SIZE of footer.
    let tail = 8 + FOOTER_SIZE;
    let declared = u32::try_from(payload.len() + tail - (EVENT_HEADER_SIZE - 8)).expect("small");
    frame.events = event_record(declared, &payload);
    frame.declared_seek_offset = Some(u64::MAX);
    assert_invalid_format(
        parse_recording_from_bytes(&frame.build()),
        "truncated event header",
    );
}

#[test]
fn declared_msgpack_lengths_beyond_the_slice_are_rejected_without_allocating() {
    // Each blob declares a 2^32-1 element/byte container but supplies none of it.
    // rmp_serde reads str/bin as borrowed sub-slices and serde caps the
    // preallocation for arrays and maps, so these fail fast with a Deserialize
    // error; this pins that the declared length is never trusted.
    let declared: [(&str, &[u8]); 4] = [
        ("array32", &[0xdd, 0xff, 0xff, 0xff, 0xff]),
        ("map32", &[0xdf, 0xff, 0xff, 0xff, 0xff]),
        ("str32", &[0xdb, 0xff, 0xff, 0xff, 0xff]),
        ("bin32", &[0xc6, 0xff, 0xff, 0xff, 0xff]),
    ];

    for (name, blob) in declared {
        // In the metadata: a bare container instead of the metadata map.
        let mut frame = Frame::valid();
        frame.metadata = blob.to_vec();
        frame.declared_meta_len = u32::try_from(blob.len()).expect("small");
        assert!(
            matches!(
                parse_recording_from_bytes(&frame.build()),
                Err(ParseError::Deserialize(_))
            ),
            "metadata {name}"
        );

        // In an event payload: the externally-tagged `PtyOutput { data }` with `data`
        // replaced by the oversized declaration.
        let mut payload = vec![0x81]; // map(1)
        payload.extend_from_slice(&pack(&"PtyOutput"));
        payload.push(0x82); // map(2)
        payload.extend_from_slice(&pack(&"pane_id"));
        payload.push(0x00);
        payload.extend_from_slice(&pack(&"data"));
        payload.extend_from_slice(blob);

        let mut frame = Frame::valid();
        frame.events = event_record(u32::try_from(payload.len()).expect("small"), &payload);
        assert!(
            matches!(
                parse_recording_from_bytes(&frame.build()),
                Err(ParseError::Deserialize(_))
            ),
            "event payload {name}"
        );
    }
}

#[test]
fn committed_fuzz_regressions_do_not_panic() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fuzz")
        .join("regressions")
        .join("frec_parse");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("regressions dir exists") {
        let path = entry.expect("dir entry").path();
        if path.file_name().is_some_and(|n| n == ".gitkeep") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read reproducer");
        // The point is that this returns at all; the reproducers are all malformed.
        assert!(
            parse_recording_from_bytes(&bytes).is_err(),
            "{} should be rejected",
            path.display()
        );
        seen += 1;
    }
    assert!(seen > 0, "no reproducers found in {}", dir.display());
}
