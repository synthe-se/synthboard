//! French subject and verb agreement. After `je`, a form of the first person
//! is likely and `sous` is not. This reads the sentence as a sentence, which
//! word pairs cannot do across `je ne suis` or `je me suis`.

//! The word lists and the bits of a mask are data: see `grammar.tsv` in the assets.

/// Marks a rule among the form lines.
const RULE_MARK: char = '@';
const SEPARATOR: char = '\t';
const WORD_SEPARATOR: char = ' ';
/// Refuses every known word that is not wanted, whatever its mask.
const EVERY_FORM: u8 = u8::MAX;

/// Reward for a verb that agrees, and cost of a known word that cannot follow.
const AGREES: f32 = 1.0;
const DISAGREES: f32 = -1.0;
/// Reward for a verb where a noun fits as well: `je veux la porter`. A full reward
/// would still turn `porte`, weight 151, into `porter`, 132, which is 0.38 nat
/// apart at `lambda`. Tuned by hand to stay under that, with `GRAMMAR_WEIGHT`.
const MAY_AGREE: f32 = 0.2;

/// A sorted list of words: a few dozen, searched for every finalist of a gesture.
#[derive(Debug, Clone, Default)]
struct Words(Vec<Box<str>>);

impl Words {
    fn add(&mut self, words: &str) {
        self.0.extend(words.split(WORD_SEPARATOR).filter(|word| !word.is_empty()).map(Into::into));
        self.0.sort_unstable();
        self.0.dedup();
    }

    fn contains(&self, word: &str) -> bool {
        self.0.binary_search_by(|known| (**known).cmp(word)).is_ok()
    }
}

/// What the language data says. Empty, as without the asset, it says nothing.
#[derive(Debug, Clone, Default)]
struct Rules {
    bits: Vec<(Box<str>, u8)>,
    conjugated: u8,
    infinitive: u8,
    participle: u8,
    subjects: Vec<(Box<str>, u8)>,
    clitics: Words,
    adverbs: Words,
    modals: Words,
    plain_modals: Words,
    auxiliaries: Words,
    reflexives: Words,
    articles: Words,
    prepositions: Words,
    also_after: Vec<(Box<str>, Words)>,
    abbreviations: Words,
    capitals: Words,
}

impl Rules {
    /// Reads one rule, without its mark. An unknown or bad rule is skipped.
    fn read(&mut self, rule: &str) {
        let mut fields = rule.split(SEPARATOR);
        let (Some(name), Some(first)) = (fields.next(), fields.next()) else {
            return;
        };
        match (name, fields.next()) {
            ("bit", Some(value)) => self.read_bit(first, value),
            ("subject", Some(words)) => {
                if let Some(&(_, bit)) = self.bits.iter().find(|(known, _)| &**known == first) {
                    self.subjects.extend(words.split(WORD_SEPARATOR).map(|word| (word.into(), bit)));
                }
            }
            ("also_after", Some(words)) => {
                let mut allowed = Words::default();
                allowed.add(words);
                self.also_after.push((first.into(), allowed));
            }
            ("clitic", None) => self.clitics.add(first),
            ("adverb", None) => self.adverbs.add(first),
            ("modal", None) => self.modals.add(first),
            ("plain_modal", None) => self.plain_modals.add(first),
            ("auxiliary", None) => self.auxiliaries.add(first),
            ("reflexive", None) => self.reflexives.add(first),
            ("article", None) => self.articles.add(first),
            ("preposition", None) => self.prepositions.add(first),
            ("abbreviation", None) => self.abbreviations.add(first),
            ("capital", None) => self.capitals.add(first),
            _ => {}
        }
    }

    fn read_bit(&mut self, name: &str, value: &str) {
        let Ok(bit) = value.parse() else {
            return;
        };
        match name {
            "infinitive" => self.infinitive = bit,
            "participle" => self.participle = bit,
            _ => {
                self.conjugated |= bit;
                self.bits.push((name.into(), bit));
            }
        }
    }

    fn person_of(&self, token: &str) -> Option<u8> {
        self.subjects.iter().find(|(word, _)| &**word == token).map(|&(_, bit)| bit)
    }

    /// The subject of the verb about to be written, when `history` ends on one.
    /// `nous` and `vous` can be objects, so the subject furthest back wins, and
    /// after a preposition no pronoun is one: `merci pour vous`.
    fn subject(&self, history: &[String]) -> Option<u8> {
        let mut found = None;
        let mut tokens = history.iter().rev().map(String::as_str).peekable();
        while let Some(token) = tokens.next() {
            let governed = tokens.peek().is_some_and(|before| self.prepositions.contains(before));
            match self.person_of(token) {
                Some(_) if governed => return found,
                Some(person) => {
                    found = Some(person);
                    if !self.clitics.contains(token) {
                        return found;
                    }
                }
                None if self.clitics.contains(token) => {}
                None => return found,
            }
        }
        found
    }

    /// Which verb forms `history` calls for, and which it rules out.
    fn expected_forms(&self, history: &[String]) -> Option<(u8, u8)> {
        let last = history.last()?.as_str();
        if self.reflexives.contains(last) {
            return Some((self.conjugated | self.infinitive, EVERY_FORM));
        }
        let verb = history.iter().rev().map(String::as_str).find(|token| !self.adverbs.contains(token))?;
        if self.auxiliaries.contains(verb) {
            return Some((self.participle, self.infinitive));
        }
        let after_clitics = history
            .iter()
            .rev()
            .map(String::as_str)
            .find(|token| !self.adverbs.contains(token) && !self.clitics.contains(token))?;
        if !self.modals.contains(after_clitics) || (self.plain_modals.contains(after_clitics) && self.adverbs.contains(last)) {
            return None;
        }
        // `je veux la porte`: after an article a noun is as good as a verb, and
        // a noun is often also a conjugated form.
        let refused = if self.articles.contains(last) { 0 } else { self.conjugated | self.participle };
        Some((self.infinitive, refused))
    }

    fn admits(&self, last: &str, word: &str) -> bool {
        self.also_after.iter().any(|(token, words)| &**token == last && words.contains(word))
    }
}

/// French word forms and the persons they conjugate, sorted in flat arrays so
/// that a hundred thousand forms stay one allocation, and the language's rules.
#[derive(Debug, Clone, Default)]
pub struct Grammar {
    text: String,
    ends: Vec<u32>,
    masks: Vec<u8>,
    rules: Rules,
}

impl Grammar {
    /// Reads `form TAB mask` lines and `@rule` lines, skipping bad ones.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut grammar = Self::default();
        let mut forms: Vec<(&str, u8)> = Vec::new();
        for line in text.lines() {
            if let Some(rule) = line.strip_prefix(RULE_MARK) {
                grammar.rules.read(rule);
            } else if let Some((form, mask)) = line.split_once(SEPARATOR)
                && let Ok(mask) = mask.parse()
            {
                forms.push((form, mask));
            }
        }
        forms.sort_unstable();
        for (form, mask) in forms {
            grammar.text.push_str(form);
            grammar.ends.push(u32::try_from(grammar.text.len()).unwrap_or(u32::MAX));
            grammar.masks.push(mask);
        }
        grammar
    }

    fn form(&self, index: usize) -> &str {
        let start = index.checked_sub(1).and_then(|i| self.ends.get(i)).map_or(0, |&end| end as usize);
        let end = self.ends.get(index).map_or(start, |&end| end as usize);
        self.text.get(start..end).unwrap_or("")
    }

    fn mask(&self, form: &str) -> Option<u8> {
        let (mut low, mut high) = (0, self.ends.len());
        while low < high {
            let middle = low + (high - low) / 2;
            match self.form(middle).cmp(form) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return self.masks.get(middle).copied(),
            }
        }
        None
    }

    /// How well `word` fits after `history`, from -1 to 1. Without a subject,
    /// or for a word French does not know, such as `merge`, it is neutral.
    #[must_use]
    pub fn score(&self, history: &[String], word: &str) -> f32 {
        let lower = word.to_lowercase();
        let rules = &self.rules;
        let free = rules.clitics.contains(&lower) || rules.adverbs.contains(&lower);
        if history.last().is_some_and(|last| rules.admits(last, &lower)) {
            return AGREES;
        }
        if let Some((wanted, refused)) = rules.expected_forms(history)
            && !free
            && let Some(mask) = self.mask(&lower)
        {
            if mask & wanted != 0 {
                return if refused == 0 { MAY_AGREE } else { AGREES };
            }
            if mask & refused != 0 || refused == EVERY_FORM {
                return DISAGREES;
            }
        }
        let Some(person) = rules.subject(history) else {
            return 0.0;
        };
        // `je vous vois`: a clitic is as welcome after a subject as a verb.
        if rules.clitics.contains(&lower) {
            return AGREES;
        }
        // `elle aussi`: an adverb says nothing of the subject, whatever else it can be.
        if rules.adverbs.contains(&lower) {
            return 0.0;
        }
        match self.mask(&lower) {
            Some(mask) if mask & person != 0 => AGREES,
            Some(mask) if mask & rules.conjugated != 0 => DISAGREES,
            _ => 0.0,
        }
    }

    /// Whether this word's last dot leaves the sentence open.
    #[must_use]
    pub fn is_abbreviation(&self, word: &str) -> bool {
        self.rules.abbreviations.contains(word)
    }

    /// Whether this word takes a capital wherever it stands.
    #[must_use]
    pub fn keeps_capital(&self, word: &str) -> bool {
        self.rules.capitals.contains(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = include_str!("../../../android/app/src/main/assets/grammar.tsv");

    fn parse(forms: &str) -> Grammar {
        Grammar::parse(&format!("{RULES}{forms}"))
    }

    fn expected_forms(history: &[String]) -> Option<(u8, u8)> {
        parse("").rules.expected_forms(history)
    }

    const FORMS: &str = "suis\t1\nsous\t0\nest\t4\nsommes\t8\nvoit\t4\nvois\t3\n";

    fn history(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn je_wants_the_first_person_even_across_clitics() {
        let grammar = parse(FORMS);
        for before in [&["je"][..], &["je", "ne"], &["je", "me"], &["je", "ne", "le"]] {
            assert!(grammar.score(&history(before), "suis") > grammar.score(&history(before), "sous"));
        }
    }

    #[test]
    fn each_person_agrees_with_its_own_form() {
        let grammar = parse(FORMS);
        assert!(grammar.score(&history(&["nous"]), "sommes") > 0.0);
        assert!(grammar.score(&history(&["il"]), "est") > 0.0);
        assert!(grammar.score(&history(&["il"]), "suis") < 0.0);
    }

    #[test]
    fn an_object_pronoun_does_not_replace_the_subject() {
        let grammar = parse(FORMS);
        assert!(grammar.score(&history(&["il", "nous"]), "voit") > 0.0);
    }

    const VERBS: &str = "être\t64\nété\t128\nbaser\t64\nbasse\t0\nbasé\t128\nest\t4\ntemps\t0\nmanger\t64\nmangé\t128\nlève\t5\nchat\t0\n";

    #[test]
    fn a_modal_wants_an_infinitive() {
        let grammar = parse(VERBS);
        for before in [&["il", "peut"][..], &["pourrait", "encore"], &["il", "ne", "peut", "pas"], &["on", "doit", "y"]] {
            assert!(grammar.score(&history(before), "être") > 0.0, "{before:?}");
            assert!(grammar.score(&history(before), "été") < 0.0, "{before:?}");
            assert!(grammar.score(&history(before), "est") < 0.0, "{before:?}");
        }
    }

    #[test]
    fn an_auxiliary_wants_a_participle() {
        let grammar = parse(VERBS);
        for before in [&["j'", "ai"][..], &["il", "a", "déjà"], &["a", "été"]] {
            assert!(grammar.score(&history(before), "mangé") > 0.0, "{before:?}");
            assert!(grammar.score(&history(before), "manger") < 0.0, "{before:?}");
        }
    }

    #[test]
    fn a_noun_after_an_auxiliary_stays_free() {
        let grammar = parse(VERBS);
        assert!(grammar.score(&history(&["j'", "ai"]), "temps") >= 0.0);
        assert!(grammar.score(&history(&["j'", "ai", "le"]), "temps").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["j'", "ai", "le"]), "manger").abs() < f32::EPSILON);
    }

    #[test]
    fn a_reflexive_wants_a_verb() {
        let grammar = parse(VERBS);
        let before = history(&["de", "se"]);
        assert!(grammar.score(&before, "baser") > 0.0);
        assert!(grammar.score(&before, "lève") > 0.0);
        assert!(grammar.score(&before, "basse") < 0.0);
        assert!(grammar.score(&before, "basé") < 0.0);
        assert!(grammar.score(&before, "merge").abs() < f32::EPSILON);
        assert!(grammar.score(&before, "pas").abs() < f32::EPSILON);
    }

    #[test]
    fn a_clitic_or_an_adverb_passes_after_a_modal() {
        let grammar = parse(VERBS);
        assert!(grammar.score(&history(&["peut"]), "le").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["peut"]), "bien").abs() < f32::EPSILON);
    }

    #[test]
    fn nothing_is_expected_after_an_ordinary_word() {
        assert_eq!(expected_forms(&history(&["le", "chat"])), None);
        assert_eq!(expected_forms(&history(&[])), None);
    }

    /// `porte`, `livre` and `entre` are nouns or a preposition too.
    const NOUNS: &str = "porte\t5\nporter\t64\nlivre\t5\nlivrer\t64\nentre\t5\nentrer\t64\nchat\t0\n\
                         donner\t64\ndonnez\t16\naider\t64\nattirer\t64\ndomain\t0\ndemain\t0\nvont\t32\n\
                         venez\t16\nplus\t3\nil\t0\nils\t0\nvenir\t64\nvenu\t128\n";

    #[test]
    fn a_pronoun_after_a_preposition_is_no_subject() {
        let grammar = parse(NOUNS);
        for before in [&["merci", "pour", "vous"][..], &["merci", "de", "vous"], &["avec", "elle"], &["chez", "nous"]] {
            assert_eq!(grammar.rules.subject(&history(before)), None, "{before:?}");
        }
        assert!(grammar.score(&history(&["merci", "pour", "vous"]), "donner").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["avec", "elle"]), "demain").abs() < f32::EPSILON);
    }

    #[test]
    fn a_subject_before_a_preposition_still_counts_after_it_only_if_it_stands_alone() {
        let grammar = parse(NOUNS);
        assert!(grammar.rules.subject(&history(&["il", "vous"])).is_some());
        assert!(grammar.rules.subject(&history(&["pour", "vous", "il"])).is_some());
        assert!(grammar.rules.subject(&history(&["de", "nous", "vous"])).is_some());
    }

    #[test]
    fn only_a_conjugated_form_can_disagree_with_a_subject() {
        let grammar = parse(NOUNS);
        assert!(grammar.score(&history(&["vous"]), "chat").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["vous"]), "vont") < 0.0);
        assert!(grammar.score(&history(&["vous"]), "venez") > 0.0);
    }

    #[test]
    fn an_adverb_is_free_after_a_subject() {
        let grammar = parse(NOUNS);
        assert!(grammar.score(&history(&["elle"]), "plus").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["elle"]), "aussi").abs() < f32::EPSILON);
    }

    #[test]
    fn an_article_after_a_modal_rewards_a_verb_and_refuses_nothing() {
        let grammar = parse(NOUNS);
        for (before, noun, verb) in [(&["je", "veux", "la"][..], "porte", "porter"), (&["il", "faut", "le"], "livre", "livrer")] {
            assert!(grammar.score(&history(before), noun).abs() < f32::EPSILON, "{before:?}");
            assert!(grammar.score(&history(before), verb) > 0.0, "{before:?}");
        }
    }

    #[test]
    fn aller_past_an_adverb_is_a_plain_verb() {
        let grammar = parse(NOUNS);
        let before = history(&["ça", "va", "bien"]);
        assert!(grammar.score(&before, "entre").abs() < f32::EPSILON);
        assert!(grammar.score(&before, "entrer").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["ça", "va"]), "entrer") > 0.0);
        assert!(grammar.score(&history(&["il", "peut", "bien"]), "entrer") > 0.0);
    }

    #[test]
    fn the_elided_s_admits_il() {
        let grammar = parse(NOUNS);
        assert!(grammar.score(&history(&["s'"]), "il") > 0.0);
        assert!(grammar.score(&history(&["s'"]), "ils") > 0.0);
        assert!(grammar.score(&history(&["s'"]), "chat") < 0.0);
        assert!(grammar.score(&history(&["se"]), "il") < 0.0);
    }

    #[test]
    fn without_rules_everything_is_neutral() {
        let grammar = Grammar::parse("suis\t1\nsous\t0\n");
        assert!(grammar.score(&history(&["je"]), "suis").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["je", "peut"]), "sous").abs() < f32::EPSILON);
        assert_eq!(grammar.mask("suis"), Some(1));
        assert!(!grammar.is_abbreviation("m."));
        assert!(!grammar.keeps_capital("i"));
    }

    #[test]
    fn bad_rules_are_skipped() {
        let grammar = Grammar::parse("@\n@bit\n@bit\tx\tnot\n@subject\tnobody\tje\n@unknown\ta\n@clitic\ta\tb\n");
        assert_eq!(grammar.rules.person_of("je"), None);
        assert!(!grammar.rules.clitics.contains("a"));
        assert_eq!(grammar.rules.conjugated, 0);
    }

    #[test]
    fn the_rules_name_the_engines_lists() {
        let grammar = parse("");
        assert!(grammar.is_abbreviation("m."));
        assert!(!grammar.is_abbreviation("fin."));
        assert!(grammar.keeps_capital("i'll"));
        assert!(!grammar.keeps_capital("il"));
        assert_eq!(grammar.rules.conjugated, 63);
    }

    #[test]
    fn every_form_is_found_in_the_sorted_arrays() {
        let grammar = Grammar::parse("vois\t3\nest\t4\nsuis\t1\nbad\nsous\t0\n");
        assert_eq!(grammar.mask("est"), Some(4));
        assert_eq!(grammar.mask("sous"), Some(0));
        assert_eq!(grammar.mask("suis"), Some(1));
        assert_eq!(grammar.mask("vois"), Some(3));
        assert_eq!(grammar.mask("aaa"), None);
        assert_eq!(grammar.mask("zzz"), None);
    }

    #[test]
    fn unknown_words_and_sentences_without_subject_stay_neutral() {
        let grammar = parse(FORMS);
        assert!(grammar.score(&history(&["je"]), "merge").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["le", "chat"]), "sous").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["je"]), "vous") > 0.0);
    }
}
