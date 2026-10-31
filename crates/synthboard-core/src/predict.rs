//! The words offered when nothing is being typed.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_give_their_words_and_bad_lines_are_skipped() {
        let words = NextWords::parse("je\tsuis ne\nbad line\n");
        assert_eq!(words.after("je"), ["suis", "ne"]);
        assert!(words.after("bad line").is_empty());
    }
}
