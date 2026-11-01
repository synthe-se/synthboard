//! The emoji catalogue and its search. French and English keywords are searched
//! together, without accents, and nothing leaves the device.

use synthswipe::plain_letters;

/// The skin tones of Unicode's emoji modifiers, which come first among an item's variants.
pub const SKIN_TONES: usize = 5;
/// Several rows of the search strip: more than anyone scrolls through.
const MAX_RESULTS: usize = 32;
/// An emoji's first words are its names, French then English, keywords after.
const NAMES: usize = 2;
/// How a match ranks, best first.
const EXACT_NAME: u8 = 0;
const NAME_PREFIX: u8 = 1;
const EXACT_KEYWORD: u8 = 2;
const KEYWORD_PREFIX: u8 = 3;

crate::wire_struct! {
    /// One emoji with its skin tone variants.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct EmojiItem {
        pub emoji: String,
        pub tones: Vec<String>,
    }
}

crate::wire_struct! {
    /// One category, in display order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct EmojiGroup {
        pub name: String,
        pub items: Vec<EmojiItem>,
    }
}

#[derive(Debug, Clone)]
struct Indexed {
    emoji: String,
    /// Folded names and keywords, names first.
    words: Vec<String>,
}

/// All emoji, searchable.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub groups: Vec<EmojiGroup>,
    index: Vec<Indexed>,
}

fn plain(text: &str) -> String {
    plain_letters(text).collect()
}

impl Catalog {
    /// Reads `group TAB emoji TAB tones TAB keywords` lines, skipping bad ones.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut catalog = Self::default();
        for line in text.lines() {
            let mut fields = line.split('\t');
            let (Some(group), Some(emoji), Some(tones), Some(words)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if catalog.groups.last().is_none_or(|last| last.name != group) {
                catalog.groups.push(EmojiGroup { name: group.to_owned(), items: Vec::new() });
            }
            if let Some(last) = catalog.groups.last_mut() {
                last.items.push(EmojiItem {
                    emoji: emoji.to_owned(),
                    tones: tones.split_whitespace().map(str::to_owned).collect(),
                });
            }
            catalog.index.push(Indexed {
                emoji: emoji.to_owned(),
                words: words.split('|').map(plain).collect(),
            });
        }
        catalog
    }

    /// Finds emoji whose keywords start with every term of `query`, best first:
    /// exact name, name prefix, exact keyword, then keyword prefix.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<String> {
        let query = plain(query);
        let terms: Vec<&str> = query.split_whitespace().collect();
        let Some(first) = terms.first() else {
            return Vec::new();
        };
        let mut found: Vec<(u8, usize, &str)> = self
            .index
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                terms.iter().all(|term| {
                    item.words.iter().any(|keyword| {
                        keyword.starts_with(term)
                            || keyword.split_whitespace().any(|word| word.starts_with(term))
                    })
                })
            })
            .map(|(position, item)| {
                let names = item.words.iter().take(NAMES);
                let rank = if names.clone().any(|name| name == first) {
                    EXACT_NAME
                } else if names.clone().any(|name| name.split_whitespace().any(|word| word.starts_with(first))) {
                    NAME_PREFIX
                } else if item.words.iter().any(|word| word == first) {
                    EXACT_KEYWORD
                } else {
                    KEYWORD_PREFIX
                };
                (rank, position, item.emoji.as_str())
            })
            .collect();
        found.sort_unstable();
        found.truncate(MAX_RESULTS);
        found.into_iter().map(|(_, _, emoji)| emoji.to_owned()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Smileys\t😀\t\tvisage rieur|grinning face|sourire|smile\n\
                          Smileys\t🥲\t\tvisage souriant avec une larme|smiling face with tear|sourire\n\
                          Food\t🍕\t\tpizza|pizza|fromage|cheese\n\
                          Food\t👍\t👍🏻 👍🏽\tpouce vers le haut|thumbs up|bien\n\
                          bad line\n";

    #[test]
    fn groups_keep_their_order_and_tones() {
        let catalog = Catalog::parse(SAMPLE);
        assert_eq!(catalog.groups.len(), 2);
        assert_eq!(catalog.groups[0].items.len(), 2);
        assert_eq!(catalog.groups[1].items[1].tones, ["👍🏻", "👍🏽"]);
    }

    #[test]
    fn french_and_english_both_find_an_emoji_without_accents() {
        let catalog = Catalog::parse(SAMPLE);
        assert_eq!(catalog.search("fromage"), ["🍕"]);
        assert_eq!(catalog.search("cheese"), ["🍕"]);
        assert_eq!(catalog.search("FROMÄGE"), ["🍕"]);
    }

    #[test]
    fn names_rank_before_keywords_and_prefixes_match() {
        let catalog = Catalog::parse(SAMPLE);
        assert_eq!(catalog.search("pizz"), ["🍕"]);
        assert_eq!(catalog.search("sour").first().map(String::as_str), Some("🥲"));
        assert_eq!(catalog.search("visage larme"), ["🥲"]);
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        assert_eq!(Catalog::parse(SAMPLE).search("  "), [] as [std::string::String; 0]);
    }
}
