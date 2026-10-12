//! User filtering of OSC 99 notifications (`[notifications] osc99_filters`).
//!
//! Each configured rule is either the bare word `all` or `location:regex`,
//! where `location` is `title`, `body`, `app` (the `f=` application name) or
//! `type` (any `t=` value). Matching is a case-sensitive regex *search*, as in
//! kitty: use `(?i)` inside the pattern for case-insensitive matching. A
//! notification that matches any rule is dropped silently, with no reply.
//!
//! [`Osc99Filter::compile`] never fails as a whole: an invalid rule is
//! reported in the returned error list and skipped, and the remaining rules
//! still apply. The config text is user-controlled, so each regex is built
//! with a size limit.

use std::fmt;

use freminal_common::buffer_states::window_manipulation::Notification99Data;
use regex::{Regex, RegexBuilder};

/// The compiled-regex size limit, in bytes. Bounds the memory a hostile or
/// careless `osc99_filters` entry can make the regex compiler allocate.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// One compiled filter rule.
#[derive(Debug, Clone)]
pub enum Osc99FilterRule {
    /// Matches every notification.
    All,
    /// Matches when the title contains a match.
    Title(Regex),
    /// Matches when the body is present and contains a match.
    Body(Regex),
    /// Matches when the application name is present and contains a match.
    App(Regex),
    /// Matches when any notification type contains a match.
    Type(Regex),
}

impl Osc99FilterRule {
    /// Whether this rule matches `data`.
    fn matches(&self, data: &Notification99Data) -> bool {
        match self {
            Self::All => true,
            Self::Title(re) => re.is_match(&data.title),
            Self::Body(re) => data.body.as_deref().is_some_and(|body| re.is_match(body)),
            Self::App(re) => data.app_name.as_deref().is_some_and(|app| re.is_match(app)),
            Self::Type(re) => data.notification_types.iter().any(|t| re.is_match(t)),
        }
    }
}

/// Why a rule could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99FilterError {
    /// The rule is neither `all` nor of the form `location:regex`.
    MissingLocation {
        /// The offending rule text.
        rule: String,
    },
    /// The part before the first `:` is not `title`, `body`, `app` or `type`.
    UnknownLocation {
        /// The offending rule text.
        rule: String,
        /// The unrecognised location.
        location: String,
    },
    /// The pattern is not a valid (or is too large a) regular expression.
    InvalidRegex {
        /// The offending rule text.
        rule: String,
        /// The regex compiler's message.
        message: String,
    },
}

impl fmt::Display for Osc99FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLocation { rule } => write!(
                f,
                "`{rule}`: expected `all` or `location:regex` (location is \
                 title, body, app or type)"
            ),
            Self::UnknownLocation { rule, location } => write!(
                f,
                "`{rule}`: unknown location `{location}` (expected title, \
                 body, app or type)"
            ),
            Self::InvalidRegex { rule, message } => {
                write!(f, "`{rule}`: invalid regex: {message}")
            }
        }
    }
}

impl std::error::Error for Osc99FilterError {}

/// Whether a notification survives the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99FilterVerdict {
    /// No rule matched: display the notification.
    Keep,
    /// A rule matched: drop the notification silently.
    Drop,
}

/// The compiled `osc99_filters` rule list. The default has no rules and
/// keeps everything.
#[derive(Debug, Clone, Default)]
pub struct Osc99Filter {
    /// The valid rules, in configuration order.
    rules: Vec<Osc99FilterRule>,
}

impl Osc99Filter {
    /// Compile `rules`, one configured entry each.
    ///
    /// Entries are trimmed; an empty entry is skipped silently. Every other
    /// entry that cannot be compiled is returned as an error and skipped.
    #[must_use]
    pub fn compile(rules: &[String]) -> (Self, Vec<Osc99FilterError>) {
        let mut compiled = Vec::with_capacity(rules.len());
        let mut errors = Vec::new();
        for raw in rules {
            let rule = raw.trim();
            if rule.is_empty() {
                continue;
            }
            match compile_rule(rule) {
                Ok(parsed) => compiled.push(parsed),
                Err(error) => errors.push(error),
            }
        }
        (Self { rules: compiled }, errors)
    }

    /// [`Self::compile`], logging each invalid rule once at `warn` level.
    ///
    /// Used where the rules are applied from configuration (startup and
    /// config apply). The rule text is the user's own config, so it is safe
    /// to log.
    #[must_use]
    pub fn compile_logged(rules: &[String]) -> Self {
        let (filter, errors) = Self::compile(rules);
        for error in &errors {
            tracing::warn!("ignoring invalid osc99_filters rule {error}");
        }
        filter
    }

    /// Split the Settings text box (one rule per line) into rule entries:
    /// each line trimmed, blank lines dropped.
    #[must_use]
    pub fn rules_from_text(text: &str) -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Join rule entries into the Settings text box form (one rule per
    /// line); the inverse of [`Self::rules_from_text`] for trimmed rules.
    #[must_use]
    pub fn rules_to_text(rules: &[String]) -> String {
        rules.join("\n")
    }

    /// Decide whether `data` is dropped: any matching rule drops it.
    #[must_use]
    pub fn matches(&self, data: &Notification99Data) -> Osc99FilterVerdict {
        if self.rules.iter().any(|rule| rule.matches(data)) {
            Osc99FilterVerdict::Drop
        } else {
            Osc99FilterVerdict::Keep
        }
    }
}

/// Compile one trimmed, non-empty rule.
fn compile_rule(rule: &str) -> Result<Osc99FilterRule, Osc99FilterError> {
    if rule == "all" {
        return Ok(Osc99FilterRule::All);
    }
    let Some((location, pattern)) = rule.split_once(':') else {
        return Err(Osc99FilterError::MissingLocation {
            rule: rule.to_owned(),
        });
    };
    let build: fn(Regex) -> Osc99FilterRule = match location {
        "title" => Osc99FilterRule::Title,
        "body" => Osc99FilterRule::Body,
        "app" => Osc99FilterRule::App,
        "type" => Osc99FilterRule::Type,
        _ => {
            return Err(Osc99FilterError::UnknownLocation {
                rule: rule.to_owned(),
                location: location.to_owned(),
            });
        }
    };
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map(build)
        .map_err(|e| Osc99FilterError::InvalidRegex {
            rule: rule.to_owned(),
            message: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use freminal_common::buffer_states::osc_notify_99::{
        NotificationOccasion, NotificationUrgency,
    };
    use freminal_common::buffer_states::osc99_notification::{
        Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry,
        Osc99Sound,
    };

    use super::*;

    fn data() -> Notification99Data {
        Notification99Data {
            id: None,
            title: "Build finished".to_owned(),
            body: None,
            icon_data: None,
            icon_names: Vec::new(),
            icon_cache_key: None,
            buttons: Vec::new(),
            activation_reporting: Osc99ActivationReporting::Silent,
            activation_focus: Osc99ActivationFocus::NoFocus,
            close_reporting: Osc99CloseReporting::Silent,
            urgency: NotificationUrgency::Normal,
            occasion: NotificationOccasion::Always,
            sound: Osc99Sound::System,
            app_name: None,
            notification_types: Vec::new(),
            expiry: Osc99Expiry::OsDefault,
        }
    }

    fn compile(rules: &[&str]) -> (Osc99Filter, Vec<Osc99FilterError>) {
        let owned: Vec<String> = rules.iter().map(|r| (*r).to_owned()).collect();
        Osc99Filter::compile(&owned)
    }

    fn verdict(rules: &[&str], data: &Notification99Data) -> Osc99FilterVerdict {
        let (filter, errors) = compile(rules);
        assert_eq!(errors, Vec::new(), "unexpected compile errors");
        filter.matches(data)
    }

    #[test]
    fn an_empty_filter_keeps_everything() {
        assert_eq!(
            Osc99Filter::default().matches(&data()),
            Osc99FilterVerdict::Keep
        );
        assert_eq!(verdict(&[], &data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn all_drops_every_notification() {
        assert_eq!(verdict(&["all"], &data()), Osc99FilterVerdict::Drop);
    }

    #[test]
    fn title_rule_matches_and_misses() {
        assert_eq!(
            verdict(&["title:^Build"], &data()),
            Osc99FilterVerdict::Drop
        );
        assert_eq!(
            verdict(&["title:^finished"], &data()),
            Osc99FilterVerdict::Keep
        );
    }

    #[test]
    fn matching_is_a_search_not_an_anchored_match() {
        assert_eq!(
            verdict(&["title:finished"], &data()),
            Osc99FilterVerdict::Drop
        );
    }

    #[test]
    fn matching_is_case_sensitive_unless_the_pattern_says_otherwise() {
        assert_eq!(verdict(&["title:build"], &data()), Osc99FilterVerdict::Keep);
        assert_eq!(
            verdict(&["title:(?i)build"], &data()),
            Osc99FilterVerdict::Drop
        );
    }

    #[test]
    fn body_rule_matches_and_misses() {
        let with_body = Notification99Data {
            body: Some("3 tests failed".to_owned()),
            ..data()
        };
        assert_eq!(
            verdict(&[r"body:\d+ tests"], &with_body),
            Osc99FilterVerdict::Drop
        );
        assert_eq!(
            verdict(&["body:passed"], &with_body),
            Osc99FilterVerdict::Keep
        );
    }

    #[test]
    fn a_missing_body_never_matches_even_an_empty_pattern() {
        assert_eq!(verdict(&["body:"], &data()), Osc99FilterVerdict::Keep);
        assert_eq!(verdict(&["body:.*"], &data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn app_rule_matches_and_misses() {
        let from_slack = Notification99Data {
            app_name: Some("slack".to_owned()),
            ..data()
        };
        assert_eq!(
            verdict(&["app:^slack$"], &from_slack),
            Osc99FilterVerdict::Drop
        );
        assert_eq!(
            verdict(&["app:^mail$"], &from_slack),
            Osc99FilterVerdict::Keep
        );
    }

    #[test]
    fn a_missing_app_name_never_matches() {
        assert_eq!(verdict(&["app:.*"], &data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn type_rule_matches_when_any_of_several_types_matches() {
        let typed = Notification99Data {
            notification_types: vec!["email".to_owned(), "im.received".to_owned()],
            ..data()
        };
        assert_eq!(
            verdict(&[r"type:^im\."], &typed),
            Osc99FilterVerdict::Drop,
            "the second type matches"
        );
        assert_eq!(
            verdict(&["type:^email$"], &typed),
            Osc99FilterVerdict::Drop,
            "the first type matches"
        );
        assert_eq!(
            verdict(&["type:^reminder$"], &typed),
            Osc99FilterVerdict::Keep
        );
    }

    #[test]
    fn a_notification_without_types_never_matches_a_type_rule() {
        assert_eq!(verdict(&["type:.*"], &data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn any_matching_rule_drops() {
        assert_eq!(
            verdict(&["app:^nope$", "title:Build"], &data()),
            Osc99FilterVerdict::Drop
        );
    }

    #[test]
    fn an_empty_rule_is_skipped_silently() {
        let (filter, errors) = compile(&["", "   ", "title:^Build"]);
        assert_eq!(errors, Vec::new());
        assert_eq!(filter.matches(&data()), Osc99FilterVerdict::Drop);

        let (only_blank, errors) = compile(&["", "  "]);
        assert_eq!(errors, Vec::new());
        assert_eq!(only_blank.matches(&data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn rules_are_trimmed() {
        assert_eq!(verdict(&["  all  "], &data()), Osc99FilterVerdict::Drop);
        assert_eq!(
            verdict(&[" title:^Build "], &data()),
            Osc99FilterVerdict::Drop
        );
    }

    #[test]
    fn a_rule_without_a_colon_is_a_missing_location() {
        let (filter, errors) = compile(&["title"]);
        assert_eq!(
            errors,
            vec![Osc99FilterError::MissingLocation {
                rule: "title".to_owned()
            }]
        );
        assert_eq!(filter.matches(&data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn all_with_a_pattern_is_not_the_bare_word() {
        let (_, errors) = compile(&["all:x"]);
        assert_eq!(
            errors,
            vec![Osc99FilterError::UnknownLocation {
                rule: "all:x".to_owned(),
                location: "all".to_owned()
            }]
        );
    }

    #[test]
    fn an_unknown_location_is_reported() {
        let (filter, errors) = compile(&["subject:hi"]);
        assert_eq!(
            errors,
            vec![Osc99FilterError::UnknownLocation {
                rule: "subject:hi".to_owned(),
                location: "subject".to_owned()
            }]
        );
        assert_eq!(filter.matches(&data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn locations_are_case_sensitive() {
        let (_, errors) = compile(&["Title:x"]);
        assert!(matches!(
            errors.as_slice(),
            [Osc99FilterError::UnknownLocation { .. }]
        ));
    }

    #[test]
    fn an_invalid_regex_is_reported_and_the_other_rules_still_apply() {
        let (filter, errors) = compile(&["title:(unclosed", "title:^Build"]);
        let [Osc99FilterError::InvalidRegex { rule, message }] = errors.as_slice() else {
            panic!("expected one InvalidRegex, got {errors:?}");
        };
        assert_eq!(rule, "title:(unclosed");
        assert!(message.contains("unclosed"), "{message}");
        assert_eq!(
            filter.matches(&data()),
            Osc99FilterVerdict::Drop,
            "the valid rule after the invalid one still applies"
        );
    }

    #[test]
    fn a_regex_over_the_size_limit_is_an_invalid_regex() {
        let (filter, errors) = compile(&["title:(\\w{100}){100}"]);
        assert!(matches!(
            errors.as_slice(),
            [Osc99FilterError::InvalidRegex { .. }]
        ));
        assert_eq!(filter.matches(&data()), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn a_pattern_may_contain_colons() {
        // Only the first `:` separates location from pattern.
        let colon = Notification99Data {
            title: "xa:by".to_owned(),
            ..data()
        };
        assert_eq!(verdict(&["title:a:b"], &colon), Osc99FilterVerdict::Drop);
        let no_colon = Notification99Data {
            title: "xab".to_owned(),
            ..data()
        };
        assert_eq!(verdict(&["title:a:b"], &no_colon), Osc99FilterVerdict::Keep);
    }

    #[test]
    fn text_form_splits_trims_and_drops_blank_lines() {
        assert_eq!(
            Osc99Filter::rules_from_text("  app:^a$ \n\n\t\ntitle:b\r\nall\n"),
            vec!["app:^a$".to_owned(), "title:b".to_owned(), "all".to_owned()]
        );
        assert_eq!(Osc99Filter::rules_from_text(""), Vec::<String>::new());
        assert_eq!(Osc99Filter::rules_from_text(" \n \n"), Vec::<String>::new());
    }

    #[test]
    fn text_form_round_trips_trimmed_rules() {
        let rules = vec!["app:^a$".to_owned(), "title:b c".to_owned()];
        let text = Osc99Filter::rules_to_text(&rules);
        assert_eq!(text, "app:^a$\ntitle:b c");
        assert_eq!(Osc99Filter::rules_from_text(&text), rules);
    }

    #[test]
    fn errors_render_the_rule_text() {
        let (_, errors) = compile(&["nope", "x:y", "title:("]);
        let rendered: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(rendered.len(), 3);
        assert!(rendered[0].contains("`nope`"));
        assert!(rendered[1].contains("unknown location `x`"));
        assert!(rendered[2].contains("invalid regex"));
    }
}
