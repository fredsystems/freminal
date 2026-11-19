// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parsing of OSC 22, the kitty pointer-shape sequence.
//!
//! See <https://sw.kovidgoyal.net/kitty/pointer-shapes/>.  The parser works on
//! the raw OSC body (`22[;body]`, terminator already stripped), not through the
//! generic tokenizer, because the body is a comma-separated name list that is
//! split on bytes.
//!
//! The grammar follows kitty's reference implementation:
//!
//! - The body is everything after the first `;`; a body with no `;` is empty.
//! - If its first byte is `=`, `>`, `<` or `?`, that byte selects the
//!   operation (set, push, pop, query) and is stripped.  Otherwise the
//!   operation is a set and the byte belongs to the first name.
//! - The rest is split on `,`.  Names are neither trimmed nor case-folded, and
//!   a name that is not valid UTF-8 is an unknown name.
//! - At most [`MAX_OSC22_NAMES`] segments (empty ones included) are read; the
//!   others are ignored for every operation.
//!
//! Unknown names are dropped one at a time and logged at `debug!` by length
//! only, never by content.  Exactly one [`PointerShapeOp`] is emitted per
//! sequence.  A set or push whose names were all dropped is still emitted, with
//! an empty list, which the handler treats as a no-op.

use freminal_common::buffer_states::osc::AnsiOscType;
use freminal_common::buffer_states::pointer_shape::{
    MAX_OSC22_NAMES, PointerShape, PointerShapeEntry, PointerShapeOp, PointerShapeQuery,
};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// The query item that asks for the current shape.
const CURRENT_NAME: &[u8] = b"__current__";
/// The query item that asks for the default shape.
const DEFAULT_NAME: &[u8] = b"__default__";
/// The query item that asks for the grabbed shape.
const GRABBED_NAME: &[u8] = b"__grabbed__";

/// Parse an OSC 22 body and emit its operation.
pub(super) fn handle_osc_pointer_shape(raw_params: &[u8], output: &mut Vec<TerminalOutput>) {
    let body = raw_params
        .iter()
        .position(|b| *b == b';')
        .and_then(|sep| raw_params.get(sep + 1..))
        .unwrap_or_default();
    output.push(TerminalOutput::OscResponse(AnsiOscType::PointerShape(
        parse_body(body),
    )));
}

/// Parse the part of an OSC 22 sequence after the first `;`.
fn parse_body(body: &[u8]) -> PointerShapeOp {
    let (prefix, names) = match body.split_first() {
        Some((first @ (b'=' | b'>' | b'<' | b'?'), rest)) => (Some(*first), rest),
        _ => (None, body),
    };
    match prefix {
        Some(b'<') => PointerShapeOp::Pop,
        Some(b'>') => PointerShapeOp::Push(
            bounded_segments(names)
                .iter()
                .filter_map(|s| known_shape(s))
                .collect(),
        ),
        Some(b'?') => PointerShapeOp::Query(
            bounded_segments(names)
                .iter()
                .map(|s| classify_query(s))
                .collect(),
        ),
        _ => PointerShapeOp::Set(
            bounded_segments(names)
                .iter()
                .filter_map(|s| set_entry(s))
                .collect(),
        ),
    }
}

/// The first [`MAX_OSC22_NAMES`] comma-separated segments of `names`.
///
/// An empty `names` is one empty segment.  Segments past the cap are ignored;
/// whether any exist is logged once at `debug!`, without a count (counting
/// would scan the whole body).
fn bounded_segments(names: &[u8]) -> Vec<&[u8]> {
    let mut iter = names.split(|b| *b == b',');
    let segments: Vec<&[u8]> = iter.by_ref().take(MAX_OSC22_NAMES).collect();
    if iter.next().is_some() {
        tracing::debug!("OSC 22: names past the {MAX_OSC22_NAMES}-name cap ignored");
    }
    segments
}

/// The shape a segment names, or `None` for an unknown (or empty) name.
fn lookup(segment: &[u8]) -> Option<PointerShape> {
    std::str::from_utf8(segment)
        .ok()
        .and_then(PointerShape::from_name)
}

/// The shape a push segment names.  Empty and unknown segments are dropped;
/// an unknown one is logged by length.
fn known_shape(segment: &[u8]) -> Option<PointerShape> {
    let shape = lookup(segment);
    if shape.is_none() && !segment.is_empty() {
        log_ignored(segment);
    }
    shape
}

/// The entry a set segment stores: [`PointerShapeEntry::Unset`] for an empty
/// segment, the shape for a known name, and nothing for an unknown one.
fn set_entry(segment: &[u8]) -> Option<PointerShapeEntry> {
    if segment.is_empty() {
        return Some(PointerShapeEntry::Unset);
    }
    known_shape(segment).map(PointerShapeEntry::Shape)
}

/// Classify one query item.
fn classify_query(segment: &[u8]) -> PointerShapeQuery {
    if segment == CURRENT_NAME {
        PointerShapeQuery::Current
    } else if segment == DEFAULT_NAME {
        PointerShapeQuery::Default
    } else if segment == GRABBED_NAME {
        PointerShapeQuery::Grabbed
    } else if lookup(segment).is_some() {
        PointerShapeQuery::Supported
    } else {
        PointerShapeQuery::Unsupported
    }
}

/// Log an ignored name by its length only.
fn log_ignored(segment: &[u8]) {
    tracing::debug!(
        "OSC 22: unknown pointer shape name ignored: length {}",
        segment.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use PointerShapeEntry::{Shape, Unset};
    use PointerShapeQuery::{Current, Default as DefaultShape, Grabbed, Supported, Unsupported};

    fn parse(raw: &[u8]) -> PointerShapeOp {
        let mut output = Vec::new();
        handle_osc_pointer_shape(raw, &mut output);
        assert_eq!(output.len(), 1, "exactly one output: {output:?}");
        match output.pop() {
            Some(TerminalOutput::OscResponse(AnsiOscType::PointerShape(op))) => op,
            other => panic!("unexpected output: {other:?}"),
        }
    }

    fn names(prefix: &str, count: usize, name: &str) -> Vec<u8> {
        let mut raw = format!("22;{prefix}").into_bytes();
        for i in 0..count {
            if i > 0 {
                raw.push(b',');
            }
            raw.extend_from_slice(name.as_bytes());
        }
        raw
    }

    // ── set ─────────────────────────────────────────────────────────

    #[test]
    fn no_prefix_is_a_set() {
        assert_eq!(
            parse(b"22;text"),
            PointerShapeOp::Set(vec![Shape(PointerShape::Text)])
        );
    }

    #[test]
    fn equals_prefix_is_a_set() {
        assert_eq!(
            parse(b"22;=pointer"),
            PointerShapeOp::Set(vec![Shape(PointerShape::Pointer)])
        );
    }

    #[test]
    fn set_list_applies_in_order() {
        assert_eq!(
            parse(b"22;text,pointer"),
            PointerShapeOp::Set(vec![
                Shape(PointerShape::Text),
                Shape(PointerShape::Pointer)
            ])
        );
    }

    #[test]
    fn bare_equals_sets_unset() {
        assert_eq!(parse(b"22;="), PointerShapeOp::Set(vec![Unset]));
    }

    #[test]
    fn empty_body_sets_unset() {
        assert_eq!(parse(b"22;"), PointerShapeOp::Set(vec![Unset]));
    }

    #[test]
    fn no_separator_sets_unset() {
        assert_eq!(parse(b"22"), PointerShapeOp::Set(vec![Unset]));
    }

    #[test]
    fn empty_middle_name_is_kept_as_unset() {
        // `a` and `b` are unknown names; only the empty one survives.
        assert_eq!(parse(b"22;=a,,b"), PointerShapeOp::Set(vec![Unset]));
        assert_eq!(
            parse(b"22;=text,,pointer"),
            PointerShapeOp::Set(vec![
                Shape(PointerShape::Text),
                Unset,
                Shape(PointerShape::Pointer)
            ])
        );
    }

    #[test]
    fn name_containing_semicolon_is_one_unknown_name() {
        assert_eq!(parse(b"22;text;x"), PointerShapeOp::Set(vec![]));
        assert_eq!(
            parse(b"22;=text;x,pointer"),
            PointerShapeOp::Set(vec![Shape(PointerShape::Pointer)])
        );
    }

    #[test]
    fn leading_space_is_unknown() {
        assert_eq!(parse(b"22; text"), PointerShapeOp::Set(vec![]));
        assert_eq!(parse(b"22;=text "), PointerShapeOp::Set(vec![]));
    }

    #[test]
    fn uppercase_is_unknown() {
        assert_eq!(parse(b"22;TEXT"), PointerShapeOp::Set(vec![]));
        assert_eq!(parse(b"22;=Pointer"), PointerShapeOp::Set(vec![]));
    }

    #[test]
    fn unknown_names_are_dropped_from_a_set() {
        assert_eq!(
            parse(b"22;text,banana,pointer"),
            PointerShapeOp::Set(vec![
                Shape(PointerShape::Text),
                Shape(PointerShape::Pointer)
            ])
        );
        assert_eq!(parse(b"22;banana"), PointerShapeOp::Set(vec![]));
    }

    #[test]
    fn aliases_resolve_in_a_set() {
        assert_eq!(
            parse(b"22;fleur"),
            PointerShapeOp::Set(vec![Shape(PointerShape::Move)])
        );
    }

    #[test]
    fn non_utf8_name_is_unknown() {
        assert_eq!(
            parse(b"22;\xff\xfe,text"),
            PointerShapeOp::Set(vec![Shape(PointerShape::Text)])
        );
        assert_eq!(parse(b"22;>\xffext"), PointerShapeOp::Push(vec![]));
        assert_eq!(parse(b"22;?\xff"), PointerShapeOp::Query(vec![Unsupported]));
    }

    // ── push ────────────────────────────────────────────────────────

    #[test]
    fn push_collects_known_shapes_in_order() {
        assert_eq!(
            parse(b"22;>wait,help"),
            PointerShapeOp::Push(vec![PointerShape::Wait, PointerShape::Help])
        );
    }

    #[test]
    fn push_drops_empty_and_unknown_names() {
        assert_eq!(
            parse(b"22;>wait,,banana,help"),
            PointerShapeOp::Push(vec![PointerShape::Wait, PointerShape::Help])
        );
    }

    #[test]
    fn bare_push_is_empty() {
        assert_eq!(parse(b"22;>"), PointerShapeOp::Push(vec![]));
    }

    // ── pop ─────────────────────────────────────────────────────────

    #[test]
    fn pop_is_a_pop() {
        assert_eq!(parse(b"22;<"), PointerShapeOp::Pop);
    }

    #[test]
    fn pop_ignores_names() {
        assert_eq!(parse(b"22;<pointer"), PointerShapeOp::Pop);
        assert_eq!(parse(b"22;<a,b,\xff"), PointerShapeOp::Pop);
    }

    // ── query ───────────────────────────────────────────────────────

    #[test]
    fn query_classifies_special_names() {
        assert_eq!(
            parse(b"22;?__current__,__default__,__grabbed__"),
            PointerShapeOp::Query(vec![Current, DefaultShape, Grabbed])
        );
    }

    #[test]
    fn query_classifies_support() {
        assert_eq!(
            parse(b"22;?pointer,crosshair,no-such-name,wait"),
            PointerShapeOp::Query(vec![Supported, Supported, Unsupported, Supported])
        );
    }

    #[test]
    fn query_aliases_are_supported() {
        assert_eq!(
            parse(b"22;?left_ptr,col-resize,row-resize"),
            PointerShapeOp::Query(vec![Supported, Supported, Supported])
        );
    }

    #[test]
    fn query_removed_extras_are_unsupported() {
        assert_eq!(
            parse(b"22;?none,context-menu,all-scroll"),
            PointerShapeOp::Query(vec![Unsupported, Unsupported, Unsupported])
        );
        assert_eq!(
            parse(b"22;none,context-menu,all-scroll"),
            PointerShapeOp::Set(vec![])
        );
        assert_eq!(
            parse(b"22;>none,context-menu,all-scroll"),
            PointerShapeOp::Push(vec![])
        );
    }

    #[test]
    fn query_empty_name_is_unsupported() {
        assert_eq!(parse(b"22;?"), PointerShapeOp::Query(vec![Unsupported]));
        assert_eq!(
            parse(b"22;?text,,text"),
            PointerShapeOp::Query(vec![Supported, Unsupported, Supported])
        );
    }

    #[test]
    fn query_special_names_are_case_sensitive() {
        assert_eq!(
            parse(b"22;?__CURRENT__,__current__ "),
            PointerShapeOp::Query(vec![Unsupported, Unsupported])
        );
    }

    // ── cap ─────────────────────────────────────────────────────────

    #[test]
    fn push_honours_the_name_cap() {
        let raw = names(">", MAX_OSC22_NAMES, "text");
        let mut over = raw.clone();
        over.extend_from_slice(b",wait");
        let expected = vec![PointerShape::Text; MAX_OSC22_NAMES];
        assert_eq!(parse(&raw), PointerShapeOp::Push(expected.clone()));
        assert_eq!(parse(&over), PointerShapeOp::Push(expected));
    }

    #[test]
    fn query_honours_the_name_cap() {
        let raw = names("?", MAX_OSC22_NAMES, "text");
        let mut over = raw.clone();
        over.extend_from_slice(b",banana,banana");
        assert_eq!(
            parse(&raw),
            PointerShapeOp::Query(vec![Supported; MAX_OSC22_NAMES])
        );
        assert_eq!(
            parse(&over),
            PointerShapeOp::Query(vec![Supported; MAX_OSC22_NAMES]),
            "names past the cap produce no items"
        );
    }

    #[test]
    fn set_honours_the_name_cap() {
        let mut over = names("", MAX_OSC22_NAMES, "text");
        over.extend_from_slice(b",wait");
        assert_eq!(
            parse(&over),
            PointerShapeOp::Set(vec![Shape(PointerShape::Text); MAX_OSC22_NAMES])
        );
    }

    #[test]
    fn empty_segments_count_towards_the_cap() {
        let mut raw = b"22;>".to_vec();
        raw.extend(std::iter::repeat_n(b',', MAX_OSC22_NAMES));
        raw.extend_from_slice(b"text");
        // 64 empty segments fill the cap, so `text` (the 65th) is ignored.
        assert_eq!(parse(&raw), PointerShapeOp::Push(vec![]));
        let mut ok = b"22;>".to_vec();
        ok.extend(std::iter::repeat_n(b',', MAX_OSC22_NAMES - 1));
        ok.extend_from_slice(b"text");
        assert_eq!(parse(&ok), PointerShapeOp::Push(vec![PointerShape::Text]));
    }

    // ── logging ─────────────────────────────────────────────────────

    #[test]
    fn ignored_names_are_never_logged_by_content() {
        const SECRET: &str = "s3cr3t-pointer-name";
        let events = crate::log_capture::capture(|| {
            let _ = parse(format!("22;text,{SECRET}").as_bytes());
            let _ = parse(format!("22;>{SECRET}").as_bytes());
            let _ = parse(format!("22;?{SECRET}").as_bytes());
        });
        assert!(
            crate::log_capture::warnings(&events).is_empty(),
            "ignoring a name must not warn: {events:?}"
        );
        assert!(
            events.iter().all(|(_, text)| !text.contains(SECRET)),
            "a log line leaked the name: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|(level, text)| *level == tracing::Level::DEBUG
                    && text.contains(&format!("length {}", SECRET.len()))),
            "ignored name should be logged by length at debug: {events:?}"
        );
    }

    #[test]
    fn names_past_the_cap_are_logged_once_without_a_count() {
        let events = crate::log_capture::capture(|| {
            let _ = parse(&names(">", MAX_OSC22_NAMES + 5, "text"));
            let _ = parse(&names(">", MAX_OSC22_NAMES, "text"));
        });
        let capped: Vec<_> = events
            .iter()
            .filter(|(_, text)| text.contains("cap ignored"))
            .collect();
        assert_eq!(
            capped.len(),
            1,
            "only the over-cap request logs: {events:?}"
        );
        assert_eq!(capped[0].0, tracing::Level::DEBUG);
        assert!(
            !capped[0].1.contains('5'),
            "the log must not carry a count: {capped:?}"
        );
    }
}
