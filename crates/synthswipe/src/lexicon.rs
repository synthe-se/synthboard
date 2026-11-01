use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The listings of one word, whatever their case.
#[derive(Debug, Default)]
struct Merged {
    lower: f32,
    capital: Option<(f32, String)>,
    languages: u8,
}

impl Merged {
    fn add(&mut self, word: &str, weight: f32, languages: u8) {
        self.languages |= languages;
        if word.chars().any(char::is_uppercase) {
            if self.capital.as_ref().is_none_or(|(best, _)| weight > *best) {
                self.capital = Some((weight, word.to_owned()));
            }
        } else {
            self.lower = self.lower.max(weight);
        }
    }

    fn weight(&self) -> f32 {
        self.capital.as_ref().map_or(self.lower, |(weight, _)| weight.max(self.lower))
    }

    fn surface(&self, lower: &str) -> String {
        match &self.capital {
            Some((weight, form))
                if form.chars().nth(1).is_some() && (self.lower == 0.0 || *weight >= self.lower + CAPITAL_LEAD) =>
            {
                form.clone()
            }
            _ => lower.to_owned(),
        }
    }
}

/// One word of a lexicon, read from its compact storage.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'lexicon> {
    /// The lowercase word as it is written.
    pub surface: &'lexicon str,
    /// The letters a finger crosses, from [`fold`], as ids into the alphabet.
    pub(crate) letters: &'lexicon [u16],
    /// Log-scale likelihood on a 0 to 255 scale: higher means more common.
    pub weight: f32,
    /// Bit set of the languages this word belongs to.
    pub languages: u8,
}

const NO_NODE: u32 = 0;
pub(crate) const APOSTROPHE: char = '\'';
/// How much more a capitalised listing must weigh to become the written form.
const CAPITAL_LEAD: f32 = 16.0;

/// A trie node. Children are chained through `next_sibling`, and index 0 is
/// the root, which is nobody's child, so 0 can mean "none".
#[derive(Debug, Clone, Copy, Default)]
struct Node {
    letter: u16,
    entries_len: u16,
    next_sibling: u32,
    first_child: u32,
    entries_start: u32,
}

/// Words stored in a trie over their key letters. Words sharing keys, such as
/// `ou` and `où`, end on the same node.
///
/// Every word lives in a few flat arrays rather than in its own allocations:
/// hundreds of thousands of small allocations cost a phone far more than the
/// text itself, and a flat layout is also faster to walk.
#[derive(Debug, Clone)]
pub struct Lexicon {
    surfaces: String,
    surface_ends: Vec<u32>,
    letters: Vec<u16>,
    letter_ends: Vec<u32>,
    weights: Vec<u8>,
    languages: Vec<u8>,
    words: usize,
    nodes: Vec<Node>,
    alphabet: Vec<char>,
}

/// The key letters of a word, doubles kept: lowercase, no accents, ligatures
/// expanded, no apostrophes or hyphens.
pub(crate) fn raw_keys(word: &str) -> Vec<char> {
    keys_of(word, false)
}

/// The keys of a word for someone who swipes through the apostrophe key, which
/// is the spelling exactly: `c'est` is `c ' e s t`. Words without one have none.
pub(crate) fn apostrophe_path(word: &str) -> Option<Vec<char>> {
    let mut keys = keys_of(word, true);
    keys.dedup();
    keys.contains(&APOSTROPHE).then_some(keys)
}

/// A text's letters in lowercase, without their accents.
pub fn plain_letters(text: &str) -> impl Iterator<Item = char> {
    text.nfd().filter(|&letter| !is_combining_mark(letter)).flat_map(char::to_lowercase)
}

fn keys_of(word: &str, keep_apostrophe: bool) -> Vec<char> {
    plain_letters(word)
        .flat_map(|letter| match letter {
            'œ' => ['o', 'e'],
            'æ' => ['a', 'e'],
            'ß' => ['s', 's'],
            // The NUL filler is dropped by the alphanumeric filter below.
            _ => [letter, '\0'],
        })
        .map(|letter| if letter == '’' { APOSTROPHE } else { letter })
        .filter(|&letter| letter.is_alphanumeric() || (keep_apostrophe && letter == APOSTROPHE))
        .collect()
}

/// Reduces a word to the key letters its gesture crosses, doubles collapsed.
#[must_use]
pub fn fold(word: &str) -> Vec<char> {
    let mut keys = raw_keys(word);
    keys.dedup();
    keys
}

/// The range an array of ends gives to one item of a flat array.
pub(crate) fn span(ends: &[u32], index: usize) -> Option<std::ops::Range<usize>> {
    let start = match index.checked_sub(1) {
        Some(previous) => *ends.get(previous)? as usize,
        None => 0,
    };
    Some(start..*ends.get(index)? as usize)
}

fn shared_prefix(a: &[u16], b: &[u16]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

impl Lexicon {
    /// Builds a lexicon from `(word, weight)` pairs. Words are lowercased, a
    /// repeated word keeps its highest weight, and words with no key are skipped.
    #[must_use]
    pub fn new<'w>(words: impl IntoIterator<Item = (&'w str, f32)>) -> Self {
        Self::with_languages(words.into_iter().map(|(word, weight)| (word, weight, 0)))
    }

    /// Builds a lexicon from `(word, weight, languages)` triples. A word
    /// repeated across languages keeps its best weight and all its languages.
    #[must_use]
    pub fn with_languages<'w>(words: impl IntoIterator<Item = (&'w str, f32, u8)>) -> Self {
        // Word lists hold both `Paris` the city and `paris` the bets: the form
        // used far more often is the one the word is written with.
        let mut merged: HashMap<String, Merged> = HashMap::new();
        for (word, weight, languages) in words {
            merged.entry(word.to_lowercase()).or_default().add(word, weight, languages);
        }
        let mut folded: Vec<(Vec<char>, String, f32, u8)> = merged
            .into_iter()
            .map(|(lower, listed)| {
                let surface = listed.surface(&lower);
                (fold(&lower), surface, listed.weight(), listed.languages)
            })
            .filter(|(keys, ..)| !keys.is_empty())
            .collect();
        let words = folded.len();
        let through_apostrophe: Vec<(Vec<char>, String, f32, u8)> = folded
            .iter()
            .filter_map(|(_, surface, weight, languages)| {
                Some((apostrophe_path(surface)?, surface.clone(), *weight, *languages))
            })
            .collect();
        folded.extend(through_apostrophe);
        folded.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        folded.truncate(u32::MAX as usize);

        let mut alphabet: Vec<char> = folded.iter().flat_map(|e| e.0.iter().copied()).collect();
        alphabet.sort_unstable();
        alphabet.dedup();
        alphabet.truncate(usize::from(u16::MAX));

        let letter_id = |key: &char| {
            alphabet
                .binary_search(key)
                .ok()
                .and_then(|id| u16::try_from(id).ok())
                .unwrap_or(u16::MAX)
        };
        let mut lexicon = Self {
            surfaces: String::new(),
            surface_ends: Vec::with_capacity(folded.len()),
            letters: Vec::new(),
            letter_ends: Vec::with_capacity(folded.len()),
            weights: Vec::with_capacity(folded.len()),
            languages: Vec::with_capacity(folded.len()),
            words,
            nodes: Vec::new(),
            alphabet: Vec::new(),
        };
        for (keys, surface, weight, languages) in folded {
            lexicon.surfaces.push_str(&surface);
            lexicon.surface_ends.push(u32::try_from(lexicon.surfaces.len()).unwrap_or(u32::MAX));
            lexicon.letters.extend(keys.iter().map(letter_id));
            lexicon.letter_ends.push(u32::try_from(lexicon.letters.len()).unwrap_or(u32::MAX));
            lexicon.weights.push(weight.clamp(0.0, f32::from(u8::MAX)).round() as u8);
            lexicon.languages.push(languages);
        }
        lexicon.alphabet = alphabet;
        lexicon.nodes = breadth_first(&build_trie(&lexicon));
        lexicon.surfaces.shrink_to_fit();
        lexicon.letters.shrink_to_fit();
        lexicon
    }

    /// Builds a lexicon from `word,weight,languages` lines, skipping bad ones.
    #[must_use]
    pub fn from_csv(text: &str) -> Self {
        Self::with_languages(text.lines().filter_map(|line| {
            let (rest, languages) = line.rsplit_once(',')?;
            let (word, weight) = rest.rsplit_once(',')?;
            Some((word, weight.parse::<f32>().ok()?, languages.parse::<u8>().ok()?))
        }))
    }

    /// Number of words.
    #[must_use]
    pub fn len(&self) -> usize {
        self.words
    }

    /// Whether the lexicon holds no words.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words == 0
    }

    /// Whether `word`, lowercased, is in the lexicon.
    #[must_use]
    pub fn contains(&self, word: &str) -> bool {
        self.find(word).is_some()
    }

    /// How `word` is written here, if it is known: `paris` can be `Paris`.
    #[must_use]
    pub fn surface(&self, word: &str) -> Option<&str> {
        self.find(word).map(|entry| entry.surface)
    }

    /// The languages of `word`, or 0 if it is unknown or belongs to none.
    #[must_use]
    pub fn languages(&self, word: &str) -> u8 {
        self.find(word).map_or(0, |entry| entry.languages)
    }

    fn find(&self, word: &str) -> Option<Entry<'_>> {
        let lower = word.to_lowercase().replace('’', "'");
        let mut node = self.nodes.first();
        for key in fold(&lower) {
            let letter = self.alphabet.binary_search(&key).ok();
            node = node.and_then(|parent| {
                self.children(parent)
                    .find(|child| Some(usize::from(child.letter)) == letter)
            });
        }
        node.and_then(|node| self.entries_of(node).find(|entry| entry.surface.to_lowercase() == lower))
    }

    fn entry(&self, index: usize) -> Option<Entry<'_>> {
        Some(Entry {
            surface: self.surfaces.get(span(&self.surface_ends, index)?)?,
            letters: self.letters.get(span(&self.letter_ends, index)?)?,
            weight: f32::from(*self.weights.get(index)?),
            languages: *self.languages.get(index)?,
        })
    }

    /// The letters of the trie, indexed by the ids used in `reachable`.
    pub(crate) fn alphabet(&self) -> &[char] {
        &self.alphabet
    }

    fn children(&self, parent: &Node) -> impl Iterator<Item = &Node> {
        std::iter::successors(self.node(parent.first_child), |node| {
            self.node(node.next_sibling)
        })
    }

    /// The children of a node as `(node, letter)`, for a walk that keeps nodes by number.
    pub(crate) fn child_ids(&self, parent: u32) -> impl Iterator<Item = (u32, u16)> + '_ {
        let first = self.nodes.get(parent as usize).map_or(NO_NODE, |node| node.first_child);
        std::iter::successors(self.node(first).map(|node| (first, node)), |(_, node)| {
            self.node(node.next_sibling).map(|next| (node.next_sibling, next))
        })
        .map(|(index, node)| (index, node.letter))
    }

    /// The words ending at a node given by number.
    pub(crate) fn entries_at(&self, node: u32) -> impl Iterator<Item = Entry<'_>> {
        self.nodes.get(node as usize).into_iter().flat_map(|node| self.entries_of(node))
    }

    /// The weight of a word, or 0 if it is unknown.
    #[must_use]
    pub fn weight(&self, word: &str) -> f32 {
        self.find(word).map_or(0.0, |entry| entry.weight)
    }

    fn node(&self, index: u32) -> Option<&Node> {
        if index == NO_NODE {
            None
        } else {
            self.nodes.get(index as usize)
        }
    }

    fn entries_of(&self, node: &Node) -> impl Iterator<Item = Entry<'_>> {
        let start = node.entries_start as usize;
        (start..start + usize::from(node.entries_len)).filter_map(|index| self.entry(index))
    }

    /// Visits the words the taps may spell, with their spatial cost and how
    /// many letters they continue past the last tap. `cost(letter, tap)` is
    /// `None` when that tap cannot mean that letter. A repeated tap may stay on
    /// a letter, since doubles are collapsed in the trie. `budget` bounds the
    /// nodes visited, so a short prefix cannot walk the whole lexicon.
    pub(crate) fn along_taps<'a>(
        &'a self,
        cost: impl Fn(u16, usize) -> Option<f32>,
        taps: usize,
        max_cost: f32,
        budget: usize,
        mut visit: impl FnMut(Entry<'a>, f32, u8),
    ) {
        /// Letters a completion may add: longer ones are rarely the word meant,
        /// and the walk grows fast with them. Tuned by hand.
        const MAX_EXTRA: u8 = 6;
        let Some(root) = self.nodes.first() else {
            return;
        };
        if taps == 0 {
            return;
        }
        let mut stack = vec![TapStep { node: root, next_tap: 0, spent: 0.0, extra: 0 }];
        let mut visited = 0;
        while let Some(step) = stack.pop() {
            visited += 1;
            if visited > budget {
                return;
            }
            if step.next_tap >= taps {
                self.entries_of(step.node)
                    .for_each(|entry| visit(entry, step.spent, step.extra));
                if step.extra < MAX_EXTRA {
                    stack.extend(self.children(step.node).map(|node| TapStep {
                        node,
                        extra: step.extra + 1,
                        ..step
                    }));
                }
                continue;
            }
            if step.next_tap > 0
                && let Some(extra_cost) = cost(step.node.letter, step.next_tap) {
                    let spent = step.spent + extra_cost;
                    if spent <= max_cost {
                        stack.push(TapStep { next_tap: step.next_tap + 1, spent, ..step });
                    }
                }
            for node in self.children(step.node) {
                if let Some(extra_cost) = cost(node.letter, step.next_tap) {
                    let spent = step.spent + extra_cost;
                    if spent <= max_cost {
                        stack.push(TapStep { node, next_tap: step.next_tap + 1, spent, extra: 0 });
                    }
                }
            }
        }
    }

    /// Visits every word whose letters can be crossed in order.
    ///
    /// `next(letter, position)` gives the first position at or after
    /// `position` where `letter` is crossed. Whole subtrees are skipped as soon
    /// as a letter cannot be reached, so the cost follows the gesture rather
    /// than the size of the lexicon.
    pub(crate) fn reachable<'a>(
        &'a self,
        next: impl Fn(u16, u16) -> Option<u16>,
        is_start: impl Fn(u16) -> bool,
        is_end: impl Fn(u16) -> bool,
        mut visit: impl FnMut(Entry<'a>),
    ) {
        let mut stack: Vec<(&Node, u16, bool)> = Vec::new();
        if let Some(root) = self.nodes.first() {
            stack.push((root, 0, true));
        }
        while let Some((parent, position, is_root)) = stack.pop() {
            for child in self.children(parent) {
                if is_root && !is_start(child.letter) {
                    continue;
                }
                let Some(reached) = next(child.letter, position) else {
                    continue;
                };
                if is_end(child.letter) {
                    self.entries_of(child).for_each(&mut visit);
                }
                if child.first_child != NO_NODE {
                    stack.push((child, reached, false));
                }
            }
        }
    }
}

/// One step of the walk along taps.
#[derive(Clone, Copy)]
struct TapStep<'a> {
    node: &'a Node,
    next_tap: usize,
    spent: f32,
    extra: u8,
}

/// Renumbers a trie so that the children of a node sit side by side. Walking them
/// is the hot loop of every search, and scattered children mean a cache miss each.
fn breadth_first(nodes: &[Node]) -> Vec<Node> {
    let mut sorted = Vec::with_capacity(nodes.len());
    sorted.extend(nodes.first().copied());
    let mut at = 0;
    while let Some(parent) = sorted.get(at).copied() {
        let first = u32::try_from(sorted.len()).unwrap_or(u32::MAX);
        let mut child = parent.first_child;
        let mut count = 0;
        while let Some(node) = (child != NO_NODE).then(|| nodes.get(child as usize).copied()).flatten() {
            child = node.next_sibling;
            count += 1;
            sorted.push(Node { next_sibling: if child == NO_NODE { NO_NODE } else { first + count }, ..node });
        }
        if let Some(moved) = sorted.get_mut(at) {
            moved.first_child = if count == 0 { NO_NODE } else { first };
        }
        at += 1;
    }
    sorted
}

fn build_trie(lexicon: &Lexicon) -> Vec<Node> {
    let mut nodes = vec![Node::default()];
    let mut path: Vec<u32> = vec![0];
    let mut previous: &[u16] = &[];
    for index in 0..lexicon.surface_ends.len() {
        let Some(entry) = lexicon.entry(index) else {
            break;
        };
        let position = u32::try_from(index).unwrap_or(u32::MAX);
        let shared = shared_prefix(previous, entry.letters);
        let mut older_sibling = path.get(shared + 1).copied();
        path.truncate(shared + 1);
        for &letter in entry.letters.iter().skip(shared) {
            let Ok(new_index) = u32::try_from(nodes.len()) else {
                return nodes;
            };
            nodes.push(Node {
                letter,
                ..Node::default()
            });
            let link = match older_sibling.take() {
                Some(sibling) => nodes.get_mut(sibling as usize).map(|n| &mut n.next_sibling),
                None => path
                    .last()
                    .and_then(|&parent| nodes.get_mut(parent as usize))
                    .map(|n| &mut n.first_child),
            };
            if let Some(link) = link {
                *link = new_index;
            }
            path.push(new_index);
        }
        if let Some(node) = path.last().and_then(|&last| nodes.get_mut(last as usize)) {
            if node.entries_len == 0 {
                node.entries_start = position;
            }
            node.entries_len = node.entries_len.saturating_add(1);
        }
        previous = entry.letters;
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_matches_what_the_finger_crosses() {
        assert_eq!(fold("Été"), ['e', 't', 'e']);
        assert_eq!(fold("l'été"), ['l', 'e', 't', 'e']);
        assert_eq!(fold("peut-être"), ['p', 'e', 'u', 't', 'e', 't', 'r', 'e']);
        assert_eq!(fold("cœur"), ['c', 'o', 'e', 'u', 'r']);
        assert_eq!(fold("balle"), ['b', 'a', 'l', 'e']);
        assert_eq!(fold("æon"), ['a', 'e', 'o', 'n']);
        assert_eq!(fold("Maß"), ['m', 'a', 's']);
    }

    #[test]
    fn a_word_is_found_in_the_way_the_lexicon_writes_it() {
        let lexicon = Lexicon::new([("Paris", 150.0), ("bon", 100.0)]);
        assert_eq!(lexicon.surface("paris"), Some("Paris"));
        assert_eq!(lexicon.surface("BON"), Some("bon"));
        assert_eq!(lexicon.surface("londres"), None);
    }

    #[test]
    fn a_lexicon_with_words_is_not_empty() {
        assert!(!Lexicon::new([("bon", 1.0)]).is_empty());
    }

    #[test]
    fn an_elided_word_has_both_paths_and_counts_once() {
        let lexicon = Lexicon::new([("c'est", 150.0), ("bon", 100.0)]);
        assert_eq!(apostrophe_path("c'est"), Some(vec!['c', '\'', 'e', 's', 't']));
        assert_eq!(apostrophe_path("bon"), None);
        assert!(lexicon.contains("c'est"));
        assert!(lexicon.contains("C’est"));
        assert_eq!(lexicon.languages("c'est"), 0);
    }

    #[test]
    fn words_read_back_exactly_from_flat_storage() {
        let lexicon = Lexicon::with_languages([("été", 140.0, 1), ("merge", 113.0, 2), ("c'est", 159.0, 1)]);
        assert_eq!(lexicon.len(), 3);
        let surfaces: Vec<&str> = (0..4).filter_map(|i| lexicon.entry(i)).map(|e| e.surface).collect();
        assert_eq!(surfaces.iter().filter(|word| **word == "c'est").count(), 2);
        assert!(surfaces.contains(&"été") && surfaces.contains(&"merge"));
        assert_eq!(lexicon.find("merge").map(|e| (e.weight, e.languages)), Some((113.0, 2)));
        assert!(lexicon.entry(4).is_none());
    }

    #[test]
    fn a_name_keeps_its_capitals_unless_it_is_also_a_plain_word() {
        let lexicon = Lexicon::new([
            ("Francis", 100.0),
            ("GitHub", 100.0),
            ("Paris", 150.0),
            ("paris", 100.0),
            ("Pierre", 104.0),
            ("pierre", 100.0),
            ("M", 150.0),
            ("m", 100.0),
        ]);
        let surface = |word| lexicon.find(word).map(|entry| entry.surface);
        assert_eq!(surface("francis"), Some("Francis"));
        assert_eq!(surface("GITHUB"), Some("GitHub"));
        assert_eq!(surface("paris"), Some("Paris"));
        assert_eq!(surface("Pierre"), Some("pierre"));
        assert_eq!(surface("m"), Some("m"));
    }

    #[test]
    fn the_heaviest_capitalised_form_wins_in_any_order() {
        for words in [[("Github", 100.0), ("GitHub", 120.0)], [("GitHub", 120.0), ("Github", 100.0)]] {
            let lexicon = Lexicon::new(words);
            assert_eq!(lexicon.find("github").map(|entry| entry.surface), Some("GitHub"));
        }
    }

    #[test]
    fn homographs_stay_distinct_words_on_the_same_keys() {
        let lexicon = Lexicon::new([("ou", 9.0), ("où", 8.0), ("OU", 1.0)]);
        assert_eq!(lexicon.len(), 2);
        assert!(lexicon.contains("où"));
        assert!(lexicon.contains("Ou"));
        assert!(!lexicon.contains("oui"));
        assert!(!lexicon.contains("o"));
    }

    #[test]
    fn prefixes_and_longer_words_coexist() {
        let lexicon = Lexicon::new([("bon", 1.0), ("bonjour", 1.0), ("bonsoir", 1.0), ("car", 1.0)]);
        for word in ["bon", "bonjour", "bonsoir", "car"] {
            assert!(lexicon.contains(word), "{word}");
        }
        assert!(!lexicon.contains("bonj"));
    }

    #[test]
    fn a_word_keeps_every_language_it_belongs_to() {
        let lexicon = Lexicon::with_languages([("table", 100.0, 1), ("table", 90.0, 2), ("il", 150.0, 1)]);
        assert_eq!(lexicon.languages("table"), 3);
        assert_eq!(lexicon.languages("il"), 1);
        assert_eq!(lexicon.languages("inconnu"), 0);
    }

    #[test]
    fn words_without_any_key_are_skipped() {
        assert!(Lexicon::new([("'-", 1.0)]).is_empty());
    }

    /// Walks with a cost of 1 for `allowed(letter, tap)`, and collects what is visited.
    fn walked(lexicon: &Lexicon, taps: usize, budget: usize, allowed: impl Fn(char, usize) -> bool) -> Vec<(String, f32, u8)> {
        let letter = |id: u16| lexicon.alphabet().get(usize::from(id)).copied().unwrap_or('?');
        let cost = |id: u16, tap: usize| allowed(letter(id), tap).then_some(1.0);
        let mut found = Vec::new();
        lexicon.along_taps(cost, taps, 100.0, budget, |entry, spent, extra| found.push((entry.surface.to_owned(), spent, extra)));
        found
    }

    #[test]
    fn the_walk_stops_at_its_budget_and_not_before() {
        let lexicon = Lexicon::new([("ab", 1.0)]);
        let spell = |letter: char, tap: usize| (letter, tap) == ('a', 0) || (letter, tap) == ('b', 1);
        assert_eq!(walked(&lexicon, 2, 3, spell).len(), 1);
        assert_eq!(walked(&lexicon, 2, 2, spell).len(), 0);
    }

    #[test]
    fn a_completion_stops_at_six_extra_letters_and_counts_them() {
        let lexicon = Lexicon::new([("abcdefg", 1.0), ("hijklmno", 1.0)]);
        let first = |letter: char, tap: usize| tap == 0 && (letter == 'a' || letter == 'h');
        assert_eq!(walked(&lexicon, 1, 1000, first), [("abcdefg".to_owned(), 1.0, 6)]);
    }

    #[test]
    fn the_first_tap_cannot_stay_on_the_root() {
        let lexicon = Lexicon::new([("a", 1.0), ("b", 1.0)]);
        let allowed = |letter: char, tap: usize| letter == 'a' || (letter == 'b' && tap == 1);
        assert!(walked(&lexicon, 2, 1000, allowed).iter().all(|(word, ..)| word != "b"));
    }

    #[test]
    fn staying_on_a_letter_adds_its_cost() {
        let lexicon = Lexicon::new([("ab", 1.0)]);
        let spell = |letter: char, tap: usize| (letter == 'a' && tap < 2) || (letter == 'b' && tap == 2);
        assert_eq!(walked(&lexicon, 3, 1000, spell), [("ab".to_owned(), 3.0, 0)]);
    }

    #[test]
    fn a_completion_keeps_what_the_taps_cost() {
        let lexicon = Lexicon::new([("abc", 1.0)]);
        let first = |letter: char, tap: usize| letter == 'a' && tap == 0;
        assert_eq!(walked(&lexicon, 1, 1000, first), [("abc".to_owned(), 1.0, 2)]);
    }

    #[test]
    fn the_children_of_a_node_sit_side_by_side() {
        let lexicon = Lexicon::new([("bonjour", 1.0), ("bol", 1.0), ("car", 1.0), ("cat", 1.0), ("dur", 1.0), ("zoo", 1.0)]);
        for (index, node) in lexicon.nodes.iter().enumerate() {
            let ids: Vec<u32> = lexicon.child_ids(u32::try_from(index).unwrap_or(0)).map(|(id, _)| id).collect();
            assert!(ids.iter().zip(ids.iter().skip(1)).all(|(a, b)| *b == a + 1), "{node:?}");
        }
        for word in ["bonjour", "bol", "car", "cat", "dur", "zoo"] {
            assert!(lexicon.contains(word), "{word}");
        }
    }

    #[test]
    fn reachable_prunes_by_order() {
        let lexicon = Lexicon::new([("ab", 1.0), ("ba", 1.0)]);
        let id = |c| lexicon.alphabet().iter().position(|&l| l == c);
        let (a, b) = (id('a'), id('b'));
        // `a` is crossed at position 0 and `b` at position 1, so only "ab" fits.
        let next = |letter: u16, position: u16| {
            let at = if Some(usize::from(letter)) == a {
                0
            } else if Some(usize::from(letter)) == b {
                1
            } else {
                return None;
            };
            (at >= position).then_some(at)
        };
        let mut found = Vec::new();
        lexicon.reachable(next, |_| true, |_| true, |e| found.push(e.surface.to_string()));
        assert_eq!(found, ["ab"]);
    }
}
