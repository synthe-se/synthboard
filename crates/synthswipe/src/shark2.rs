//! Geometric decoder after SHARK2 (Kristensson and Zhai, UIST 2004).
//!
//! A trie walk keeps the words whose letters the gesture crosses in order.
//! Each survivor is then compared to the gesture through two channels: shape,
//! which ignores position and scale, and location, which does not. The
//! location channel is a weighted mean distance, without the original's
//! zero-cost tunnel, whose quadratic cost does not fit thousands of candidates.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::f32::consts::PI;
use std::fmt;

use crate::geometry::{Point, Sample, distance, is_usable, normalize_shape, resample};
use crate::layout::Layout;
use crate::lexicon::{Entry, Lexicon};
use crate::marks::{Mark, agreement, detect, intent_points, missing_turns, unexplained};

/// Decoder tuning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Points compared per gesture.
    pub points: u16,
    /// Points used by the first, cheap pass over every surviving word.
    pub coarse_points: u16,
    /// How many words survive the coarse pass.
    pub refine: u16,
    /// Spread of the shape channel, in normalized shape units.
    pub sigma_shape: f32,
    /// Spread of the location channel, in key pitches.
    pub sigma_location: f32,
    /// Weight of the word likelihood.
    pub lambda: f32,
    /// How far from the first and last sample a word may start or end, in key pitches.
    pub end_radius: f32,
    /// How close the gesture must pass to each letter, in key pitches.
    pub pass_radius: f32,
    /// How many of the best candidates get the elastic alignment pass.
    pub rerank: u16,
    /// Weight of the elastic alignment cost.
    pub alignment_weight: f32,
    /// Weight of the log probability given by the sentence so far.
    pub context_weight: f32,
    /// Weight of pauses and loops in the gesture.
    pub mark_weight: f32,
    /// Weight of slowdowns and turns that a word leaves unexplained.
    pub intent_weight: f32,
    /// Cost of each sharp turn in a word that the gesture never made.
    pub turn_weight: f32,
    /// Weight of the squared distance between the touch-down and the first key.
    pub start_weight: f32,
    /// Movements smaller than this, in key pitches, are treated as jitter.
    pub jitter_keys: f32,
    /// Cost of reading a gesture without its final hook.
    pub hook_penalty: f32,
}

/// Precision of a location weight, kept in integers so that it needs no cast from a count.
const RATIO_STEPS: usize = 1000;
/// A final turn closer to the end than this, in key pitches, is finger noise.
const MIN_HOOK_KEYS: f32 = 0.15;

/// Defaults tuned with `synthswipe-bench --tune`, then on the owner's traces.
impl Default for Params {
    fn default() -> Self {
        Self {
            points: 64,
            coarse_points: 16,
            refine: 256,
            sigma_shape: 0.15,
            sigma_location: 0.7,
            lambda: 0.02,
            end_radius: 1.5,
            pass_radius: 1.6,
            rerank: 128,
            alignment_weight: 2.0,
            context_weight: 0.15,
            mark_weight: 0.5,
            intent_weight: 0.5,
            turn_weight: 0.5,
            start_weight: 0.5,
            jitter_keys: 0.1,
            hook_penalty: 0.3,
        }
    }
}

/// A ranked word.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate<'lexicon> {
    /// The word as written.
    pub surface: &'lexicon str,
    /// Higher is better. Only comparable within one decode.
    pub score: f32,
}

/// Why a gesture was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The gesture has no samples.
    EmptyGesture,
    /// A sample is NaN, infinite or absurdly large.
    UnusableSample,
    /// `points` is below 2.
    TooFewPoints,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyGesture => "gesture has no samples",
            Self::UnusableSample => "gesture has an unusable sample",
            Self::TooFewPoints => "at least two comparison points are needed",
        })
    }
}

impl std::error::Error for DecodeError {}

fn location_weight(index: usize, count: usize) -> f32 {
    let center = count.saturating_sub(1) / 2;
    let offset = index.abs_diff(center);
    let span = center.max(1);
    let ratio = u16::try_from(offset * RATIO_STEPS / span).map_or(1.0, |value| f32::from(value) / f32::from(RATIO_STEPS as u16));
    0.5 + ratio
}

/// The mean squared distance, in squared key pitches, between each key and
/// the gesture point it best aligns with. Points are assigned in order, so a
/// finger may slow down or cut corners without a penalty. The first key is
/// tied to the first point and the last key to the last point.
fn alignment_cost(gesture: &[Point], keys: &[Point], pitch: f32, row: &mut Vec<f32>) -> f32 {
    let squared = |a, b| {
        let gap = distance(a, b) / pitch;
        gap * gap
    };
    let (Some(&first_key), Some(&last_point)) = (keys.first(), gesture.last()) else {
        return 0.0;
    };
    let Some(&first_point) = gesture.first() else {
        return 0.0;
    };
    row.clear();
    row.resize(gesture.len(), f32::INFINITY);
    if let Some(start) = row.first_mut() {
        *start = squared(first_point, first_key);
    }
    let mut key_count = 1.0_f32;
    for &key in keys.iter().skip(1) {
        let mut best_before = f32::INFINITY;
        for (cost, &point) in row.iter_mut().zip(gesture) {
            best_before = best_before.min(*cost);
            *cost = best_before + squared(point, key);
        }
        key_count += 1.0;
    }
    let last_cost = match keys {
        [only] => row.first().copied().unwrap_or(0.0) + squared(last_point, *only),
        _ => row.last().copied().unwrap_or(f32::INFINITY),
    };
    last_cost / key_count
}

const UNREACHED: u16 = u16::MAX;

/// For each letter and gesture position, the first later position within
/// `radius` of that letter's key. Letters absent from the layout are never reached.
struct Crossings {
    table: Vec<u16>,
    points: usize,
}

impl Crossings {
    fn new(gesture: &[Point], alphabet: &[char], layout: &Layout, radius: f32) -> Self {
        let points = gesture.len();
        let mut table = vec![UNREACHED; alphabet.len() * points];
        for (&letter, row) in alphabet.iter().zip(table.chunks_exact_mut(points)) {
            let Some(center) = layout.center(letter) else {
                continue;
            };
            let mut latest = UNREACHED;
            for (position, (slot, &point)) in row.iter_mut().zip(gesture).enumerate().rev() {
                if distance(point, center) <= radius {
                    latest = u16::try_from(position).unwrap_or(UNREACHED);
                }
                *slot = latest;
            }
        }
        Self { table, points }
    }

    fn next(&self, letter: u16, position: u16) -> Option<u16> {
        let index = usize::from(letter) * self.points + usize::from(position);
        self.table.get(index).copied().filter(|&at| at != UNREACHED)
    }
}

/// Decodes a gesture without any sentence context.
///
/// # Errors
/// See [`decode_in_context`].
pub fn decode<'lexicon>(
    layout: &Layout,
    lexicon: &'lexicon Lexicon,
    samples: &[Sample],
    params: &Params,
    top_k: usize,
) -> Result<Vec<Candidate<'lexicon>>, DecodeError> {
    decode_in_context(layout, lexicon, samples, params, top_k, |_| 0.0)
}

/// Decodes a gesture and returns at most `top_k` candidates, best first.
/// `context` gives each finalist a log probability from the sentence so far.
///
/// The work is bounded by the lexicon and `params.points`, whatever the gesture.
///
/// # Errors
/// Returns an error on an empty gesture, a non-finite sample or too few points.
pub fn decode_in_context<'lexicon>(
    layout: &Layout,
    lexicon: &'lexicon Lexicon,
    samples: &[Sample],
    params: &Params,
    top_k: usize,
    context: impl Fn(&str) -> f32,
) -> Result<Vec<Candidate<'lexicon>>, DecodeError> {
    if params.points < 2 {
        return Err(DecodeError::TooFewPoints);
    }
    if samples.iter().any(|s| !is_usable(s.x) || !is_usable(s.y)) {
        return Err(DecodeError::UnusableSample);
    }
    let raw: Vec<Point> = samples.iter().map(|s| [s.x, s.y]).collect();
    if raw.is_empty() {
        return Err(DecodeError::EmptyGesture);
    }
    let pitch = layout.pitch();
    let path = steady(&raw, params.jitter_keys * pitch);
    let marks = detect(samples, layout);
    let intent = intent_points(samples, pitch);

    // A hook may be a real last letter, so both readings compete.
    let mut ranked = rank_path(layout, lexicon, &path, &marks, &intent, params, &context);
    if let Some(trimmed) = without_end_hook(&path, pitch) {
        let others = rank_path(layout, lexicon, &trimmed, &marks, &intent, params, &context);
        ranked.extend(others.into_iter().map(|(score, entry)| (score - params.hook_penalty, entry)));
    }
    ranked.sort_by(best_first);
    // A word can be reached by two paths and must still appear only once.
    let mut seen = HashSet::new();
    ranked.retain(|item| seen.insert(item.1.surface));
    ranked.truncate(top_k);
    Ok(ranked
        .into_iter()
        .map(|(score, entry)| Candidate {
            surface: entry.surface,
            score,
        })
        .collect())
}

fn best_first(a: &(f32, Entry<'_>), b: &(f32, Entry<'_>)) -> Ordering {
    b.0.total_cmp(&a.0).then_with(|| a.1.surface.cmp(b.1.surface))
}

/// Ranks the lexicon against one reading of the gesture.
fn rank_path<'lexicon>(
    layout: &Layout,
    lexicon: &'lexicon Lexicon,
    path: &[Point],
    marks: &[Mark],
    intent: &[Point],
    params: &Params,
    context: &impl Fn(&str) -> f32,
) -> Vec<(f32, Entry<'lexicon>)> {
    let (Some(&start), Some(&end)) = (path.first(), path.last()) else {
        return Vec::new();
    };
    let pitch = layout.pitch();
    let alphabet = lexicon.alphabet();
    let centers: Vec<Option<Point>> = alphabet.iter().map(|&key| layout.center(key)).collect();
    let word_path = |entry: &Entry<'_>, out: &mut Vec<Point>| {
        out.clear();
        out.extend(
            entry
                .letters
                .iter()
                .filter_map(|&letter| centers.get(usize::from(letter)).copied().flatten()),
        );
    };

    let radius = params.end_radius * pitch;
    let starts = layout.keys_near(start, radius);
    let ends = layout.keys_near(end, radius);
    let flags = |keys: &[char]| -> Vec<bool> { alphabet.iter().map(|c| keys.contains(c)).collect() };
    let (is_start, is_end) = (flags(&starts), flags(&ends));
    let flagged = |flags: &[bool], letter: u16| flags.get(usize::from(letter)).copied().unwrap_or(false);

    let fine = Probe::new(path, params.points, params, pitch);
    let coarse = Probe::new(path, params.coarse_points.clamp(2, params.points), params, pitch);
    let crossings = Crossings::new(&fine.gesture, alphabet, layout, params.pass_radius * pitch);

    let mut scratch = Scratch::default();
    let mut keys: Vec<Point> = Vec::new();
    let mut ranked: Vec<(f32, Entry<'lexicon>)> = Vec::new();
    lexicon.reachable(
        |letter, position| crossings.next(letter, position),
        |letter| flagged(&is_start, letter),
        |letter| flagged(&is_end, letter),
        |entry| {
            word_path(&entry, &mut keys);
            let score = coarse.score(&keys, params.lambda * entry.weight, &mut scratch);
            ranked.push((score, entry));
        },
    );

    keep_best(&mut ranked, usize::from(params.refine), best_first);
    for (score, entry) in &mut ranked {
        word_path(entry, &mut keys);
        *score = fine.score(&keys, params.lambda * entry.weight, &mut scratch);
    }
    keep_best(&mut ranked, usize::from(params.rerank), best_first);

    let mut row = Vec::new();
    for (score, entry) in &mut ranked {
        word_path(entry, &mut keys);
        *score -= params.alignment_weight * alignment_cost(&fine.gesture, &keys, pitch, &mut row);
        *score -= params.intent_weight * unexplained(intent, &keys, pitch);
        *score -= params.turn_weight * missing_turns(&keys, intent, pitch);
        if let Some(&first) = keys.first() {
            let miss = distance(start, first) / pitch;
            *score -= params.start_weight * miss * miss;
        }
        *score += params.context_weight * context(entry.surface);
        *score += params.mark_weight * agreement(entry.surface, marks);
    }
    ranked
}

/// Drops points closer than `jitter` to the last one kept, so that the small
/// wander of a slow restart does not count as a shape.
fn steady(path: &[Point], jitter: f32) -> Vec<Point> {
    let mut kept: Vec<Point> = Vec::with_capacity(path.len());
    for &point in path {
        if kept.last().is_none_or(|&last| distance(last, point) >= jitter) {
            kept.push(point);
        }
    }
    kept
}

/// The path without a sharp turn in its last stretch, if it has one.
fn without_end_hook(path: &[Point], pitch: f32) -> Option<Vec<Point>> {
    const HOOK_KEYS: f32 = 1.0;
    const APPROACH_KEYS: f32 = 1.0;
    const MIN_TURN: f32 = 0.9;
    let end = *path.last()?;
    let reach = HOOK_KEYS * pitch;
    let mut best: Option<(usize, f32)> = None;
    for (index, &corner) in path.iter().enumerate().rev().skip(1) {
        let tail = distance(corner, end);
        if tail > reach {
            break;
        }
        if tail < MIN_HOOK_KEYS * pitch {
            continue;
        }
        let approach = path
            .get(..index)?
            .iter()
            .rev()
            .find(|&&point| distance(point, corner) >= APPROACH_KEYS * pitch)?;
        let before = (corner[1] - approach[1]).atan2(corner[0] - approach[0]);
        let after = (end[1] - corner[1]).atan2(end[0] - corner[0]);
        let turn = ((after - before + PI).rem_euclid(2.0 * PI) - PI).abs();
        if turn >= MIN_TURN && best.is_none_or(|(_, sharpest)| turn > sharpest) {
            best = Some((index, turn));
        }
    }
    let (index, _) = best?;
    path.get(..=index).map(<[Point]>::to_vec)
}

/// Keeps the `count` best items, without sorting the ones it drops.
fn keep_best<T>(items: &mut Vec<T>, count: usize, order: impl Fn(&T, &T) -> Ordering + Copy) {
    if items.len() > count && count > 0 {
        items.select_nth_unstable_by(count - 1, order);
        items.truncate(count);
    }
}

/// Reusable buffers, so that a decode allocates once and not per word.
#[derive(Default)]
struct Scratch {
    template: Vec<Point>,
    template_shape: Vec<Point>,
}

/// A gesture prepared at one resolution for comparison with word paths.
struct Probe {
    gesture: Vec<Point>,
    shape: Vec<Point>,
    weights: Vec<f32>,
    weight_sum: f32,
    points: u16,
    pitch: f32,
    shape_divisor: f32,
    location_divisor: f32,
}

impl Probe {
    fn new(path: &[Point], points: u16, params: &Params, pitch: f32) -> Self {
        let (mut gesture, mut shape) = (Vec::new(), Vec::new());
        resample(path, points, &mut gesture);
        normalize_shape(&gesture, &mut shape);
        let count = usize::from(points);
        let weights: Vec<f32> = (0..count).map(|i| location_weight(i, count)).collect();
        Self {
            gesture,
            shape,
            weight_sum: weights.iter().sum(),
            weights,
            points,
            pitch,
            shape_divisor: 2.0 * params.sigma_shape * params.sigma_shape,
            location_divisor: 2.0 * params.sigma_location * params.sigma_location,
        }
    }

    fn score(&self, keys: &[Point], prior: f32, scratch: &mut Scratch) -> f32 {
        resample(keys, self.points, &mut scratch.template);
        normalize_shape(&scratch.template, &mut scratch.template_shape);
        let shape = self
            .shape
            .iter()
            .zip(&scratch.template_shape)
            .map(|(&a, &b)| distance(a, b))
            .sum::<f32>()
            / f32::from(self.points);
        let location = self
            .gesture
            .iter()
            .zip(&scratch.template)
            .zip(&self.weights)
            .map(|((&a, &b), &weight)| weight * distance(a, b))
            .sum::<f32>()
            / (self.weight_sum * self.pitch);
        prior - shape * shape / self.shape_divisor - location * location / self.location_divisor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::grid;
    use crate::layout::Key;


    fn trace(layout: &Layout, word: &str) -> Vec<Sample> {
        crate::fold(word)
            .iter()
            .filter_map(|&key| layout.center(key))
            .map(|[x, y]| Sample { x, y, t_ms: 0 })
            .collect()
    }

    fn best(layout: &Layout, lexicon: &Lexicon, word: &str) -> Option<String> {
        decode(layout, lexicon, &trace(layout, word), &Params::default(), 3)
            .ok()?
            .first()
            .map(|candidate| candidate.surface.to_owned())
    }

    #[test]
    fn alignment_is_free_on_a_perfect_path_and_ignores_extra_samples() {
        let keys = [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]];
        let dense = [[0.0, 0.0], [2.0, 0.0], [4.0, 0.0], [4.0, 1.0], [4.0, 3.0]];
        let mut row = Vec::new();
        assert!(alignment_cost(&dense, &keys, 1.0, &mut row).abs() < 1e-6);
    }

    #[test]
    fn alignment_respects_order() {
        let keys = [[0.0, 0.0], [4.0, 0.0], [8.0, 0.0]];
        let forward = [[0.0, 0.0], [4.0, 0.0], [8.0, 0.0]];
        let backward = [[0.0, 0.0], [8.0, 0.0], [4.0, 0.0], [8.0, 0.0]];
        let mut row = Vec::new();
        let straight = alignment_cost(&forward, &keys, 1.0, &mut row);
        let looped = alignment_cost(&backward, &keys, 1.0, &mut row);
        assert!(straight <= looped);
        let missing_middle = [[0.0, 0.0], [8.0, 0.0]];
        assert!(alignment_cost(&missing_middle, &keys, 1.0, &mut row) > straight);
    }

    #[test]
    fn alignment_handles_a_single_key() {
        let mut row = Vec::new();
        let cost = alignment_cost(&[[0.0, 0.0], [2.0, 0.0]], &[[0.0, 0.0]], 1.0, &mut row);
        assert!((cost - 4.0).abs() < 1e-6);
    }

    /// Pins the scoring arithmetic. Any change to these values must be a
    /// deliberate one, justified by the benchmark.
    #[test]
    fn scores_match_the_reference_values() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("bon", 100.0), ("bien", 50.0), ("bo", 10.0), ("bn", 200.0)]);
        let samples: Vec<Sample> = [[5.2, 2.1], [6.0, 1.0], [7.5, 0.3], [6.5, 1.1], [5.3, 1.9]]
            .into_iter()
            .map(|[x, y]| Sample { x, y, t_ms: 0 })
            .collect();
        let found = decode(&layout, &lexicon, &samples, &Params::default(), 4).unwrap_or_default();
        let surfaces: Vec<&str> = found.iter().map(|candidate| candidate.surface).collect();
        assert_eq!(surfaces, ["bon", "bn"]);
        assert!(found.first().is_some_and(|c| c.score > -3.0), "{found:?}");
        assert!(found.get(1).is_some_and(|c| c.score < -5.0), "{found:?}");
    }

    #[test]
    fn scores_do_not_depend_on_the_size_of_the_keyboard() {
        let scaled = |factor: f32| {
            let rows = ["azertyuiop", "qsdfghjklm", "wxcvbn"];
            let keys = rows.iter().zip([0.0_f32, 1.0, 2.0]).flat_map(move |(row, y)| {
                row.chars().zip((0_u8..).map(f32::from)).map(move |(label, x)| Key {
                    label,
                    x: x * factor,
                    y: y * factor,
                })
            });
            let layout = Layout::new(keys).ok()?;
            let samples: Vec<Sample> = [[5.2, 2.1], [6.0, 1.0], [7.5, 0.3], [6.5, 1.1], [5.3, 1.9]]
                .into_iter()
                .map(|[x, y]| Sample { x: x * factor, y: y * factor, t_ms: 0 })
                .collect();
            let lexicon = Lexicon::new([("bon", 100.0), ("bn", 200.0)]);
            let found = decode(&layout, &lexicon, &samples, &Params::default(), 2).ok()?;
            Some(found.iter().map(|c| c.score).collect::<Vec<f32>>())
        };
        let (small, large) = (scaled(1.0).unwrap_or_default(), scaled(3.0).unwrap_or_default());
        assert_eq!(small.len(), 2);
        for (a, b) in small.iter().zip(&large) {
            assert!((a - b).abs() < 0.001, "{small:?} {large:?}");
        }
    }

    #[test]
    fn crossings_find_the_first_pass_at_or_after_a_position() {
        let Some(layout) = grid() else {
            return;
        };
        let gesture = [[9.0, 2.0], [0.0, 0.0], [9.0, 2.0], [0.0, 0.0]];
        let alphabet = ['a', 'p'];
        let crossings = Crossings::new(&gesture, &alphabet, &layout, 0.5);
        assert_eq!(crossings.next(0, 0), Some(1));
        assert_eq!(crossings.next(0, 1), Some(1));
        assert_eq!(crossings.next(0, 2), Some(3));
        assert_eq!(crossings.next(1, 0), None);
        assert_eq!(crossings.next(2, 0), None);
    }

    #[test]
    fn location_weight_favours_the_ends_of_the_gesture() {
        assert!((location_weight(0, 65) - 1.5).abs() < 1e-3);
        assert!((location_weight(32, 65) - 0.5).abs() < 1e-3);
        assert!((location_weight(64, 65) - 1.5).abs() < 1e-3);
    }

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(DecodeError::EmptyGesture.to_string(), "gesture has no samples");
    }

    #[test]
    fn a_final_hook_does_not_change_the_word() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("bon", 100.0), ("bonh", 100.0), ("bong", 100.0)]);
        let mut samples = trace(&layout, "bon");
        let &Sample { x, y, .. } = samples.last().unwrap_or(&Sample { x: 0.0, y: 0.0, t_ms: 0 });
        samples.push(Sample { x: x - 0.4, y: y + 0.6, t_ms: 0 });
        let found = decode(&layout, &lexicon, &samples, &Params::default(), 3).unwrap_or_default();
        assert_eq!(found.first().map(|c| c.surface), Some("bon"));
    }

    #[test]
    fn jitter_is_dropped_and_real_moves_are_kept() {
        let path = [[0.0, 0.0], [0.01, 0.0], [0.02, 0.01], [1.0, 0.0]];
        assert_eq!(steady(&path, 0.1), [[0.0, 0.0], [1.0, 0.0]]);
    }

    #[test]
    fn a_straight_path_has_no_hook() {
        let path: Vec<Point> = (0_u8..20).map(|x| [f32::from(x) * 0.25, 0.0]).collect();
        assert!(without_end_hook(&path, 1.0).is_none());
    }

    #[test]
    fn an_elided_word_is_found_with_or_without_the_apostrophe_key() {
        let keys = ["azertyuiop", "qsdfghjklm", "wxcvbn'"].iter().zip([0.0_f32, 1.0, 2.0]).flat_map(|(row, y)| {
            row.chars().zip((0_u8..).map(f32::from)).map(move |(label, x)| Key { label, x, y })
        });
        let Ok(layout) = Layout::new(keys) else {
            return;
        };
        let lexicon = Lexicon::new([("c'est", 150.0), ("cnet", 60.0), ("chest", 60.0)]);
        let spelled = |letters: &str| -> Vec<Sample> {
            letters
                .chars()
                .filter_map(|letter| layout.center(letter))
                .map(|[x, y]| Sample { x, y, t_ms: 0 })
                .collect()
        };
        for gesture in ["c'est", "cest"] {
            let found = decode(&layout, &lexicon, &spelled(gesture), &Params::default(), 3).unwrap_or_default();
            assert_eq!(found.first().map(|c| c.surface), Some("c'est"), "{gesture}");
            assert_eq!(found.iter().filter(|c| c.surface == "c'est").count(), 1);
        }
    }

    #[test]
    fn context_can_overturn_the_geometry() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("ou", 200.0), ("où", 10.0)]);
        let samples = trace(&layout, "ou");
        let params = Params { context_weight: 1.0, ..Params::default() };
        let context = |word: &str| if word == "où" { 0.0 } else { -20.0 };
        let found = decode_in_context(&layout, &lexicon, &samples, &params, 2, context);
        let top = found.ok().and_then(|list| list.first().map(|c| c.surface.to_owned()));
        assert_eq!(top.as_deref(), Some("où"));
    }

    #[test]
    fn an_ideal_trace_finds_its_word() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("bonjour", 1.0), ("bonheur", 1.0), ("bon", 1.0)]);
        assert_eq!(best(&layout, &lexicon, "bonjour").as_deref(), Some("bonjour"));
        assert_eq!(best(&layout, &lexicon, "bon").as_deref(), Some("bon"));
    }

    #[test]
    fn a_word_with_a_letter_missing_from_the_layout_is_not_offered() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("été", 1.0), ("eπe", 9.0)]);
        assert_eq!(best(&layout, &lexicon, "été").as_deref(), Some("été"));
    }

    #[test]
    fn the_same_gesture_always_gives_the_same_ranking() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("ou", 2.0), ("où", 2.0), ("oui", 1.0)]);
        let samples = trace(&layout, "ou");
        let first = decode(&layout, &lexicon, &samples, &Params::default(), 3);
        let second = decode(&layout, &lexicon, &samples, &Params::default(), 3);
        assert_eq!(first, second);
    }

    #[test]
    fn bad_input_is_rejected_without_panicking() {
        let Some(layout) = grid() else {
            return;
        };
        let lexicon = Lexicon::new([("bon", 1.0)]);
        let params = Params::default();
        assert_eq!(decode(&layout, &lexicon, &[], &params, 3), Err(DecodeError::EmptyGesture));
        let nan = [Sample { x: f32::NAN, y: 0.0, t_ms: 0 }];
        assert_eq!(decode(&layout, &lexicon, &nan, &params, 3), Err(DecodeError::UnusableSample));
        let huge = [Sample { x: 1e30, y: 0.0, t_ms: 0 }];
        assert_eq!(decode(&layout, &lexicon, &huge, &params, 3), Err(DecodeError::UnusableSample));
        let thin = Params { points: 1, ..params };
        let tap = [Sample { x: 0.0, y: 0.0, t_ms: 0 }];
        assert_eq!(decode(&layout, &lexicon, &tap, &thin, 3), Err(DecodeError::TooFewPoints));
    }
}
