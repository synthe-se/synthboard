//! What the keyboard learns from its user, kept on the device. It stores which
//! words follow which, and words missing from the lexicon. A word counts once
//! the next one has begun, so a word deleted straight away is never learned.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::engine::BAR_SLOTS;
use synthswipe::{Lexicon, SENTENCE_START, signature_distance, tokens};

/// Uses before a word missing from the lexicon becomes swipeable (B-10.6).
const VALIDATIONS: u32 = 2;
/// Weight of a learned word, about that of a fairly common word.
const LEARNED_WEIGHT: f32 = 140.0;
/// Bounds the file, and every save with it: years of a daily user's words fit well under.
const MAX_ENTRIES: usize = 50_000;
const SEPARATOR: char = '\t';
const FORM_MARK: char = '^';
/// Words of context kept before a word, to learn whole phrasings.
const MAX_CONTEXT: usize = 3;
/// Words of context a phrasing needs to run on.
const MIN_RUN_CONTEXT: usize = 2;
/// Uses before a word extends an offered phrasing.
const PHRASE_USES: u32 = 2;
/// Words a phrasing may add after the first offered one.
const MAX_PHRASE_EXTRA: usize = 3;
const BLOCK_MARK: char = '!';
const TRACE_MARK: char = '~';
/// The gestures kept as the user's own, each compared on every decode.
const MAX_TRACES: usize = 500;
/// Enough to cover how one word gets drawn, while older habits age out.
const TRACES_PER_WORD: usize = 4;
/// How close a gesture must be to a personal trace, in key pitches.
const TRACE_REACH: f32 = 0.45;

/// The last one to three words of a sentence, shortest first, as joined context.
fn contexts(history: &[String]) -> Vec<String> {
    (1..=MAX_CONTEXT.min(history.len()))
        .filter_map(|length| history.get(history.len() - length..))
        .map(|words| words.join(" "))
        .collect()
}

/// Learned words and word pairs.
#[derive(Debug, Clone, Default)]
pub struct Memory {
    words: HashMap<String, u32>,
    /// What followed each context of one to three words, with its count.
    pairs: HashMap<String, HashMap<String, u32>>,
    /// How many pairs there are, so that learning a word needs no count of them.
    pair_total: usize,
    /// Words the user writes with capitals, by their lowercase form.
    forms: HashMap<String, String>,
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
                    } else if let Some(form) = word.strip_prefix(FORM_MARK) {
                        memory.forms.insert(form.to_lowercase(), form.to_owned());
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
                        memory.pairs.entry((*first).to_owned()).or_default().insert((*second).to_owned(), count);
                    }
                }
                _ => {}
            }
        }
        memory.count_pairs();
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
        for (first, next) in &self.pairs {
            for (second, count) in next {
                let _ = writeln!(text, "{first}{SEPARATOR}{second}{SEPARATOR}{count}");
            }
        }
        for word in &self.blocked {
            let _ = writeln!(text, "{BLOCK_MARK}{word}");
        }
        for form in self.forms.values() {
            let _ = writeln!(text, "{FORM_MARK}{form}");
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

    /// Counts a refused correction as one use, so that refusing the same word
    /// twice adopts it: one backspace can be a mere edit of the corrected word.
    pub fn keep(&mut self, word: &str, known: impl Fn(&str) -> bool) {
        let word = word.to_lowercase();
        let count = self.words.entry(word.clone()).or_default();
        *count = count.saturating_add(1);
        if *count >= VALIDATIONS {
            self.blocked.remove(&word);
            self.rebuild(known);
        }
    }

    /// Records that `word` was validated after `previous`.
    pub fn learn(&mut self, previous: &str, word: &str, known: impl Fn(&str) -> bool) {
        if self.is_full() {
            self.forget_rarest();
            self.rebuild(&known);
        }
        let count = self.words.entry(word.to_owned()).or_default();
        *count = count.saturating_add(1);
        let promoted = *count == VALIDATIONS;
        self.bump(previous.to_owned(), word);
        if promoted {
            self.blocked.remove(word);
            self.rebuild(known);
        }
    }

    fn bump(&mut self, context: String, word: &str) {
        let count = self.pairs.entry(context).or_default().entry(word.to_owned()).or_default();
        self.pair_total += usize::from(*count == 0);
        *count = count.saturating_add(1);
    }

    /// Records `word` after every length of context in `history`, so that the
    /// longest phrasing the user has really written can be recognised.
    pub fn learn_after(&mut self, history: &[String], previous: &str, word: &str, known: impl Fn(&str) -> bool) {
        self.learn(previous, word, known);
        for context in contexts(history).into_iter().skip(1) {
            self.bump(context, word);
        }
    }

    /// Takes back one [`Self::learn_after`], for words the user erased.
    pub fn unlearn(&mut self, history: &[String], previous: &str, word: &str, known: impl Fn(&str) -> bool) {
        let demoted = self.words.get(word) == Some(&VALIDATIONS);
        if let Some(count) = self.words.get_mut(word) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.words.remove(word);
            }
        }
        self.drop_pair(previous, word);
        for context in contexts(history).into_iter().skip(1) {
            self.drop_pair(&context, word);
        }
        if demoted {
            self.rebuild(known);
        }
    }

    fn drop_pair(&mut self, context: &str, word: &str) {
        let Some(followers) = self.pairs.get_mut(context) else {
            return;
        };
        if let Some(count) = followers.get_mut(word) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                followers.remove(word);
                self.pair_total = self.pair_total.saturating_sub(1);
            }
        }
        if followers.is_empty() {
            self.pairs.remove(context);
        }
    }

    /// Remembers how a word is written. Plain lowercase means it has no form.
    pub fn set_form(&mut self, written: &str) {
        let lower = written.to_lowercase();
        if lower == written {
            self.forms.remove(&lower);
        } else {
            self.forms.insert(lower, written.to_owned());
        }
    }

    /// The form stored for a lowercase word, if any.
    #[must_use]
    pub fn saved_form(&self, word: &str) -> Option<String> {
        self.forms.get(word).cloned()
    }

    /// Puts back what [`Self::saved_form`] gave.
    pub fn restore_form(&mut self, word: &str, form: Option<String>) {
        match form {
            Some(form) => self.forms.insert(word.to_owned(), form),
            None => self.forms.remove(word),
        };
    }

    /// How the user writes this word, or the word as given.
    #[must_use]
    pub fn form(&self, word: &str) -> String {
        self.forms.get(&word.to_lowercase()).cloned().unwrap_or_else(|| word.to_owned())
    }

    /// The strongest habit for `word`, longer contexts counting more.
    #[must_use]
    pub fn habit_after(&self, history: &[String], previous: &str, word: &str) -> f32 {
        let longest = contexts(history)
            .iter()
            .zip(1_u8..)
            .map(|(context, length)| f32::from(length) * self.habit(context, word))
            .fold(0.0, f32::max);
        longest.max(self.habit(previous, word))
    }

    /// The user's phrasings that follow `history`, longest context first. Each
    /// runs on while the user has written its next word more than once.
    #[must_use]
    pub fn phrases(&self, history: &[String], previous: &str) -> Vec<String> {
        let mut starts = contexts(history);
        starts.reverse();
        if starts.is_empty() {
            starts.push(previous.to_owned());
        }
        let mut found: Vec<String> = Vec::new();
        for context in starts {
            for word in self.next_after(&context) {
                if !found.iter().any(|known| known.split(' ').next() == Some(word.as_str())) {
                    found.push(self.extend(&context, &word));
                }
            }
        }
        found.truncate(BAR_SLOTS);
        found
    }

    fn extend(&self, context: &str, first: &str) -> String {
        let mut recent: Vec<String> =
            context.split(' ').filter(|token| *token != SENTENCE_START).map(str::to_owned).collect();
        let mut phrase = self.form(first);
        recent.extend(tokens(first));
        for _ in 0..MAX_PHRASE_EXTRA {
            // Two words of context at least, or a common word would run on forever.
            let shortest = MIN_RUN_CONTEXT.min(recent.len());
            let next = (shortest..=MAX_CONTEXT.min(recent.len())).rev().find_map(|length| {
                let tail = recent.get(recent.len() - length..).unwrap_or_default().join(" ");
                self.pairs
                    .get(&tail)
                    .into_iter()
                    .flatten()
                    .filter(|(_, count)| **count >= PHRASE_USES)
                    .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            });
            let Some((word, _)) = next else {
                break;
            };
            phrase.push(' ');
            phrase.push_str(&self.form(word));
            recent.extend(tokens(word));
        }
        phrase
    }

    /// Forgets a word: its habits go, and it is no longer offered until the
    /// user types it twice again.
    pub fn forget(&mut self, word: &str, known: impl Fn(&str) -> bool) {
        let word = word.to_lowercase();
        self.words.remove(&word);
        self.forms.remove(&word);
        self.pairs.retain(|context, next| {
            next.remove(&word);
            !next.is_empty() && !context.split(' ').any(|token| token == word)
        });
        self.count_pairs();
        self.traces.retain(|(known, _)| *known != word);
        self.blocked.insert(word);
        self.rebuild(known);
    }

    /// Blocks a word again, when the use that adopted it is taken back.
    pub fn restore_block(&mut self, word: &str, known: impl Fn(&str) -> bool) {
        self.blocked.insert(word.to_owned());
        self.rebuild(known);
    }

    /// How many words the user asked to forget.
    #[must_use]
    pub fn blocked_count(&self) -> usize {
        self.blocked.len()
    }

    /// Whether the user asked to forget this word.
    #[must_use]
    pub fn is_blocked(&self, word: &str) -> bool {
        self.blocked.contains(&word.to_lowercase())
    }

    fn count_pairs(&mut self) {
        self.pair_total = self.pairs.values().map(HashMap::len).sum();
    }

    fn is_full(&self) -> bool {
        self.words.len() + self.pair_total >= MAX_ENTRIES
    }

    /// Drops what was seen once, then, while that leaves memory full, what
    /// was seen twice as often, so that `MAX_ENTRIES` holds however used it is.
    fn forget_rarest(&mut self) {
        let mut uses = 1;
        loop {
            self.pairs.retain(|_, next| {
                next.retain(|_, count| *count > uses);
                !next.is_empty()
            });
            self.count_pairs();
            self.words.retain(|_, count| *count > uses);
            if !self.is_full() || uses == u32::MAX {
                return;
            }
            uses = uses.saturating_mul(2);
        }
    }

    /// The words learned that the lexicon lacks.
    #[must_use]
    pub fn extra_words(&self) -> Option<Arc<Lexicon>> {
        self.extra.clone()
    }

    /// How strongly the user's habit favours `word` after `previous`, in nats.
    #[must_use]
    pub fn habit(&self, previous: &str, word: &str) -> f32 {
        let word = word.to_lowercase();
        self.pairs.get(previous).and_then(|next| next.get(&word)).map_or(0.0, |&count| (1.0 + count as f32).ln())
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

    /// Removes the newest trace of a word with exactly these points.
    pub fn remove_trace(&mut self, word: &str, points: &[[f32; 2]]) {
        if let Some(index) = self.traces.iter().rposition(|(known, shape)| known == word && shape == points) {
            self.traces.remove(index);
        }
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
        let mut found: Vec<(u32, &str)> =
            self.pairs.get(previous).into_iter().flatten().map(|(word, &count)| (count, word.as_str())).collect();
        found.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        found.into_iter().take(BAR_SLOTS).map(|(_, word)| word.to_owned()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_word_taken_back_below_two_uses_leaves_the_extra_words() {
        let mut memory = Memory::default();
        memory.learn("pour", "zzgloop", |_| false);
        memory.learn("pour", "zzgloop", |_| false);
        assert!(memory.extra_words().is_some_and(|words| words.contains("zzgloop")));
        memory.unlearn(&[], "pour", "zzgloop", |_| false);
        assert!(!memory.extra_words().is_some_and(|words| words.contains("zzgloop")));
        assert_eq!(memory.words.get("zzgloop"), Some(&1));
    }

    #[test]
    fn a_word_taken_back_leaves_no_trace_of_its_pairs() {
        let mut memory = Memory::default();
        let history = vec!["merci".to_owned(), "pour".to_owned()];
        memory.learn_after(&history, "pour", "tout", |_| true);
        memory.learn_after(&history, "pour", "tout", |_| true);
        memory.unlearn(&history, "pour", "tout", |_| true);
        assert_eq!(memory.next_after("pour"), ["tout"]);
        memory.unlearn(&history, "pour", "tout", |_| true);
        assert_eq!(memory.next_after("pour"), Vec::<String>::new());
        assert_eq!(memory.pair_total, 0);
        assert!(memory.words.is_empty());
        memory.unlearn(&history, "pour", "tout", |_| true);
        assert_eq!(memory.pair_total, 0);
    }

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

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    fn write(memory: &mut Memory, sentence: &str) {
        let tokens = words(sentence);
        for index in 0..tokens.len() {
            let history = tokens[..index].to_vec();
            let previous = history.last().cloned().unwrap_or_else(|| "<s>".to_owned());
            memory.learn_after(&history, &previous, &tokens[index], |_| true);
        }
    }

    #[test]
    fn a_repeated_phrasing_is_offered_whole() {
        let mut memory = Memory::default();
        write(&mut memory, "je te tiens au courant");
        write(&mut memory, "je te tiens au courant");
        let offered = memory.phrases(&words("je te"), "te");
        assert_eq!(offered.first().map(String::as_str), Some("tiens au courant"));
    }

    #[test]
    fn a_phrasing_seen_once_offers_only_its_next_word() {
        let mut memory = Memory::default();
        write(&mut memory, "je te tiens au courant");
        assert_eq!(memory.phrases(&words("je te"), "te"), ["tiens"]);
    }

    #[test]
    fn the_longest_context_wins_over_a_common_word() {
        let mut memory = Memory::default();
        for _ in 0..5 {
            write(&mut memory, "de la");
        }
        write(&mut memory, "je pense de toi");
        write(&mut memory, "je pense de toi");
        let offered = memory.phrases(&words("je pense de"), "de");
        assert!(offered.first().is_some_and(|phrase| phrase.starts_with("toi")), "{offered:?}");
        assert!(memory.habit_after(&words("je pense de"), "de", "toi") > memory.habit("de", "toi"));
    }

    #[test]
    fn forms_survive_saving_and_lowercase_clears_them() {
        let mut memory = Memory::default();
        memory.set_form("GitHub");
        assert_eq!(memory.form("github"), "GitHub");
        let saved = Memory::parse(&memory.to_text(), |_| true);
        assert_eq!(saved.form("GITHUB"), "GitHub");
        memory.set_form("github");
        assert_eq!(memory.form("github"), "github");
    }

    #[test]
    fn forgetting_a_word_removes_the_phrasings_through_it() {
        let mut memory = Memory::default();
        write(&mut memory, "je te tiens");
        memory.forget("te", |_| true);
        assert_eq!(memory.phrases(&words("je"), "je"), Vec::<String>::new());
    }

    #[test]
    fn forgetting_a_word_spares_what_followed_other_contexts() {
        let mut memory = Memory::default();
        memory.learn("te", "voir", |_| true);
        memory.learn("pour", "voir", |_| true);
        memory.forget("te", |_| true);
        assert_eq!(memory.habit("te", "voir"), 0.0);
        assert!(memory.habit("pour", "voir") > 0.0);
    }

    #[test]
    fn a_full_memory_drops_its_rarest_entries() {
        let mut memory = Memory::default();
        for index in 0..MAX_ENTRIES / 2 {
            memory.words.insert(format!("w{index}"), 1);
            memory.bump(format!("c{index}"), "x");
        }
        memory.learn("a", "b", |_| true);
        assert!(memory.words.len() + memory.pairs.len() < 10);
    }

    #[test]
    fn a_memory_full_of_frequent_entries_still_makes_room() {
        let mut memory = Memory::default();
        for index in 0..MAX_ENTRIES {
            memory.words.insert(format!("w{index}"), 5);
        }
        memory.learn("a", "b", |_| true);
        assert!(memory.words.len() + memory.pair_total < MAX_ENTRIES);
    }

    #[test]
    fn a_word_dropped_when_full_leaves_the_extra_words() {
        let mut memory = Memory::default();
        memory.learn("a", "zxcv", |_| false);
        memory.learn("a", "zxcv", |_| false);
        for index in 0..MAX_ENTRIES {
            memory.words.insert(format!("w{index}"), 9);
        }
        memory.learn("a", "b", |_| false);
        assert!(memory.extra_words().is_none_or(|words| !words.contains("zxcv")));
    }

    #[test]
    fn a_longer_context_counts_in_proportion() {
        let mut memory = Memory::default();
        write(&mut memory, "je te vois");
        let expected = 2.0 * 2.0_f32.ln();
        assert!((memory.habit_after(&words("je te"), "te", "vois") - expected).abs() < 1e-5);
    }

    #[test]
    fn the_pair_count_follows_what_is_forgotten() {
        let mut memory = Memory::default();
        memory.learn("a", "b", |_| true);
        memory.learn("a", "c", |_| true);
        assert_eq!(memory.pair_total, 2);
        memory.forget("c", |_| true);
        assert_eq!(memory.pair_total, 1);
        assert_eq!(Memory::parse(&memory.to_text(), |_| true).pair_total, 1);
    }

    #[test]
    fn a_word_refused_twice_is_adopted_and_survives_saving() {
        let mut memory = Memory::default();
        memory.forget("synthe", |_| false);
        memory.keep("Synthe", |_| false);
        assert!(memory.is_blocked("synthe"));
        assert!(memory.extra_words().is_none());
        memory.keep("synthe", |_| false);
        assert!(!memory.is_blocked("synthe"));
        let saved = Memory::parse(&memory.to_text(), |_| false);
        assert!(saved.extra_words().is_some_and(|words| words.contains("synthe")));
    }

    #[test]
    fn a_capitalised_candidate_finds_its_habit() {
        let mut memory = Memory::default();
        memory.learn("voir", "francis", |_| true);
        assert!(memory.habit("voir", "Francis") > 0.0);
    }

    #[test]
    fn a_phrasing_runs_on_from_a_sentence_start_and_through_elisions() {
        let mut memory = Memory::default();
        let spoken = |memory: &mut Memory| {
            memory.learn_after(&[], "<s>", "bonne", |_| true);
            memory.learn_after(&words("bonne"), "bonne", "journée", |_| true);
            memory.learn_after(&words("j'"), "j'", "ai", |_| true);
            memory.learn_after(&words("j' ai"), "ai", "dit", |_| true);
        };
        spoken(&mut memory);
        spoken(&mut memory);
        assert_eq!(memory.phrases(&[], "<s>").first().map(String::as_str), Some("bonne journée"));
        memory.learn_after(&words("tu"), "tu", "j'ai", |_| true);
        assert_eq!(memory.phrases(&words("tu"), "tu").first().map(String::as_str), Some("j'ai dit"));
    }

    #[test]
    fn a_phrasing_shows_the_forms_the_user_gave_its_words() {
        let mut memory = Memory::default();
        write(&mut memory, "sur github");
        write(&mut memory, "sur github");
        memory.set_form("GitHub");
        assert_eq!(memory.phrases(&words("sur"), "sur"), ["GitHub"]);
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
