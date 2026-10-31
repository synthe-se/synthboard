//! What the keyboard learns from its user, kept on the device. It stores which
//! words follow which, and words missing from the lexicon. A word counts once
//! the next one has begun, so a word deleted straight away is never learned.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use synthswipe::{Lexicon, signature_distance};

/// Uses before a word missing from the lexicon becomes swipeable (B-10.6).
const VALIDATIONS: u32 = 2;
/// Weight of a learned word, about that of a fairly common word.
const LEARNED_WEIGHT: f32 = 140.0;
const MAX_ENTRIES: usize = 50_000;
const SLOTS: usize = 3;
const SEPARATOR: char = '\t';
const BLOCK_MARK: char = '!';
const TRACE_MARK: char = '~';
const MAX_TRACES: usize = 500;
const TRACES_PER_WORD: usize = 4;
/// How close a gesture must be to a personal trace, in key pitches.
const TRACE_REACH: f32 = 0.45;

/// Learned words and word pairs.
#[derive(Debug, Clone, Default)]
pub struct Memory {
    words: HashMap<String, u32>,
    pairs: HashMap<(String, String), u32>,
    /// Words the user asked to forget, lexicon words included.
    blocked: HashSet<String>,
    /// The user's own gestures for words the decoder got wrong.
    traces: Vec<(String, Vec<[f32; 2]>)>,
    extra: Option<Arc<Lexicon>>,
}

impl Memory {
    /// Reads saved memory, skipping bad lines.
    #[must_use]
    pub fn parse(text: &str, known: impl Fn(&str) -> bool) -> Self {
        let mut memory = Self::default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split(SEPARATOR).collect();
            match fields.as_slice() {
                [word] => {
                    if let Some(blocked) = word.strip_prefix(BLOCK_MARK) {
                        memory.blocked.insert(blocked.to_owned());
                    }
                }
                [marked, points] if marked.starts_with(TRACE_MARK) => {
                    let parsed: Vec<[f32; 2]> = points
                        .split(' ')
                        .filter_map(|point| {
                            let (x, y) = point.split_once(',')?;
                            Some([x.parse().ok()?, y.parse().ok()?])
                        })
                        .collect();
                    memory.traces.push((marked.trim_start_matches(TRACE_MARK).to_owned(), parsed));
                }
                [word, count] => {
                    if let Ok(count) = count.parse() {
                        memory.words.insert((*word).to_owned(), count);
                    }
                }
                [first, second, count] => {
                    if let Ok(count) = count.parse() {
                        memory.pairs.insert(((*first).to_owned(), (*second).to_owned()), count);
                    }
                }
                _ => {}
            }
        }
        memory.rebuild(known);
        memory
    }

    /// The text to save.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for (word, count) in &self.words {
            let _ = writeln!(text, "{word}{SEPARATOR}{count}");
        }
        for ((first, second), count) in &self.pairs {
            let _ = writeln!(text, "{first}{SEPARATOR}{second}{SEPARATOR}{count}");
        }
        for word in &self.blocked {
            let _ = writeln!(text, "{BLOCK_MARK}{word}");
        }
        for (word, points) in &self.traces {
            let joined: Vec<String> = points.iter().map(|[x, y]| format!("{x:.2},{y:.2}")).collect();
            let _ = writeln!(text, "{TRACE_MARK}{word}{SEPARATOR}{}", joined.join(" "));
        }
        text
    }

    fn rebuild(&mut self, known: impl Fn(&str) -> bool) {
        let learned: Vec<&str> = self
            .words
            .iter()
            .filter(|&(word, &count)| count >= VALIDATIONS && !known(word) && !self.blocked.contains(word))
            .map(|(word, _)| word.as_str())
            .collect();
        self.extra = (!learned.is_empty())
            .then(|| Arc::new(Lexicon::new(learned.into_iter().map(|word| (word, LEARNED_WEIGHT)))));
    }

    /// Records that `word` was validated after `previous`.
    pub fn learn(&mut self, previous: &str, word: &str, known: impl Fn(&str) -> bool) {
        if self.words.len() + self.pairs.len() >= MAX_ENTRIES {
            self.forget_rarest();
        }
        let count = self.words.entry(word.to_owned()).or_default();
        *count += 1;
        let promoted = *count == VALIDATIONS;
        *self.pairs.entry((previous.to_owned(), word.to_owned())).or_default() += 1;
        if promoted {
            self.blocked.remove(word);
            self.rebuild(known);
        }
    }

    /// Forgets a word: its habits go, and it is no longer offered until the
    /// user types it twice again.
    pub fn forget(&mut self, word: &str, known: impl Fn(&str) -> bool) {
        let word = word.to_lowercase();
        self.words.remove(&word);
        self.pairs.retain(|(first, second), _| *first != word && *second != word);
        self.traces.retain(|(known, _)| *known != word);
        self.blocked.insert(word);
        self.rebuild(known);
    }

    /// Whether the user asked to forget this word.
    #[must_use]
    pub fn is_blocked(&self, word: &str) -> bool {
        self.blocked.contains(&word.to_lowercase())
    }

    fn forget_rarest(&mut self) {
        self.pairs.retain(|_, count| *count > 1);
        self.words.retain(|_, count| *count > 1);
    }

    /// The words learned that the lexicon lacks.
    #[must_use]
    pub fn extra_words(&self) -> Option<Arc<Lexicon>> {
        self.extra.clone()
    }

    /// How strongly the user's habit favours `word` after `previous`, in nats.
    #[must_use]
    pub fn habit(&self, previous: &str, word: &str) -> f32 {
        let pair = (previous.to_owned(), word.to_owned());
        self.pairs.get(&pair).map_or(0.0, |&count| (1.0 + count as f32).ln())
    }

    /// Keeps a gesture as the user's own way of writing `word`.
    pub fn add_trace(&mut self, word: &str, points: Vec<[f32; 2]>) {
        let word = word.to_lowercase();
        if self.traces.iter().filter(|(known, _)| *known == word).count() >= TRACES_PER_WORD
            && let Some(oldest) = self.traces.iter().position(|(known, _)| *known == word) {
                self.traces.remove(oldest);
            }
        if self.traces.len() >= MAX_TRACES {
            self.traces.remove(0);
        }
        self.traces.push((word, points));
    }

    /// Words whose personal trace resembles this gesture, closest first. Each
    /// has a strength from 0 at the limit of reach to 1 for an exact match,
    /// and the number of its traces that match.
    #[must_use]
    pub fn trace_matches(&self, gesture: &[[f32; 2]]) -> Vec<(String, f32, u32)> {
        let mut found: Vec<(String, f32, u32)> = Vec::new();
        for (word, points) in &self.traces {
            let strength = 1.0 - signature_distance(points, gesture) / TRACE_REACH;
            if strength <= 0.0 || self.blocked.contains(word) {
                continue;
            }
            match found.iter_mut().find(|(known, ..)| known == word) {
                Some(entry) => {
                    entry.1 = entry.1.max(strength);
                    entry.2 += 1;
                }
                None => found.push((word.clone(), strength, 1)),
            }
        }
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        found
    }

    /// How familiar a word is to this user, in nats, wherever it stands.
    #[must_use]
    pub fn familiarity(&self, word: &str) -> f32 {
        self.words.get(&word.to_lowercase()).map_or(0.0, |&count| (1.0 + count as f32).ln())
    }

    /// The user's own next words after `previous`, most frequent first.
    #[must_use]
    pub fn next_after(&self, previous: &str) -> Vec<String> {
        let mut found: Vec<(u32, &str)> = self
            .pairs
            .iter()
            .filter(|((first, _), _)| first == previous)
            .map(|((_, second), &count)| (count, second.as_str()))
            .collect();
        found.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        found.into_iter().take(SLOTS).map(|(_, word)| word.to_owned()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn habits_lead_the_next_words() {
        let mut memory = Memory::default();
        memory.learn("merci", "beaucoup", |_| true);
        memory.learn("merci", "encore", |_| true);
        memory.learn("merci", "encore", |_| true);
        assert_eq!(memory.next_after("merci"), ["encore", "beaucoup"]);
        assert!(memory.habit("merci", "encore") > memory.habit("merci", "beaucoup"));
        assert!(memory.habit("autre", "encore").abs() < f32::EPSILON);
    }

    #[test]
    fn a_personal_trace_is_recognised_nearby_and_survives_saving() {
        let trace = vec![[1.0, 1.0], [2.0, 1.0], [3.0, 1.0]];
        let near = vec![[1.1, 1.0], [2.1, 1.0], [3.1, 1.0]];
        let far = vec![[1.0, 3.0], [2.0, 3.0], [3.0, 3.0]];
        let mut memory = Memory::default();
        memory.add_trace("Pue", trace);
        assert_eq!(memory.trace_matches(&near).first().map(|m| m.0.as_str()), Some("pue"));
        assert_eq!(memory.trace_matches(&far), [] as [(std::string::String, f32, u32); 0]);
        let saved = Memory::parse(&memory.to_text(), |_| true);
        assert_eq!(saved.trace_matches(&near).len(), 1);
    }

    #[test]
    fn a_word_keeps_only_its_latest_traces() {
        let mut memory = Memory::default();
        for offset in 0_u8..9 {
            memory.add_trace("mot", vec![[f32::from(offset), 0.0]]);
        }
        assert!(memory.traces.len() <= TRACES_PER_WORD);
    }

    #[test]
    fn a_habit_is_the_log_of_its_count() {
        let mut memory = Memory::default();
        memory.learn("a", "b", |_| true);
        memory.learn("a", "b", |_| true);
        assert!((memory.habit("a", "b") - 3.0_f32.ln()).abs() < 0.001);
        assert!((memory.familiarity("b") - 3.0_f32.ln()).abs() < 0.001);
    }

    #[test]
    fn a_full_memory_forgets_what_was_seen_once() {
        let mut memory = Memory::default();
        for index in 0..30 {
            memory.learn("x", &format!("w{index}"), |_| true);
        }
        memory.learn("x", "kept", |_| true);
        memory.learn("x", "kept", |_| true);
        memory.forget_rarest();
        assert_eq!(memory.next_after("x"), ["kept"]);
        assert!(memory.familiarity("w1").abs() < f32::EPSILON);
        assert!(memory.familiarity("kept") > 0.0);
    }

    #[test]
    fn a_known_word_never_joins_the_extra_words() {
        let mut memory = Memory::default();
        memory.learn("a", "bonjour", |_| true);
        memory.learn("a", "bonjour", |_| true);
        assert!(memory.extra_words().is_none());
    }

    #[test]
    fn saved_memory_keeps_blocks_counts_and_extra_words() {
        let mut memory = Memory::default();
        memory.learn("a", "zxcv", |_| false);
        memory.learn("a", "zxcv", |_| false);
        memory.forget("autre", |_| true);
        let saved = Memory::parse(&memory.to_text(), |_| false);
        assert!(saved.is_blocked("autre"));
        assert!(saved.extra_words().is_some_and(|words| words.contains("zxcv")));
        assert!((saved.habit("a", "zxcv") - 3.0_f32.ln()).abs() < 0.001);
    }

    #[test]
    fn traces_keep_the_closest_strength_and_count_every_match() {
        let mut memory = Memory::default();
        let exact = vec![[1.0, 1.0], [2.0, 1.0]];
        memory.add_trace("mot", exact.clone());
        memory.add_trace("mot", vec![[1.2, 1.0], [2.2, 1.0]]);
        memory.add_trace("mot", vec![[9.0, 9.0], [9.0, 9.0]]);
        let found = memory.trace_matches(&exact);
        assert_eq!(found.len(), 1);
        assert!((found[0].1 - 1.0).abs() < 0.001);
        assert_eq!(found[0].2, 2);
        memory.forget("mot", |_| true);
        assert_eq!(memory.trace_matches(&exact), Vec::new());
    }

    #[test]
    fn a_chosen_word_grows_familiar_everywhere() {
        let mut memory = Memory::default();
        assert!(memory.familiarity("pue").abs() < f32::EPSILON);
        memory.learn("il", "pue", |_| true);
        memory.learn("ça", "pue", |_| true);
        assert!(memory.familiarity("Pue") > 1.0);
    }

    #[test]
    fn a_missing_word_becomes_swipeable_after_two_uses() {
        let mut memory = Memory::default();
        memory.learn("<s>", "commité", |_| false);
        assert!(memory.extra_words().is_none());
        memory.learn("<s>", "commité", |_| false);
        assert!(memory.extra_words().is_some_and(|words| words.contains("commité")));
    }

    #[test]
    fn a_forgotten_word_leaves_every_list_until_typed_twice_again() {
        let mut memory = Memory::default();
        memory.learn("je", "merde", |_| true);
        memory.forget("Merde", |_| true);
        assert!(memory.is_blocked("merde"));
        assert_eq!(memory.next_after("je"), [] as [std::string::String; 0]);
        let saved = Memory::parse(&memory.to_text(), |_| true);
        assert!(saved.is_blocked("merde"));
        memory.learn("je", "merde", |_| true);
        assert!(memory.is_blocked("merde"));
        memory.learn("je", "merde", |_| true);
        assert!(!memory.is_blocked("merde"));
    }

    #[test]
    fn memory_survives_saving_and_skips_bad_lines() {
        let mut memory = Memory::default();
        memory.learn("je", "suis", |_| true);
        let text = format!("{}junk\n", memory.to_text());
        let loaded = Memory::parse(&text, |_| true);
        assert_eq!(loaded.next_after("je"), ["suis"]);
    }
}
