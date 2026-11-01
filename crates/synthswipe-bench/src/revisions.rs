//! What the next word revising a word would do to the owner's gestures, replayed
//! through the decoder that ships, for the owner to tune `NEXT_WORD_MARGIN`.
//!
//! Each pair of gestures is read again, the second after what the first reads now.
//! A word the owner corrected is what a revision could have saved, so a pair counts
//! whatever became of its first word, although the keyboard would leave a corrected
//! word alone. Counts only: this may be shown beyond the volume.

use std::sync::Arc;

use synthboard_core::{Decoding, Language, NEXT_WORD_MARGIN};
use synthswipe::{Params, tokens};

use crate::traces::{Traced, layout_of, next_word_pairs};

/// The margins tried, in nats, around the provisional one: from a table's whole
/// span of 1.2 to past the 1.5 of one agreement alone.
pub const MARGINS: [f32; 5] = [0.5, 1.0, 1.5, NEXT_WORD_MARGIN, 2.5];

/// What revisions would do at one margin.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// A wrong word becomes the word the owner kept.
    pub help: usize,
    /// The word the owner kept is replaced.
    pub hurt: usize,
    /// One wrong word becomes another.
    pub nothing: usize,
}

/// What the best alternative of a word would be against the truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    Help,
    Hurt,
    Nothing,
}

fn effect(written: &str, alternative: &str, truth: &str) -> Effect {
    if alternative == truth {
        Effect::Help
    } else if written == truth {
        Effect::Hurt
    } else {
        Effect::Nothing
    }
}

/// The gain of the revision this pair would offer, and its effect, if there is one.
fn proposal(word: &Traced, next: &Traced, language: &Language, params: &Params) -> Option<(f32, Effect)> {
    let truth = word.truth.as_deref()?.to_lowercase();
    let reading = Decoding::replaying(language, Arc::new(layout_of(&word.keys).ok()?), *params, &word.history);
    let mut ranked = reading.rank(&word.samples).into_iter();
    let written = ranked.next()?;
    let alternatives: Vec<String> = ranked.collect();
    let mut sentence = word.history.clone();
    sentence.extend(tokens(&written));
    let following = Decoding::replaying(language, Arc::new(layout_of(&next.keys).ok()?), *params, &sentence);
    let after = following.rank(&next.samples).into_iter().next()?;
    let (index, gain) = following.next_word_gain(&written, &alternatives, &after)?;
    let alternative = alternatives.get(index)?.to_lowercase();
    Some((gain, effect(&written.to_lowercase(), &alternative, &truth)))
}

fn tally(proposals: &[(f32, Effect)], margin: f32) -> Tally {
    let mut found = Tally::default();
    for (_, effect) in proposals.iter().filter(|(gain, _)| *gain > margin) {
        match effect {
            Effect::Help => found.help += 1,
            Effect::Hurt => found.hurt += 1,
            Effect::Nothing => found.nothing += 1,
        }
    }
    found
}

/// How many pairs of gestures were read, and what revisions do to them per margin.
#[must_use]
pub fn replay(gestures: &[Traced], language: &Language, params: &Params) -> (usize, Vec<(f32, Tally)>) {
    let pairs: Vec<(&Traced, &Traced)> = next_word_pairs(gestures).collect();
    let proposals: Vec<(f32, Effect)> = pairs.iter().filter_map(|(word, next)| proposal(word, next, language, params)).collect();
    (pairs.len(), MARGINS.into_iter().map(|margin| (margin, tally(&proposals, margin))).collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::cast_precision_loss)]
mod tests {
    use synthswipe::{Affinities, Bigrams, Grammar, Lexicon, fold};

    use super::*;
    use crate::traces::parse;

    const RULES: &str = include_str!("../../../android/app/src/main/assets/grammar.tsv");
    /// A letter row laid out wide enough for `peur` and `peut` to read alike.
    const ROWS: [&str; 3] = ["azertyuiop", "qsdfghjklm", "wxcvbn"];
    const PITCH: f32 = 100.0;
    const STEPS: u32 = 8;

    fn center(letter: char) -> (f32, f32) {
        ROWS.iter()
            .enumerate()
            .find_map(|(y, row)| row.find(letter).map(|x| (x as f32 * PITCH, y as f32 * PITCH)))
            .unwrap()
    }

    fn keys_line() -> String {
        let keys: Vec<String> = ROWS.concat().chars().map(|letter| {
            let (x, y) = center(letter);
            format!("{letter}:{x}:{y}:{PITCH}:{PITCH}")
        }).collect();
        format!("K\t{}\n", keys.join(" "))
    }

    fn path(word: &str) -> String {
        let centers: Vec<(f32, f32)> = fold(word).into_iter().map(center).collect();
        let mut points = vec![format!("{},{},0", centers[0].0, centers[0].1)];
        for pair in centers.windows(2) {
            for step in 1..=STEPS {
                let ratio = step as f32 / STEPS as f32;
                let (x, y) = (pair[0].0 + (pair[1].0 - pair[0].0) * ratio, pair[0].1 + (pair[1].1 - pair[0].1) * ratio);
                points.push(format!("{x},{y},{}", points.len() * 12));
            }
        }
        points.join(" ")
    }

    fn gesture(word: &str, history: &str) -> String {
        format!("G\t{word}\t{word}\t{}\t{history}\t\tg\n", path(word))
    }

    fn language() -> Language {
        let mut pairs = Vec::new();
        Bigrams::parse("peur 1000\npeut 1000\naller 1000\nrien 100000\npeut aller 900\n").compile_into(1, 0.0, &mut pairs);
        Language {
            lexicon: Arc::new(Lexicon::new([("peur", 170.0), ("peut", 165.0), ("aller", 150.0), ("merde", 150.0)])),
            affinities: Some(Arc::new(Affinities::from_bytes(&Affinities::to_bytes(pairs)))),
            grammar: Some(Arc::new(Grammar::parse(&format!("{RULES}aller\t64\n")))),
            neural: None,
        }
    }

    fn replayed(trace: &str) -> (usize, Vec<(f32, Tally)>) {
        let text = format!("{}{trace}", keys_line());
        replay(&parse(&text), &language(), &Params::default())
    }

    fn at_first_margin(trace: &str) -> Tally {
        replayed(trace).1[0].1
    }

    #[test]
    fn replacing_a_kept_word_hurts() {
        let trace = format!("{}{}", gesture("peur", ""), gesture("aller", "peur"));
        assert_eq!(at_first_margin(&trace), Tally { hurt: 1, ..Tally::default() });
    }

    #[test]
    fn replacing_a_word_with_the_one_the_owner_chose_helps() {
        let trace = format!("{}C\tpeut\n{}", gesture("peur", ""), gesture("aller", "peut"));
        assert_eq!(at_first_margin(&trace), Tally { help: 1, ..Tally::default() });
    }

    #[test]
    fn replacing_a_wrong_word_with_another_changes_nothing() {
        let trace = format!("{}C\tmerde\n{}", gesture("peur", ""), gesture("aller", "merde"));
        assert_eq!(at_first_margin(&trace), Tally { nothing: 1, ..Tally::default() });
    }

    #[test]
    fn a_gesture_of_unknown_truth_is_left_out() {
        let trace = format!("{}D\nM\n{}", gesture("peur", ""), gesture("aller", "peur"));
        assert_eq!(replayed(&trace).0, 0);
    }

    #[test]
    fn a_high_margin_revises_nothing() {
        let trace = format!("{}C\tpeut\n{}", gesture("peur", ""), gesture("aller", "peut"));
        let (pairs, tallies) = replayed(&trace);
        assert_eq!(pairs, 1);
        assert_eq!(tallies.iter().map(|(margin, _)| *margin).collect::<Vec<f32>>(), MARGINS);
        assert_eq!(tallies.last().unwrap().1, Tally::default());
    }

    #[test]
    fn the_shipped_margin_is_among_those_tried() {
        assert!(MARGINS.contains(&NEXT_WORD_MARGIN));
    }

    #[test]
    fn a_gain_at_the_margin_is_not_past_it() {
        assert_eq!(tally(&[(1.0, Effect::Help), (1.5, Effect::Hurt)], 1.0), Tally { hurt: 1, ..Tally::default() });
    }
}
