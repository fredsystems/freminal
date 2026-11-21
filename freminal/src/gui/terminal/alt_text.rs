// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Alt as the `ESC` prefix key for typed text.
//!
//! Whether `Event::Text` becomes `ESC` followed by the text is decided here,
//! from pure inputs: the held modifiers, the host platform and `?1036`. The
//! platform is a parameter, not a `cfg`, so every platform's rule is tested on
//! every platform.
//!
//! macOS Option is never the prefix: it composes characters, and the text it
//! composes is sent as typed. Making Option act as Alt needs the key without
//! its modifiers, which does not reach this code yet.

use std::borrow::Cow;

use egui::Modifiers;
use freminal_common::buffer_states::modes::meta_sends_escape::MetaSendsEscape;
use freminal_terminal_emulator::input::{TerminalInput, collect_alt_text, collect_text};

/// The platform whose Alt / Option rules apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostPlatform {
    MacOs,
    Other,
}

impl HostPlatform {
    /// The platform this binary runs on.
    pub(super) const CURRENT: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else {
        Self::Other
    };
}

/// What the held modifiers make of the text typed with them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextModifier {
    /// Alt is held and acts as the prefix key.
    AltPrefix,
    /// The text is typed as it is: no Alt, or not Alt as far as text goes.
    Plain,
}

/// Classify `mods` for typed text.
///
/// Ctrl+Alt is never a prefix: on Windows `AltGr` arrives as Ctrl+Alt and the
/// text it types must reach the application as typed. (On Linux `AltGr` is not
/// Alt at all.) On macOS Option is never Alt: it composes characters.
const fn text_modifier(mods: Modifiers, platform: HostPlatform) -> TextModifier {
    if !mods.alt || mods.ctrl {
        return TextModifier::Plain;
    }
    match platform {
        HostPlatform::MacOs => TextModifier::Plain,
        HostPlatform::Other => TextModifier::AltPrefix,
    }
}

/// The inputs for text typed with `mods` held.
///
/// The Alt prefix is built here, as a [`TerminalInput::AltText`], when Alt
/// applies and `?1036` is set; otherwise the text is collected as plain
/// characters, exactly as it was before Alt was considered. The encoder drops
/// the prefix again while a kitty keyboard flag is active.
pub(super) fn text_inputs(
    text: &str,
    mods: Modifiers,
    platform: HostPlatform,
    meta_sends_escape: MetaSendsEscape,
) -> Cow<'static, [TerminalInput]> {
    match (text_modifier(mods, platform), meta_sends_escape) {
        (TextModifier::AltPrefix, MetaSendsEscape::Set) => collect_alt_text(text),
        _ => collect_text(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::modes::{
        application_escape_key::ApplicationEscapeKey, decbkm::Decbkm, decckm::Decckm, lnm::Lnm,
        s8c1t::S8c1t,
    };
    use freminal_terminal_emulator::input::{KeyEventMeta, TerminalInputPayload};

    const ALT: Modifiers = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    const CTRL_ALT: Modifiers = Modifiers {
        alt: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    const SHIFT_ALT: Modifiers = Modifiers {
        alt: true,
        shift: true,
        ..Modifiers::NONE
    };

    /// The bytes sent for `inputs` with the given kitty keyboard flags.
    fn encode(inputs: &[TerminalInput], kitty_flags: u32) -> Vec<u8> {
        inputs
            .iter()
            .flat_map(|input| {
                match input.to_payload(
                    Decckm::Ansi,
                    0,
                    ApplicationEscapeKey::Reset,
                    Decbkm::BackarrowSendsDel,
                    Lnm::LineFeed,
                    kitty_flags,
                    S8c1t::SevenBit,
                    &KeyEventMeta::PRESS,
                ) {
                    TerminalInputPayload::Single(b) => vec![b],
                    TerminalInputPayload::Many(bs) => bs.to_vec(),
                    TerminalInputPayload::Owned(bs) => bs,
                }
            })
            .collect()
    }

    fn typed(
        text: &str,
        mods: Modifiers,
        platform: HostPlatform,
        meta: MetaSendsEscape,
        kitty_flags: u32,
    ) -> Vec<u8> {
        encode(&text_inputs(text, mods, platform, meta), kitty_flags)
    }

    #[test]
    fn alt_text_gets_an_escape_prefix_while_1036_is_set() {
        assert_eq!(
            typed("b", ALT, HostPlatform::Other, MetaSendsEscape::Set, 0),
            b"\x1bb"
        );
    }

    #[test]
    fn alt_text_is_sent_alone_while_1036_is_reset() {
        assert_eq!(
            typed("b", ALT, HostPlatform::Other, MetaSendsEscape::Reset, 0),
            b"b"
        );
    }

    #[test]
    fn text_without_alt_is_never_prefixed() {
        for meta in [MetaSendsEscape::Set, MetaSendsEscape::Reset] {
            assert_eq!(
                typed("b", Modifiers::NONE, HostPlatform::Other, meta, 0),
                b"b"
            );
        }
    }

    #[test]
    fn shifted_alt_text_keeps_its_case_after_the_prefix() {
        assert_eq!(
            typed("B", SHIFT_ALT, HostPlatform::Other, MetaSendsEscape::Set, 0),
            b"\x1bB"
        );
    }

    #[test]
    fn multi_byte_alt_text_gets_one_prefix() {
        let text = "é\u{1f600}";
        let mut expected = vec![0x1b];
        expected.extend_from_slice(text.as_bytes());
        assert_eq!(
            typed(text, ALT, HostPlatform::Other, MetaSendsEscape::Set, 0),
            expected
        );
    }

    #[test]
    fn alt_prefix_is_one_input_so_it_is_not_repeated_per_byte() {
        let inputs = text_inputs("ab", ALT, HostPlatform::Other, MetaSendsEscape::Set);
        assert_eq!(inputs.len(), 1);
    }

    #[test]
    fn empty_alt_text_sends_nothing() {
        assert!(text_inputs("", ALT, HostPlatform::Other, MetaSendsEscape::Set).is_empty());
    }

    #[test]
    fn any_kitty_flag_leaves_alt_text_unchanged() {
        // 1 = disambiguate, 2 = event types, 8 = report all keys. Each must
        // encode `Alt+text` exactly as the same text typed without Alt.
        for flags in [1_u32, 2, 4, 8, 16, 1 | 2 | 8, 31] {
            for text in ["b", "é", "ab"] {
                let with_alt = typed(text, ALT, HostPlatform::Other, MetaSendsEscape::Set, flags);
                let without_alt = typed(
                    text,
                    Modifiers::NONE,
                    HostPlatform::Other,
                    MetaSendsEscape::Set,
                    flags,
                );
                assert_eq!(with_alt, without_alt, "text {text:?}, kitty flags {flags}");
                assert!(
                    !with_alt.starts_with(b"\x1b") || without_alt.starts_with(b"\x1b"),
                    "no Alt prefix may appear, text {text:?}, kitty flags {flags}"
                );
            }
        }
    }

    #[test]
    fn altgr_text_is_never_prefixed() {
        // Windows reports AltGr as Ctrl+Alt; the text it types goes through
        // as typed, whatever the mode.
        for platform in [HostPlatform::Other, HostPlatform::MacOs] {
            for meta in [MetaSendsEscape::Set, MetaSendsEscape::Reset] {
                assert_eq!(
                    typed("@", CTRL_ALT, platform, meta, 0),
                    b"@",
                    "{platform:?} {meta:?}"
                );
            }
        }
    }

    #[test]
    fn option_text_on_macos_is_plain_whatever_1036_says() {
        // Option composes characters on macOS; the composed text is sent as
        // typed, with no prefix, in either `?1036` state.
        for meta in [MetaSendsEscape::Set, MetaSendsEscape::Reset] {
            for text in ["\u{222b}", "b"] {
                assert_eq!(
                    typed(text, ALT, HostPlatform::MacOs, meta, 0),
                    text.as_bytes(),
                    "{text:?} {meta:?}"
                );
            }
        }
    }

    #[test]
    fn alt_text_off_macos_follows_1036() {
        assert_eq!(
            typed("b", ALT, HostPlatform::Other, MetaSendsEscape::Set, 0),
            b"\x1bb"
        );
        assert_eq!(
            typed("b", ALT, HostPlatform::Other, MetaSendsEscape::Reset, 0),
            b"b"
        );
    }
}
