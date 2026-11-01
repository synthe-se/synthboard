//! Correction of tapped words. Each tap counts for every key near it, weighted
//! by its real distance, so `yhan` finds `than` on any layout where `y` and `t`
//! are neighbours, with no table of neighbouring letters.

use crate::geometry::{Point, distance};
use crate::layout::Layout;
use crate::lexicon::Lexicon;
use crate::shark2::better;

/// How far from a key a tap can still count for it, in key pitches.
const REACH_KEYS: f32 = 1.6;
/// Spread of a tap around its key, in key pitches.
const SIGMA_KEYS: f32 = 0.6;
/// The total spatial cost beyond which a word is not considered.
const MAX_COST: f32 = 7.0;
const COMPLETION_PENALTY: f32 = 0.35;
const NODE_BUDGET: usize = 30_000;

/// A word proposed for some taps.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction<'lexicon> {
    /// The word as written.
    pub surface: &'lexicon str,
    /// Higher is better. Only comparable within one call.
    pub score: f32,
    /// How far the taps were from this word's keys.
    pub cost: f32,
    /// False when the word continues past the taps.
    pub complete: bool,
}

/// Finds the words the taps may mean, best first. `weight` turns a word's
/// lexicon weight and `context` its sentence affinity into log scores.
#[must_use]
pub fn correct<'lexicon>(
    layout: &Layout,
    lexicon: &'lexicon Lexicon,
    taps: &[Point],
    weight: f32,
    top_k: usize,
    context: impl Fn(&str) -> f32,
) -> Vec<Correction<'lexicon>> {
    let pitch = layout.pitch();
    let reach = REACH_KEYS * pitch;
    let divisor = 2.0 * (SIGMA_KEYS * pitch).powi(2);
    let alphabet = lexicon.alphabet();
    let costs: Vec<f32> = taps
        .iter()
        .flat_map(|&tap| {
            alphabet.iter().map(move |&letter| {
                layout.center(letter).map_or(f32::INFINITY, |center| {
                    let gap = distance(tap, center);
                    if gap <= reach { gap * gap / divisor } else { f32::INFINITY }
                })
            })
        })
        .collect();
    let columns = alphabet.len().max(1);
    let cost = |letter: u16, tap: usize| {
        costs
            .get(tap * columns + usize::from(letter))
            .copied()
            .filter(|cost| cost.is_finite())
    };

    let mut found: Vec<Correction<'lexicon>> = Vec::new();
    lexicon.along_taps(cost, taps.len(), MAX_COST, NODE_BUDGET, |entry, spent, extra| {
        let penalty = COMPLETION_PENALTY * f32::from(extra);
        found.push(Correction {
            surface: entry.surface,
            score: weight * entry.weight + context(entry.surface) - spent - penalty,
            cost: spent,
            complete: extra == 0,
        });
    });
    found.sort_by(|a, b| better((a.score, a.surface), (b.score, b.surface)));
    found.truncate(top_k);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::grid;


    fn taps(layout: &Layout, word: &str) -> Vec<Point> {
        word.chars().filter_map(|letter| layout.center(letter)).collect()
    }

    fn best(found: &[Correction<'_>]) -> Option<String> {
        found.first().map(|c| c.surface.to_owned())
    }

    #[test]
    fn yhan_means_than_because_y_touches_t() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("than", 150.0), ("gran", 100.0), ("yeah", 100.0)]);
        let found = correct(&layout, &lexicon, &taps(&layout, "yhan"), 0.02, 3, |_| 0.0);
        assert_eq!(best(&found).as_deref(), Some("than"));
        assert!(found.first().is_some_and(|c| c.complete));
    }

    #[test]
    fn a_far_away_letter_is_not_a_typo() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("than", 150.0)]);
        assert_eq!(correct(&layout, &lexicon, &taps(&layout, "phan"), 0.02, 3, |_| 0.0), Vec::new());
    }

    #[test]
    fn doubled_letters_need_two_taps_on_one_key() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("aller", 150.0)]);
        let found = correct(&layout, &lexicon, &taps(&layout, "aller"), 0.02, 3, |_| 0.0);
        assert_eq!(best(&found).as_deref(), Some("aller"));
    }

    #[test]
    fn a_prefix_offers_completions_and_marks_them() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("bonjour", 150.0)]);
        let found = correct(&layout, &lexicon, &taps(&layout, "bon"), 0.02, 3, |_| 0.0);
        assert_eq!(best(&found).as_deref(), Some("bonjour"));
        assert!(found.first().is_some_and(|c| !c.complete));
    }

    #[test]
    fn context_ranks_equal_taps() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("ou", 100.0), ("où", 100.0)]);
        let prefer = |word: &str| if word == "où" { 1.0 } else { 0.0 };
        let found = correct(&layout, &lexicon, &taps(&layout, "ou"), 0.02, 2, prefer);
        assert_eq!(best(&found).as_deref(), Some("où"));
    }
}
