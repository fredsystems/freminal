//! The eight OSC 99 standard icon names, and everything keyed on them.

/// One of kitty's eight OSC 99 standard icon names (`n=`).
///
/// This is the one table behind the standard names: the name parser, the
/// freedesktop icon name used on Linux/BSD, and the bundled glyph codepoint
/// that macOS and Windows draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StandardIcon {
    /// The `error` icon: a crossed circle.
    Error,
    /// The `warn` / `warning` icon: a warning triangle.
    Warning,
    /// The `info` icon: a circled "i".
    Info,
    /// The `question` icon: a circled "?".
    Question,
    /// The `help` icon: a life ring.
    Help,
    /// The `file-manager` icon: an open folder.
    FileManager,
    /// The `system-monitor` icon: a gauge.
    SystemMonitor,
    /// The `text-editor` icon: a text document.
    TextEditor,
}

impl StandardIcon {
    /// Every standard icon, for exhaustive iteration.
    // Follows its only callers (tests) until desktop_notify becomes a crate (150.10).
    #[cfg(test)]
    pub const ALL: [Self; 8] = [
        Self::Error,
        Self::Warning,
        Self::Info,
        Self::Question,
        Self::Help,
        Self::FileManager,
        Self::SystemMonitor,
        Self::TextEditor,
    ];

    /// The standard icon named `name`, or `None` when `name` is not one.
    ///
    /// The names are `error`, `warn` / `warning`, `info`, `question`, `help`,
    /// `file-manager`, `system-monitor` and `text-editor`. Matching is
    /// case-sensitive, as in kitty's own table.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            "question" => Some(Self::Question),
            "help" => Some(Self::Help),
            "file-manager" => Some(Self::FileManager),
            "system-monitor" => Some(Self::SystemMonitor),
            "text-editor" => Some(Self::TextEditor),
            _ => None,
        }
    }

    /// The freedesktop icon name for this icon.
    // Follows its callers (Linux/BSD presentation, tests) until desktop_notify becomes a crate (150.10).
    #[cfg(any(all(unix, not(target_os = "macos")), test))]
    #[must_use]
    pub const fn freedesktop_name(self) -> &'static str {
        match self {
            Self::Error => "dialog-error",
            Self::Warning => "dialog-warning",
            Self::Info => "dialog-information",
            Self::Question => "dialog-question",
            Self::Help => "system-help",
            Self::FileManager => "system-file-manager",
            Self::SystemMonitor => "utilities-system-monitor",
            Self::TextEditor => "utilities-text-editor",
        }
    }

    /// The Nerd Font codepoint of this icon's glyph in the bundled
    /// `CaskaydiaCove` face.
    // Follows its callers (macOS/Windows icon raster, tests) until desktop_notify becomes a crate (150.10).
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    #[must_use]
    pub const fn codepoint(self) -> char {
        match self {
            // nf-fa-times_circle
            Self::Error => '\u{f057}',
            // nf-fa-warning (exclamation triangle)
            Self::Warning => '\u{f071}',
            // nf-fa-info_circle
            Self::Info => '\u{f05a}',
            // nf-fa-question_circle
            Self::Question => '\u{f059}',
            // nf-fa-life_ring
            Self::Help => '\u{f1cd}',
            // nf-fa-folder_open
            Self::FileManager => '\u{f07c}',
            // nf-fa-tachometer
            Self::SystemMonitor => '\u{f0e4}',
            // nf-fa-file_text
            Self::TextEditor => '\u{f15c}',
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_name_parses() {
        for (name, icon) in [
            ("error", StandardIcon::Error),
            ("warn", StandardIcon::Warning),
            ("warning", StandardIcon::Warning),
            ("info", StandardIcon::Info),
            ("question", StandardIcon::Question),
            ("help", StandardIcon::Help),
            ("file-manager", StandardIcon::FileManager),
            ("system-monitor", StandardIcon::SystemMonitor),
            ("text-editor", StandardIcon::TextEditor),
        ] {
            assert_eq!(StandardIcon::from_name(name), Some(icon), "name {name:?}");
        }
    }

    #[test]
    fn non_names_do_not_parse() {
        for name in [
            "",
            "Error",
            "WARN",
            "Warning",
            "file_manager",
            "folder",
            "error ",
            "dialog-error",
        ] {
            assert_eq!(StandardIcon::from_name(name), None, "name {name:?}");
        }
    }

    #[test]
    fn every_icon_has_its_freedesktop_name() {
        for (icon, expected) in [
            (StandardIcon::Error, "dialog-error"),
            (StandardIcon::Warning, "dialog-warning"),
            (StandardIcon::Info, "dialog-information"),
            (StandardIcon::Question, "dialog-question"),
            (StandardIcon::Help, "system-help"),
            (StandardIcon::FileManager, "system-file-manager"),
            (StandardIcon::SystemMonitor, "utilities-system-monitor"),
            (StandardIcon::TextEditor, "utilities-text-editor"),
        ] {
            assert_eq!(icon.freedesktop_name(), expected, "{icon:?}");
        }
    }

    #[test]
    fn all_lists_every_icon_once_with_distinct_codepoints() {
        let mut codepoints: Vec<char> = StandardIcon::ALL.iter().map(|i| i.codepoint()).collect();
        codepoints.sort_unstable();
        codepoints.dedup();
        assert_eq!(codepoints.len(), 8);
    }
}
