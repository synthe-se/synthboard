use std::collections::HashMap;

/// The token that opens a sentence.
pub const SENTENCE_START: &str = "<s>";

/// Lowercase words of a line, with an elided word ending on its apostrophe,
/// so that `j'ai` gives `j'` then `ai`. Both apostrophes count as one.
#[must_use]
pub fn tokens(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for letter in line.chars().flat_map(char::to_lowercase) {
        if letter.is_alphanumeric() || (letter == '-' && !word.is_empty()) {
            word.push(letter);
        } else {
            if matches!(letter, '\'' | '’') && !word.is_empty() {
                word.push('\'');
            }
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// A lexicon weight from a count in spoken language: its log, on the 0 to 255
/// scale of lexicon weights.
#[must_use]
pub fn spoken_weight(count: u32) -> f32 {
    /// Puts the log of a corpus count on the scale of the word lists.
    const WEIGHT_PER_NAT: f32 = 16.0;
    (1.0 + count as f32).ln() * WEIGHT_PER_NAT
}

/// Corpus counts of words and word pairs, from which an [`Affinities`] table
/// is compiled. This is a build-time structure and never reaches the keyboard.
#[derive(Debug, Clone, Default)]
pub struct Bigrams {
    ids: HashMap<Box<str>, u32>,
    unigrams: Vec<u32>,
    pairs: HashMap<(u32, u32), u32>,
    total: f64,
}

impl Bigrams {
    /// Reads `word count` and `first second count` lines, in any order.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut model = Self::default();
        let rows: Vec<Vec<&str>> = text.lines().map(|line| line.split(' ').collect()).collect();
        for row in &rows {
            if let [word, count] = row.as_slice() {
                let count = count.parse().unwrap_or(0);
                let id = model.intern(word);
                if let Some(slot) = model.unigrams.get_mut(id as usize) {
                    *slot = count;
                }
                model.total += f64::from(count);
            }
        }
        for row in &rows {
            if let [first, second, count] = row.as_slice() {
                let ids = (model.ids.get(*first).copied(), model.ids.get(*second).copied());
                if let (Some(a), Some(b)) = ids {
                    model.pairs.insert((a, b), count.parse().unwrap_or(0));
                }
            }
        }
        model
    }

    fn intern(&mut self, word: &str) -> u32 {
        if let Some(&id) = self.ids.get(word) {
            return id;
        }
        let id = u32::try_from(self.unigrams.len()).unwrap_or(u32::MAX);
        self.ids.insert(word.into(), id);
        self.unigrams.push(0);
        id
    }

    fn unigram(&self, id: u32) -> f64 {
        f64::from(self.unigrams.get(id as usize).copied().unwrap_or(0))
    }

    /// Appends one `(pair hash, affinity)` entry per pair worth keeping. The
    /// affinity says how much the first word favours the second, in nats,
    /// compared to its usual frequency, so that common words get no credit
    /// for being common.
    pub fn compile_into(&self, min_count: u32, min_nats: f64, entries: &mut Vec<(u32, i8)>) {
        let words: HashMap<u32, &str> = self.ids.iter().map(|(word, &id)| (id, &**word)).collect();
        for (&(first, second), &count) in &self.pairs {
            if count < min_count {
                continue;
            }
            let expected = self.unigram(first) * self.unigram(second) / self.total.max(1.0);
            let nats = (f64::from(count) / expected.max(MIN_EXPECTED)).ln().clamp(-MAX_NATS, MAX_NATS);
            if nats.abs() < min_nats {
                continue;
            }
            if let (Some(a), Some(b)) = (words.get(&first), words.get(&second)) {
                entries.push((pair_hash(a, b), (nats * STEPS_PER_NAT) as i8));
            }
        }
    }
}

/// A pair seen in subtitles may not outweigh the gesture, whatever its score.
const MAX_NATS: f64 = 4.0;
/// Floors the expected count, so that rare words do not get huge scores.
const MIN_EXPECTED: f64 = 1.0;
const STEPS_PER_NAT: f64 = 16.0;
const ENTRY_BYTES: usize = 5;

/// The 32-bit FNV-1a hash, as its specification fixes it.
const FNV_OFFSET_BASIS: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;

fn pair_hash(first: &str, second: &str) -> u32 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in first.bytes().chain([0]).chain(second.bytes()) {
        hash = (hash ^ u32::from(byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

/// How much each word pair is favoured, as a compact sorted table. Pairs that
/// are not in it count as neutral.
#[derive(Debug, Clone, Default)]
pub struct Affinities {
    hashes: Vec<u32>,
    values: Vec<i8>,
}

impl Affinities {
    /// Serializes entries, sorted, keeping the first one given for each hash.
    #[must_use]
    pub fn to_bytes(mut entries: Vec<(u32, i8)>) -> Vec<u8> {
        entries.sort_by_key(|entry| entry.0);
        entries.dedup_by_key(|entry| entry.0);
        let mut bytes = Vec::with_capacity(entries.len() * ENTRY_BYTES);
        for (hash, value) in entries {
            bytes.extend_from_slice(&hash.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    /// Reads a table. A truncated final entry is ignored.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut table = Self::default();
        for entry in bytes.as_chunks::<ENTRY_BYTES>().0 {
            let [a, b, c, d, value] = *entry;
            table.hashes.push(u32::from_le_bytes([a, b, c, d]));
            table.values.push(i8::from_le_bytes([value]));
        }
        table
    }

    fn pair(&self, first: &str, second: &str) -> f32 {
        let hash = pair_hash(first, second);
        let value = self
            .hashes
            .binary_search(&hash)
            .ok()
            .and_then(|index| self.values.get(index))
            .copied()
            .unwrap_or(0);
        f32::from(value) / STEPS_PER_NAT as f32
    }

    /// The context score of a lexicon word after `previous`. An elided word
    /// such as `c'est` is scored through its two tokens.
    #[must_use]
    pub fn score(&self, previous: &str, word: &str) -> f32 {
        let mut before = previous.to_owned();
        let mut total = 0.0;
        for token in tokens(word) {
            total += self.pair(&before, &token);
            before = token;
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elided_words_end_on_their_apostrophe() {
        assert_eq!(
            tokens("J'ai dit : c’est peut-être l'été."),
            ["j'", "ai", "dit", "c'", "est", "peut-être", "l'", "été"]
        );
    }

    const COUNTS: &str = "je 1000\nle 2000\nsuis 400\nsous 600\nc' 500\nest 900\nje suis 300\nle sous 30\nc' est 450\n";

    fn table(counts: &str) -> Affinities {
        let mut entries = Vec::new();
        Bigrams::parse(counts).compile_into(1, 0.0, &mut entries);
        Affinities::from_bytes(&Affinities::to_bytes(entries))
    }

    #[test]
    fn the_previous_word_decides_between_two_words() {
        let model = table(COUNTS);
        assert!(model.score("je", "suis") > model.score("je", "sous"));
    }

    #[test]
    fn words_without_evidence_are_neutral() {
        let model = table(COUNTS);
        assert!(model.score("je", "inconnu").abs() < f32::EPSILON);
        assert!(model.score("inconnu", "suis").abs() < f32::EPSILON);
    }

    #[test]
    fn elided_words_use_their_tokens() {
        assert!(table(COUNTS).score("<s>", "c'est") > 0.0 || table(COUNTS).score("je", "c'est") > 0.0);
    }

    #[test]
    fn scores_stay_bounded_and_a_truncated_table_is_safe() {
        let model = table("a 2\nb 2\nc 100000000\na b 2000\n");
        assert!(model.score("a", "b") <= MAX_NATS as f32);
        let bytes = [1, 2, 3, 4, 5, 6, 7];
        assert_eq!(Affinities::from_bytes(&bytes).hashes.len(), 1);
    }
}
