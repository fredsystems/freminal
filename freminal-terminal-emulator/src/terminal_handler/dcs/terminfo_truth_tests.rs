// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The table of truth: `res/freminal.ti` against what freminal actually does.
//!
//! The terminfo entry makes claims the code must back: XTGETTCAP answers
//! (`lookup_termcap` in `dcs.rs`) and key sequences (the input encoder).  These
//! tests parse the entry and fail when either side drifts.

use std::collections::BTreeMap;

use freminal_common::buffer_states::modes::{
    application_escape_key::ApplicationEscapeKey, decbkm::Decbkm, decckm::Decckm, lnm::Lnm,
    s8c1t::S8c1t,
};
use freminal_common::terminfo_source::{FREMINAL_TI, parse_terminfo_entry};

use super::TerminalHandler;
use crate::input::{KeyEventMeta, KeyModifiers, TerminalInput, TerminalInputPayload};

/// The `TERM` handed to `lookup_termcap` where the test is not about `TN`.
const TEST_TERM: &str = "xterm-freminal";

const DCS_SOURCE: &str = include_str!("../dcs.rs");

/// The terminfo capability a termcap alias names.
const TERMCAP_ALIASES: [(&str, &str); 4] = [
    ("Co", "colors"),
    ("ut", "bce"),
    ("SF", "indn"),
    ("SR", "rin"), // codespell:ignore rin
];

/// Names `lookup_termcap` answers that are not capabilities of the entry.
const XTGETTCAP_ONLY: [(&str, &str); 4] = [
    (
        "TN",
        "the terminal name is the value of $TERM, not a terminfo capability",
    ),
    (
        "RGB",
        "xterm's termcap flag; the terminfo string form is unverified, so the entry omits it",
    ),
    (
        "query-os-name",
        "kitty/foot extension, not a terminfo capability",
    ),
    (
        "query-os_name",
        "spelling alias of query-os-name, not a terminfo capability",
    ),
];

/// Capabilities that the entry declares and XTGETTCAP does not
/// answer, because no known client queries them over XTGETTCAP. Every
/// name starting with `k` (key capabilities) is covered by
/// `is_key_capability` instead of being listed here.
const TERMINFO_ONLY: &[&str] = &[
    "am", "ccc", "hs", "mc5i", "mir", "msgr", "npc", "xenl", "AX", "XF", "cols", "it", "lines",
    "pairs", "acsc", "bel", "blink", "bold", "cbt", "civis", "clear", "cnorm", "cr", "csr", "cub",
    "cub1", "cud", "cud1", "cuf", "cuf1", "cup", "cuu", "cuu1", "cvvis", "dch", "dch1", "dim",
    "dl", "dl1", "dsl", "ech", "ed", "el", "el1", "flash", "fsl", "home", "hpa", "ht", "hts",
    "ich", "il", "il1", "ind", "initc", "nel", "oc", "op", "rc", "rep", "rev", "ri", "ritm",
    "rmacs", "rmam", "rmcup", "rmir", "rmso", "rmul", "rs1", "sc", "setab", "setaf", "sgr", "sgr0",
    "sitm", "smacs", "smam", "smcup", "smir", "smso", "smul", "tbc", "tsl", "u6", "u7", "u8", "u9",
    "vpa", "BD", "BE", "Cr", "Cs", "E3", "PE", "PS", "Sync", "XM", "XR", "fd", "fe", "rmxx",
    "smxx",
];

/// Capabilities deliberately absent from the entry.
const NOT_IN_ENTRY: [(&str, &str); 11] = [
    ("invis", "SGR 8 conceal is parsed but not implemented"),
    ("km", "Alt sends no 8th bit and there is no ?1034"),
    ("ich1", "no reference entry declares it"),
    ("RGB", "string form unverified"),
    ("xm", "mouse report template unverified"),
    ("RV", "secondary DA reply template unverified"),
    ("is2", "initialisation string unverified"),
    ("rs2", "reset string unverified"),
    ("smglp", "DECSLRM argument handling unverified"),
    ("smglr", "DECSLRM argument handling unverified"),
    ("smgrp", "DECSLRM argument handling unverified"),
];

fn is_key_capability(name: &str) -> bool {
    name.starts_with('k')
}

/// The names `lookup_termcap` answers, read from its match arms so that
/// a newly added arm cannot escape the table-of-truth test.
///
/// The scan only understands one-line arms (`"a" | "b" => value,`). The match
/// body is checked line by line so an arm in any other shape fails loudly
/// instead of being skipped: a line holding `=>` must be a whole arm, and no
/// line may start a pattern without finishing it or continue one.
fn xtgettcap_answered_names() -> Vec<&'static str> {
    let Some(start) = DCS_SOURCE.find("fn lookup_termcap<'a>(name: &str") else {
        panic!("lookup_termcap not found");
    };
    let function = &DCS_SOURCE[start..];
    let Some(open) = function.find("match name {") else {
        panic!("lookup_termcap has no `match name {{`");
    };
    let body = &function[open + "match name {".len()..];
    let Some(end) = body.find("_ => None,") else {
        panic!("lookup_termcap has no fallback arm");
    };

    let mut names = Vec::new();
    for line in body[..end].lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let Some((patterns, _value)) = line.split_once("=>") else {
            panic!("match arm the scan does not understand (no `=>` on its line): {line:?}");
        };
        assert!(
            !line.ends_with("=>") && !line.ends_with('|'),
            "multi-line match arm the scan does not understand: {line:?}"
        );
        for pattern in patterns.split('|') {
            let pattern = pattern.trim();
            let Some(name) = pattern
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
            else {
                panic!("match arm pattern the scan does not understand: {pattern:?} in {line:?}");
            };
            assert!(
                !name.contains('"') && !name.is_empty(),
                "unsupported pattern {pattern:?} in {line:?}"
            );
            names.push(name);
        }
    }
    names
}

fn freminal_entry() -> BTreeMap<String, String> {
    parse_terminfo_entry(FREMINAL_TI, "xterm-freminal").1
}

#[test]
fn terminfo_header_names_are_a_phrase() {
    let (names, _) = parse_terminfo_entry(FREMINAL_TI, "xterm-freminal");
    assert_eq!(
        names.split('|').collect::<Vec<_>>(),
        ["xterm-freminal", "freminal", "Freminal terminal emulator"]
    );
}

#[test]
fn xtgettcap_answers_match_the_terminfo_entry() {
    let entry = freminal_entry();
    let answered = xtgettcap_answered_names();
    assert!(answered.len() > 20, "scan of lookup_termcap failed");

    for (name, reason) in XTGETTCAP_ONLY {
        assert!(
            answered.contains(&name),
            "{name} is listed as XTGETTCAP-only but is not answered ({reason})"
        );
        assert!(
            !entry.contains_key(name),
            "{name} is listed as XTGETTCAP-only but the entry declares it ({reason})"
        );
    }

    for name in answered {
        if XTGETTCAP_ONLY.iter().any(|(only, _)| *only == name) {
            continue;
        }
        let terminfo_name = TERMCAP_ALIASES
            .iter()
            .find(|(alias, _)| *alias == name)
            .map_or(name, |(_, terminfo)| *terminfo);
        assert_eq!(
            entry.get(terminfo_name).map(String::as_str),
            TerminalHandler::lookup_termcap(name, TEST_TERM),
            "XTGETTCAP {name} disagrees with {terminfo_name} in res/freminal.ti"
        );
    }
}

#[test]
fn terminfo_entry_capabilities_are_answered_identically_or_terminfo_only() {
    let entry = freminal_entry();

    for (name, value) in &entry {
        let mut candidates = vec![name.as_str()];
        candidates.extend(
            TERMCAP_ALIASES
                .iter()
                .filter(|(_, terminfo)| terminfo == name)
                .map(|(alias, _)| *alias),
        );
        let answers = candidates
            .iter()
            .filter_map(|candidate| TerminalHandler::lookup_termcap(candidate, TEST_TERM))
            .collect::<Vec<_>>();

        if answers.is_empty() {
            assert!(
                is_key_capability(name) || TERMINFO_ONLY.contains(&name.as_str()),
                "{name} is in res/freminal.ti but neither answered by XTGETTCAP nor \
                 listed in TERMINFO_ONLY"
            );
        } else {
            for answer in answers {
                assert_eq!(answer, value, "{name} in res/freminal.ti disagrees");
            }
        }
    }

    for name in TERMINFO_ONLY.iter().copied() {
        assert!(entry.contains_key(name), "stale TERMINFO_ONLY name {name}");
        assert_eq!(
            TerminalHandler::lookup_termcap(name, TEST_TERM),
            None,
            "{name} is answered by XTGETTCAP, so it is not terminfo-only"
        );
    }
}

#[test]
fn terminfo_entry_omits_unimplemented_and_unverified_capabilities() {
    let entry = freminal_entry();
    for (name, reason) in NOT_IN_ENTRY {
        assert!(!entry.contains_key(name), "{name} must stay out: {reason}");
    }

    let sgr = entry.get("sgr").map_or("", String::as_str);
    assert!(!sgr.contains("%p7"), "sgr must not set SGR 8: {sgr}");
    assert_eq!(
        entry.get("acsc").map(String::as_str),
        Some("``aaffggiijjkkllmmnnooppqqrrssttuuvvwwxxyyzz{{||}}~~"),
        "acsc must equal infocmp -x xterm-256color"
    );
}

/// Key capabilities the entry declares that the input encoder cannot send, so
/// the table-of-truth check leaves them unasserted. Each needs a reason; a
/// name that stops being declared, or that becomes checkable, fails the test.
const DECLARED_NOT_SENT: [(&str, &str); 2] = [
    (
        "kcbt",
        "Shift+Tab: the encoder has no back-tab input (`TerminalInput::Tab` carries no modifiers)",
    ),
    (
        "kmous",
        "not a key: the mouse-report prefix, produced by the mouse encoder rather than the \
         keyboard encoder",
    ),
];

/// The GUI source file holding the tests named in [`CHECKED_IN_GUI`].
const GUI_INPUT_SOURCE: &str = include_str!("../../../../freminal/src/gui/terminal/input.rs");

/// Key capabilities that are sent, but by an encoder on the GUI side that this
/// crate cannot reach (it must not depend on the `freminal` binary crate): the
/// capability, why, and the GUI test that compares the encoder's bytes with the
/// value parsed from `res/freminal.ti`. The test checks that the named test
/// still exists.
const CHECKED_IN_GUI: [(&str, &str, &str); 1] = [(
    "kent",
    "keypad Enter in application keypad mode (SS3 M) is encoded by the GUI's raw-key keypad \
     path, not by `TerminalInput::to_payload`",
    "keypad_enter_matches_terminfo_kent",
)];

/// The bytes the input encoder sends for `input`.
fn encoded(input: &TerminalInput, decckm: Decckm) -> Vec<u8> {
    match input.to_payload(
        decckm,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsDel,
        Lnm::LineFeed,
        0,
        S8c1t::SevenBit,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// Modifiers for the xterm parameter `n` (`n = 1 + shift + 2 * alt + 4 * ctrl`).
fn modifiers_for_param(n: u8) -> KeyModifiers {
    let bits = n - 1;
    KeyModifiers {
        shift: bits & 1 != 0,
        alt: bits & 2 != 0,
        ctrl: bits & 4 != 0,
        ..KeyModifiers::NONE
    }
}

/// Capability name to the bytes the encoder sends for it.
type KeyCapabilities = BTreeMap<String, Vec<u8>>;

fn expect(expected: &mut KeyCapabilities, name: &str, input: &TerminalInput, decckm: Decckm) {
    let previous = expected.insert(name.to_string(), encoded(input, decckm));
    assert!(previous.is_none(), "{name} is checked twice");
}

/// The modified editing and cursor keys: the bare `kXXX` name is Shift (param 2)
/// and `kXXX3` to `kXXX7` are params 3 to 7.  Shift+Down and Shift+Up are also
/// the scroll-forward (`kind`) and scroll-reverse (`kri`) keys.
fn add_modified_families(expected: &mut KeyCapabilities) {
    type Key = fn(KeyModifiers) -> TerminalInput;
    let families: [(&str, Key); 10] = [
        ("kDC", TerminalInput::Delete),
        ("kDN", TerminalInput::ArrowDown),
        ("kEND", TerminalInput::End),
        ("kHOM", TerminalInput::Home),
        ("kIC", TerminalInput::Insert),
        ("kLFT", TerminalInput::ArrowLeft),
        ("kNXT", TerminalInput::PageDown),
        ("kPRV", TerminalInput::PageUp),
        ("kRIT", TerminalInput::ArrowRight),
        ("kUP", TerminalInput::ArrowUp),
    ];
    for (prefix, key) in families {
        for n in 2_u8..=7 {
            let name = if n == 2 {
                prefix.to_string()
            } else {
                format!("{prefix}{n}")
            };
            let input = key(modifiers_for_param(n));
            expect(expected, &name, &input, Decckm::Ansi);
        }
    }

    let shift = modifiers_for_param(2);
    for (name, input) in [
        ("kind", TerminalInput::ArrowDown(shift)),
        ("kri", TerminalInput::ArrowUp(shift)),
    ] {
        expect(expected, name, &input, Decckm::Ansi);
    }
}

/// The unmodified editing keys, backspace, focus reports, and the cursor and
/// home/end keys.  The latter are declared as the application-mode (`smkx`)
/// sequences, so the encoder is asked in application cursor mode.
fn add_plain_keys(expected: &mut KeyCapabilities) {
    let none = KeyModifiers::NONE;
    for (name, input) in [
        ("kich1", TerminalInput::Insert(none)),
        ("kdch1", TerminalInput::Delete(none)),
        ("kpp", TerminalInput::PageUp(none)),
        ("knp", TerminalInput::PageDown(none)),
        ("kbs", TerminalInput::Backspace),
        ("kxIN", TerminalInput::InFocus),
        ("kxOUT", TerminalInput::LostFocus),
    ] {
        expect(expected, name, &input, Decckm::Ansi);
    }
    for (name, input) in [
        ("khome", TerminalInput::Home(none)),
        ("kend", TerminalInput::End(none)),
        ("kcuu1", TerminalInput::ArrowUp(none)),
        ("kcud1", TerminalInput::ArrowDown(none)),
        ("kcuf1", TerminalInput::ArrowRight(none)),
        ("kcub1", TerminalInput::ArrowLeft(none)),
    ] {
        expect(expected, name, &input, Decckm::Application);
    }
}

/// `kf1` to `kf12` are the unmodified F1 to F12.  `kf13` to `kf63` follow the
/// terminfo convention: twelve keys per modifier group, in the order Shift,
/// Ctrl, Ctrl+Shift, Alt, then Shift+Alt for the first three.
fn add_function_keys(expected: &mut KeyCapabilities) {
    for number in 1_u8..=12 {
        expect(
            expected,
            &format!("kf{number}"),
            &TerminalInput::FunctionKey(number, KeyModifiers::NONE),
            Decckm::Ansi,
        );
    }

    let group_params = [2_u8, 5, 6, 3, 4];
    for number in 13_u8..=63 {
        let group = usize::from((number - 1) / 12 - 1);
        let function_key = (number - 1) % 12 + 1;
        expect(
            expected,
            &format!("kf{number}"),
            &TerminalInput::FunctionKey(function_key, modifiers_for_param(group_params[group])),
            Decckm::Ansi,
        );
    }
}

/// Every `k*` capability whose value the encoder can produce, with the bytes it
/// sends.
fn encoder_key_capabilities() -> KeyCapabilities {
    let mut expected = BTreeMap::new();
    add_modified_families(&mut expected);
    add_plain_keys(&mut expected);
    add_function_keys(&mut expected);
    expected
}

#[test]
fn every_key_capability_in_the_entry_is_checked_here_declared_not_sent_or_checked_in_gui() {
    let entry = freminal_entry();
    let expected = encoder_key_capabilities();

    assert!(
        entry
            .get("smkx")
            .is_some_and(|smkx| smkx.contains("\x1b[?1h")),
        "the application-mode key values assume smkx enters application cursor mode"
    );

    for (name, bytes) in &expected {
        assert_eq!(
            entry.get(name).map(String::as_bytes),
            Some(bytes.as_slice()),
            "{name} in res/freminal.ti differs from what the encoder sends"
        );
    }

    for (name, reason) in DECLARED_NOT_SENT {
        assert!(
            entry.contains_key(name),
            "{name} is listed as declared-not-sent but the entry does not declare it ({reason})"
        );
        assert!(
            !expected.contains_key(name),
            "{name} is checked against the encoder, so it is not declared-not-sent ({reason})"
        );
    }

    for (name, reason, gui_test) in CHECKED_IN_GUI {
        assert!(
            entry.contains_key(name),
            "{name} is listed as checked-in-GUI but the entry does not declare it ({reason})"
        );
        assert!(
            !expected.contains_key(name),
            "{name} is checked against the encoder here, so it is not checked-in-GUI ({reason})"
        );
        assert!(
            !DECLARED_NOT_SENT
                .iter()
                .any(|(declared, _)| *declared == name),
            "{name} is in both DECLARED_NOT_SENT and CHECKED_IN_GUI ({reason})"
        );
        assert!(
            GUI_INPUT_SOURCE.contains(&format!("fn {gui_test}()")),
            "{name} is checked in the GUI by `{gui_test}`, which no longer exists"
        );
    }

    for name in entry.keys().filter(|name| is_key_capability(name)) {
        let sets = [
            expected.contains_key(name.as_str()),
            DECLARED_NOT_SENT
                .iter()
                .any(|(declared, _)| declared == name),
            CHECKED_IN_GUI.iter().any(|(checked, _, _)| checked == name),
        ];
        assert_eq!(
            sets.iter().filter(|member| **member).count(),
            1,
            "{name} is a key capability in res/freminal.ti that must be in exactly one of: \
             checked against the encoder here, DECLARED_NOT_SENT, CHECKED_IN_GUI \
             (checked here, declared-not-sent, checked in GUI = {sets:?})"
        );
    }
}

#[test]
fn the_function_key_group_arithmetic_matches_the_documented_convention() {
    // Spot checks of the terminfo kf13..kf63 layout the test relies on.
    let expected = encoder_key_capabilities();
    let sends = |name: &str| expected.get(name).map(Vec::as_slice);
    assert_eq!(sends("kf13"), Some(b"\x1b[1;2P".as_slice())); // Shift+F1
    assert_eq!(sends("kf24"), Some(b"\x1b[24;2~".as_slice())); // Shift+F12
    assert_eq!(sends("kf25"), Some(b"\x1b[1;5P".as_slice())); // Ctrl+F1
    assert_eq!(sends("kf37"), Some(b"\x1b[1;6P".as_slice())); // Ctrl+Shift+F1
    assert_eq!(sends("kf49"), Some(b"\x1b[1;3P".as_slice())); // Alt+F1
    assert_eq!(sends("kf61"), Some(b"\x1b[1;4P".as_slice())); // Shift+Alt+F1
    assert_eq!(sends("kf63"), Some(b"\x1b[1;4R".as_slice())); // Shift+Alt+F3
    assert_eq!(sends("kf64"), None);
}
