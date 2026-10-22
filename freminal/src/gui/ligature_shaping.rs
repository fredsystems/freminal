// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Ligature shaping policy: the named mode and the OpenType feature lists
//! freminal shapes text with.

use freminal_common::config::FontConfig;

/// Named ligature-shaping mode: the single source of truth for the OpenType
/// feature set freminal shapes with.
///
/// Each variant selects one full, fixed feature list (`features`), and
/// the variant itself is part of the font manager's shape-plan cache key
/// (`PlanKey`). Because the plan cache is keyed on this value rather than on a
/// caller-supplied feature slice, the key fully determines the inputs the
/// cached `ShapePlan` is compiled from: two callers can never share a plan
/// compiled for a different feature set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LigatureShaping {
    /// `liga`/`calt` enabled.
    Enabled,
    /// `liga`/`calt` explicitly disabled.
    Disabled,
}

/// Feature list for [`LigatureShaping::Enabled`].
const FEATURES_LIGATURES_ENABLED: [harfrust::Feature; 4] = ligature_features(1);

/// Feature list for [`LigatureShaping::Disabled`].
const FEATURES_LIGATURES_DISABLED: [harfrust::Feature; 4] = ligature_features(0);

/// Build the feature list with `liga`/`calt` set to `lig_value`.
///
/// Struct literals (not `Feature::new`) so this can be `const`; `start = 0`
/// and `end = u32::MAX` are exactly what `Feature::new(tag, value, ..)`
/// produces for an unbounded range, i.e. a global feature.
const fn ligature_features(lig_value: u32) -> [harfrust::Feature; 4] {
    [
        // Enable kerning.
        harfrust::Feature {
            tag: harfrust::Tag::new(b"kern"),
            value: 1,
            start: 0,
            end: u32::MAX,
        },
        // Standard ligatures — controlled by config.
        harfrust::Feature {
            tag: harfrust::Tag::new(b"liga"),
            value: lig_value,
            start: 0,
            end: u32::MAX,
        },
        // Contextual alternates — controlled by config.
        harfrust::Feature {
            tag: harfrust::Tag::new(b"calt"),
            value: lig_value,
            start: 0,
            end: u32::MAX,
        },
        // Discretionary ligatures — always disabled (too aggressive for
        // terminal use; can be revisited later).
        harfrust::Feature {
            tag: harfrust::Tag::new(b"dlig"),
            value: 0,
            start: 0,
            end: u32::MAX,
        },
    ]
}

impl LigatureShaping {
    /// Classify the TOML `font.ligatures` toggle into the domain enum.
    ///
    /// This is the single place the config bool is turned into a
    /// [`LigatureShaping`]; everything downstream of config loading carries
    /// the enum, never the bool.
    #[must_use]
    pub const fn from_config(font: &FontConfig) -> Self {
        if font.ligatures {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    /// The OpenType features to shape with in this mode.
    ///
    /// `kern` is always enabled. `liga` and `calt` are enabled for
    /// [`Self::Enabled`] and explicitly disabled (value 0) for
    /// [`Self::Disabled`], to prevent ligature formation even in fonts that
    /// enable them by default. `dlig` is always disabled.
    #[must_use]
    pub(crate) const fn features(self) -> &'static [harfrust::Feature] {
        match self {
            Self::Enabled => &FEATURES_LIGATURES_ENABLED,
            Self::Disabled => &FEATURES_LIGATURES_DISABLED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LigatureShaping;

    #[test]
    fn ligature_mode_feature_lists_are_pinned() {
        let tuples = |mode: LigatureShaping| {
            mode.features()
                .iter()
                .map(|f| (f.tag, f.value, f.start, f.end))
                .collect::<Vec<_>>()
        };
        let tag = |t: &[u8; 4]| harfrust::Tag::new(t);

        assert_eq!(
            tuples(LigatureShaping::Enabled),
            [
                (tag(b"kern"), 1, 0, u32::MAX),
                (tag(b"liga"), 1, 0, u32::MAX),
                (tag(b"calt"), 1, 0, u32::MAX),
                (tag(b"dlig"), 0, 0, u32::MAX),
            ]
        );
        assert_eq!(
            tuples(LigatureShaping::Disabled),
            [
                (tag(b"kern"), 1, 0, u32::MAX),
                (tag(b"liga"), 0, 0, u32::MAX),
                (tag(b"calt"), 0, 0, u32::MAX),
                (tag(b"dlig"), 0, 0, u32::MAX),
            ]
        );
    }
}
