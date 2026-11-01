//! French subject and verb agreement. After `je`, a form of the first person
//! is likely and `sous` is not. This reads the sentence as a sentence, which
//! word pairs cannot do across `je ne suis` or `je me suis`.

// One bit per person, so that a form can agree with several.
const FIRST_SINGULAR: u8 = 1;
const SECOND_SINGULAR: u8 = 2;
const THIRD_SINGULAR: u8 = 4;
const FIRST_PLURAL: u8 = 8;
const SECOND_PLURAL: u8 = 16;
const THIRD_PLURAL: u8 = 32;
const CONJUGATED: u8 = 63;
const INFINITIVE: u8 = 64;
const PAST_PARTICIPLE: u8 = 128;

/// Words that may stand between a subject and its verb.
const CLITICS: [&str; 18] = [
    "ne", "n'", "me", "m'", "te", "t'", "se", "s'", "le", "la", "les", "l'", "lui", "leur", "y", "en", "nous", "vous",
];
/// Words that may stand between a verb and the verb form it calls for.
const ADVERBS: [&str; 14] = [
    "pas", "plus", "jamais", "encore", "bien", "mieux", "toujours", "déjà", "vraiment", "aussi", "même", "alors", "peut-être",
    "souvent",
];
/// Forms that call for an infinitive: `il peut être`, `elle pourrait venir`.
const MODALS: [&str; 44] = [
    "peux", "peut", "pouvons", "pouvez", "peuvent", "pourrais", "pourrait", "pourrions", "pourriez", "pourraient", "dois",
    "doit", "devons", "devez", "doivent", "devrais", "devrait", "devrions", "devriez", "devraient", "veux", "veut",
    "voulons", "voulez", "veulent", "voudrais", "voudrait", "voudrions", "voudriez", "voudraient", "vais", "vas", "va",
    "allons", "allez", "vont", "faut", "faudrait", "sais", "sait", "savons", "savez", "savent", "saurait",
];
/// Forms that call for a past participle: `j'ai mangé`, `a été optimisé`.
const AUXILIARIES: [&str; 13] = [
    "ai", "as", "a", "avons", "avez", "ont", "avais", "avait", "avions", "aviez", "avaient", "aurait", "été",
];
/// The clitics that make the next word a verb: `se baser`, `il se lève`.
const REFLEXIVE: [&str; 2] = ["se", "s'"];

/// Reward for a verb that agrees, and cost of a known word that cannot follow.
const AGREES: f32 = 1.0;
const DISAGREES: f32 = -1.0;

fn person_of(token: &str) -> Option<u8> {
    Some(match token {
        "je" | "j'" => FIRST_SINGULAR,
        "tu" => SECOND_SINGULAR,
        "il" | "elle" | "on" => THIRD_SINGULAR,
        "nous" => FIRST_PLURAL,
        "vous" => SECOND_PLURAL,
        "ils" | "elles" => THIRD_PLURAL,
        _ => return None,
    })
}

/// The subject of the verb about to be written, when `history` ends on one.
/// `nous` and `vous` can be objects, so the subject furthest back wins.
fn subject(history: &[String]) -> Option<u8> {
    let mut found = None;
    for token in history.iter().rev() {
        let token = token.as_str();
        match person_of(token) {
            Some(person) => {
                found = Some(person);
                if !CLITICS.contains(&token) {
                    return found;
                }
            }
            None if CLITICS.contains(&token) => {}
            None => return found,
        }
    }
    found
}

/// Which verb forms `history` calls for, and which it rules out.
fn expected_forms(history: &[String]) -> Option<(u8, u8)> {
    let last = history.last()?.as_str();
    if REFLEXIVE.contains(&last) {
        return Some((CONJUGATED | INFINITIVE, u8::MAX));
    }
    let mut before = history.iter().rev().map(String::as_str).skip_while(|token| ADVERBS.contains(token));
    let verb = before.next()?;
    if AUXILIARIES.contains(&verb) {
        return Some((PAST_PARTICIPLE, INFINITIVE));
    }
    let after_clitics = history
        .iter()
        .rev()
        .map(String::as_str)
        .find(|token| !ADVERBS.contains(token) && !CLITICS.contains(token))?;
    MODALS.contains(&after_clitics).then_some((INFINITIVE, CONJUGATED | PAST_PARTICIPLE))
}

/// French word forms and the persons they conjugate, sorted in flat arrays so
/// that a hundred thousand forms stay one allocation.
#[derive(Debug, Clone, Default)]
pub struct Grammar {
    text: String,
    ends: Vec<u32>,
    masks: Vec<u8>,
}

impl Grammar {
    /// Reads `form TAB mask` lines, skipping bad ones.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut forms: Vec<(&str, u8)> = text
            .lines()
            .filter_map(|line| {
                let (form, mask) = line.split_once('\t')?;
                Some((form, mask.parse().ok()?))
            })
            .collect();
        forms.sort_unstable();
        let mut grammar = Self::default();
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
        if let Some((wanted, refused)) = expected_forms(history)
            && !CLITICS.contains(&lower.as_str())
            && !ADVERBS.contains(&lower.as_str())
            && let Some(mask) = self.mask(&lower)
        {
            if mask & wanted != 0 {
                return AGREES;
            }
            if mask & refused != 0 || refused == u8::MAX {
                return DISAGREES;
            }
        }
        let Some(person) = subject(history) else {
            return 0.0;
        };
        // `je vous vois`: a clitic is as welcome after a subject as a verb.
        if CLITICS.contains(&lower.as_str()) {
            return AGREES;
        }
        match self.mask(&lower) {
            Some(mask) if mask & person != 0 => AGREES,
            Some(_) => DISAGREES,
            None => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMS: &str = "suis\t1\nsous\t0\nest\t4\nsommes\t8\nvoit\t4\nvois\t3\n";

    fn history(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn je_wants_the_first_person_even_across_clitics() {
        let grammar = Grammar::parse(FORMS);
        for before in [&["je"][..], &["je", "ne"], &["je", "me"], &["je", "ne", "le"]] {
            assert!(grammar.score(&history(before), "suis") > grammar.score(&history(before), "sous"));
        }
    }

    #[test]
    fn each_person_agrees_with_its_own_form() {
        let grammar = Grammar::parse(FORMS);
        assert!(grammar.score(&history(&["nous"]), "sommes") > 0.0);
        assert!(grammar.score(&history(&["il"]), "est") > 0.0);
        assert!(grammar.score(&history(&["il"]), "suis") < 0.0);
    }

    #[test]
    fn an_object_pronoun_does_not_replace_the_subject() {
        let grammar = Grammar::parse(FORMS);
        assert!(grammar.score(&history(&["il", "nous"]), "voit") > 0.0);
    }

    const VERBS: &str = "être\t64\nété\t128\nbaser\t64\nbasse\t0\nbasé\t128\nest\t4\ntemps\t0\nmanger\t64\nmangé\t128\nlève\t5\nchat\t0\n";

    #[test]
    fn a_modal_wants_an_infinitive() {
        let grammar = Grammar::parse(VERBS);
        for before in [&["il", "peut"][..], &["pourrait", "encore"], &["il", "ne", "peut", "pas"], &["on", "doit", "le"]] {
            assert!(grammar.score(&history(before), "être") > 0.0, "{before:?}");
            assert!(grammar.score(&history(before), "été") < 0.0, "{before:?}");
            assert!(grammar.score(&history(before), "est") < 0.0, "{before:?}");
        }
    }

    #[test]
    fn an_auxiliary_wants_a_participle() {
        let grammar = Grammar::parse(VERBS);
        for before in [&["j'", "ai"][..], &["il", "a", "déjà"], &["a", "été"]] {
            assert!(grammar.score(&history(before), "mangé") > 0.0, "{before:?}");
            assert!(grammar.score(&history(before), "manger") < 0.0, "{before:?}");
        }
    }

    #[test]
    fn a_noun_after_an_auxiliary_stays_free() {
        let grammar = Grammar::parse(VERBS);
        assert!(grammar.score(&history(&["j'", "ai"]), "temps") >= 0.0);
        assert!(grammar.score(&history(&["j'", "ai", "le"]), "temps").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["j'", "ai", "le"]), "manger").abs() < f32::EPSILON);
    }

    #[test]
    fn a_reflexive_wants_a_verb() {
        let grammar = Grammar::parse(VERBS);
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
        let grammar = Grammar::parse(VERBS);
        assert!(grammar.score(&history(&["peut"]), "le").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["peut"]), "bien").abs() < f32::EPSILON);
    }

    #[test]
    fn nothing_is_expected_after_an_ordinary_word() {
        assert_eq!(expected_forms(&history(&["le", "chat"])), None);
        assert_eq!(expected_forms(&history(&[])), None);
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
        let grammar = Grammar::parse(FORMS);
        assert!(grammar.score(&history(&["je"]), "merge").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["le", "chat"]), "sous").abs() < f32::EPSILON);
        assert!(grammar.score(&history(&["je"]), "vous") > 0.0);
    }
}
