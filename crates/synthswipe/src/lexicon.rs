use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// One word of a lexicon.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The lowercase word as it is written.
    pub surface: Box<str>,
    /// The letters a finger crosses, from [`fold`], as ids into the alphabet.
    pub(crate) letters: Box<[u16]>,
    /// Log-scale likelihood on a 0 to 255 scale: higher means more common.
    pub weight: f32,
    /// Bit set of the languages this word belongs to.
    pub languages: u8,
}

const NO_NODE: u32 = 0;
const APOSTROPHE: char = '\'';

/// A trie node. Children are chained through `next_sibling`, and index 0 is
/// the root, which is nobody's child, so 0 can mean "none".
#[derive(Debug, Clone, Copy, Default)]
struct Node {
    letter: u16,
    next_sibling: u32,
    first_child: u32,
    entries_start: u32,
    entries_end: u32,
}

/// Words stored in a trie over their key letters. Words sharing keys, such as
/// `ou` and `où`, end on the same node.
#[derive(Debug, Clone)]
pub struct Lexicon {
    entries: Vec<Entry>,
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
fn apostrophe_path(word: &str) -> Option<Vec<char>> {
    let mut keys = keys_of(word, true);
    keys.dedup();
    keys.contains(&APOSTROPHE).then_some(keys)
}

fn keys_of(word: &str, keep_apostrophe: bool) -> Vec<char> {
    word
        .nfd()
        .filter(|&letter| !is_combining_mark(letter))
        .flat_map(char::to_lowercase)
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
        let mut weights: HashMap<String, (f32, u8)> = HashMap::new();
        for (word, weight, languages) in words {
            let best = weights.entry(word.to_lowercase()).or_insert((weight, 0));
            best.0 = best.0.max(weight);
            best.1 |= languages;
        }
        let mut folded: Vec<(Vec<char>, String, f32, u8)> = weights
            .into_iter()
            .map(|(surface, (weight, languages))| (fold(&surface), surface, weight, languages))
            .filter(|(keys, ..)| !keys.is_empty())
            .collect();
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
        let entries: Vec<Entry> = folded
            .into_iter()
            .map(|(keys, surface, weight, languages)| Entry {
                surface: surface.into_boxed_str(),
                letters: keys.iter().map(letter_id).collect(),
                weight,
                languages,
            })
            .collect();
        let nodes = build_trie(&entries);
        Self {
            entries,
            nodes,
            alphabet,
        }
    }

    /// Number of words.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the lexicon holds no words.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `word`, lowercased, is in the lexicon.
    #[must_use]
    pub fn contains(&self, word: &str) -> bool {
        self.find(word).is_some()
    }

    /// The languages of `word`, or 0 if it is unknown or belongs to none.
    #[must_use]
    pub fn languages(&self, word: &str) -> u8 {
        self.find(word).map_or(0, |entry| entry.languages)
    }

    fn find(&self, word: &str) -> Option<&Entry> {
        let lower = word.to_lowercase().replace('’', "'");
        let mut node = self.nodes.first();
        for key in fold(&lower) {
            let letter = self.alphabet.binary_search(&key).ok();
            node = node.and_then(|parent| {
                self.children(parent)
                    .find(|child| Some(usize::from(child.letter)) == letter)
            });
        }
        node.and_then(|node| self.entries_of(node).iter().find(|e| *e.surface == *lower))
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

    fn node(&self, index: u32) -> Option<&Node> {
        if index == NO_NODE {
            None
        } else {
            self.nodes.get(index as usize)
        }
    }

    fn entries_of(&self, node: &Node) -> &[Entry] {
        self.entries
            .get(node.entries_start as usize..node.entries_end as usize)
            .unwrap_or_default()
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
        mut visit: impl FnMut(&'a Entry, f32, u8),
    ) {
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
                    .iter()
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
            if step.next_tap > 0 {
                if let Some(extra_cost) = cost(step.node.letter, step.next_tap) {
                    let spent = step.spent + extra_cost;
                    if spent <= max_cost {
                        stack.push(TapStep { next_tap: step.next_tap + 1, spent, ..step });
                    }
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
        mut visit: impl FnMut(&'a Entry),
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
                    self.entries_of(child).iter().for_each(&mut visit);
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

fn build_trie(entries: &[Entry]) -> Vec<Node> {
    let mut nodes = vec![Node::default()];
    let mut path: Vec<u32> = vec![0];
    let mut previous: &[u16] = &[];
    for (index, entry) in (0_u32..).zip(entries) {
        let shared = shared_prefix(previous, &entry.letters);
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
            if node.entries_end == node.entries_start {
                node.entries_start = index;
            }
            node.entries_end = index + 1;
        }
        previous = &entry.letters;
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
