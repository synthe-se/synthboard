//! The words offered when nothing is being typed.

use crate::engine::BAR_SLOTS;
use std::collections::HashMap;

/// The likeliest next words after common words, and at the start of a sentence.
#[derive(Debug, Clone, Default)]
pub struct NextWords {
    next: HashMap<Box<str>, Vec<Box<str>>>,
}

impl NextWords {
    /// Reads `word TAB next next next` lines, skipping bad ones.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let next = text
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(word, list)| (word.into(), list.split(' ').map(Into::into).collect()))
            .collect();
        Self { next }
    }

    /// The words to offer after `previous`, likeliest first.
    #[must_use]
    pub fn after(&self, previous: &str) -> Vec<String> {
        self.next
            .get(previous)
            .map(|list| list.iter().map(ToString::to_string).collect())
            .unwrap_or_default()
    }
}

/// Mail providers, most common first among French and English speakers. A
/// judgement, not a measurement: nothing here is learned from the user.
const MAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "proton.me",
    "outlook.com",
    "icloud.com",
    "hotmail.com",
    "yahoo.fr",
    "orange.fr",
    "free.fr",
    "yahoo.com",
    "laposte.net",
    "sfr.fr",
    "protonmail.com",
];

/// What to offer after the `@` of the address that ends `before`, if it has one.
#[must_use]
pub fn mail_domains(before: &str) -> Vec<String> {
    let Some((_, typed)) = before.rsplit_once('@') else {
        return Vec::new();
    };
    if typed.contains(char::is_whitespace) {
        return Vec::new();
    }
    let lower = typed.to_lowercase();
    MAIL_DOMAINS
        .iter()
        .filter(|domain| domain.starts_with(&lower) && **domain != lower)
        .take(BAR_SLOTS)
        .map(|domain| (*domain).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_follow_what_was_typed_after_the_at_sign() {
        assert_eq!(mail_domains("john@pro"), ["proton.me", "protonmail.com"]);
        assert_eq!(mail_domains("john@GM"), ["gmail.com"]);
        assert_eq!(mail_domains("john@gmail.com"), [] as [String; 0]);
        assert_eq!(mail_domains("john"), [] as [String; 0]);
        assert_eq!(mail_domains("john@ a"), [] as [String; 0]);
        assert_eq!(mail_domains("a@").len(), BAR_SLOTS);
    }

    #[test]
    fn lines_give_their_words_and_bad_lines_are_skipped() {
        let words = NextWords::parse("je\tsuis ne\nbad line\n");
        assert_eq!(words.after("je"), ["suis", "ne"]);
        assert_eq!(words.after("bad line"), [] as [std::string::String; 0]);
    }
}
