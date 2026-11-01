//! Reads a gesture with a small network, and blends that reading into the
//! geometric ranking. The network only says, frame by frame, which key the
//! finger is after; the lexicon, the context and the memory stay outside it.
//!
//! Each key is described to the network by its position, so that a layout stays
//! data. The running of the model is behind [`Encoder`], which keeps this crate
//! free of any runtime. The model comes from `tools/pack-neural.sh`.

use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;

use crate::geometry::Sample;
use crate::layout::Layout;
use crate::lexicon::{Lexicon, apostrophe_path, fold};
use crate::shark2::{Candidate, Params, best_first};

/// Frames a gesture is resampled to.
pub const FRAMES: usize = 64;
/// Values per frame: position, step, turn and time.
pub const FEATURES: usize = 7;
/// Size of a key's description, and of what the network asks of it.
pub const KEY_DIMENSION: usize = 64;
/// Values the model reads, feature by feature.
pub const INPUT_VALUES: usize = FEATURES * FRAMES;
/// Values the model writes: what it asks of a key, then the blank, each over all frames.
pub const OUTPUT_VALUES: usize = (KEY_DIMENSION + 1) * FRAMES;

/// The keys the model knows, in the order of its outputs.
pub const MODEL_KEYS: &str = "abcdefghijklmnopqrstuvwxyz'";
const KEYS: usize = MODEL_KEYS.len();
/// Keys and the CTC blank, which comes first.
const CLASSES: usize = KEYS + 1;
/// A key absent from the layout gets this logit, which no alignment will pick.
pub const ABSENT: f32 = -1e4;

/// Share of a frame's place along the gesture given by path length. Time gives
/// the rest, so that a pause on a key keeps some frames. From the training spike.
const ARC_SHARE: f32 = 0.7;
/// Maps key pitches to about [-1, 1]: a layout is about ten keys wide.
pub const POSITION_SCALE: f32 = 0.2;
/// Scales of the first and second differences, which make them about unit size.
const STEP_SCALE: f32 = 4.0;
const TURN_SCALE: f32 = 8.0;
const TIME_SCALE: f32 = 0.33;

/// Hypotheses kept per frame. Top-1 is unchanged from 256 to within 0.2 point, at
/// a third of the time, on the held-out sets of the 2026-11-01 spike.
const BEAM: usize = 128;
/// A hypothesis this far below the best one, in nats, is dropped. Same measurement.
const MARGIN: f32 = 12.0;
/// One beam hypothesis meets at most one self-loop and all keys as children.
const TABLE_BITS: u32 = (2 * BEAM * CLASSES).next_power_of_two().trailing_zeros();
/// Words of the search and of the geometry that compete in the blend.
pub const POOL: usize = 32;
/// Weight of the network against the geometric score. Tuned on the odd rows of the
/// held-out sets of the 2026-11-01 spike, one value for every set.
pub const NETWORK_WEIGHT: f32 = 0.5;
/// Weight of a word's frequency in what the network says of it. Same tuning.
pub const FREQUENCY_WEIGHT: f32 = 0.02;

/// Geometric effort when the network takes part. On the A520 this takes a full decode
/// from 21.3 to 16 ms at p95, for 0.2 to 0.65 point on the held-out sets (2026-11-01).
const LEAN_REFINE: u16 = 64;
const LEAN_RERANK: u16 = 32;
const LEAN_COARSE_POINTS: u16 = 8;
/// The network's scale drowns the sentence, so context counts this much more beside
/// it. Best of 0.5 to 16 on the owner's 195 replayed gestures: 142 right, against 139
/// at 1 and 139 at 4 (2026-11-01).
const LEAN_CONTEXT_BOOST: f32 = 2.0;

/// What a model file starts with.
pub const MODEL_MAGIC: &[u8; 8] = b"SYNEURO1";
/// Key head: a linear map from a position to `KEY_DIMENSION`, then a square one.
const FIRST_WEIGHTS: usize = 2 * KEY_DIMENSION;
const SECOND_WEIGHTS: usize = KEY_DIMENSION * KEY_DIMENSION;
const HEAD_VALUES: usize = FIRST_WEIGHTS + KEY_DIMENSION + SECOND_WEIGHTS + KEY_DIMENSION;
const F32_BYTES: usize = size_of::<f32>();
/// Bytes of the key head in a model file.
pub const MODEL_HEAD_BYTES: usize = HEAD_VALUES * F32_BYTES;

/// How the network's reading counts in a blend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mix {
    /// Weight of the network against the geometric score.
    pub weight: f32,
    /// Weight of a word's frequency in what the network says of it.
    pub frequency: f32,
    /// Where a word only the network found starts. Across lexicons blended
    /// together it must be one number, or their scores cannot be compared.
    pub floor: f32,
}

impl Mix {
    /// The shipped weights, starting from the worst word the geometry kept.
    #[must_use]
    pub fn shipped(geometry: &[Candidate<'_>]) -> Self {
        Self { weight: NETWORK_WEIGHT, frequency: FREQUENCY_WEIGHT, floor: floor_of(geometry) }
    }
}

/// The worst word the geometry kept, or 0 if it kept none.
#[must_use]
pub fn floor_of(geometry: &[Candidate<'_>]) -> f32 {
    let floor = geometry.iter().map(|candidate| candidate.score).fold(f32::INFINITY, f32::min);
    if floor.is_finite() { floor } else { 0.0 }
}

/// What the geometry may skip when the network reads the gesture too.
#[must_use]
pub fn lean(params: &Params) -> Params {
    Params {
        refine: LEAN_REFINE,
        rerank: LEAN_RERANK,
        coarse_points: LEAN_COARSE_POINTS,
        context_weight: params.context_weight * LEAN_CONTEXT_BOOST,
        ..*params
    }
}

/// Runs the model on one gesture.
pub trait Encoder: Send + Sync + fmt::Debug {
    /// Fills `output` from `input`, of [`OUTPUT_VALUES`] and [`INPUT_VALUES`] values.
    /// Returns false on any failure, and the geometry then answers alone.
    fn encode(&self, input: &[f32], output: &mut [f32]) -> bool;
}

/// Splits a model file into the key head and what the runtime loads. Any bytes
/// are accepted: `None` means this is not a model file.
#[must_use]
pub fn split_model(bytes: &[u8]) -> Option<(KeyHead, &[u8])> {
    let body = bytes.strip_prefix(MODEL_MAGIC)?;
    let (head, model) = body.split_at_checked(MODEL_HEAD_BYTES)?;
    let values: Vec<f32> = head.as_chunks::<F32_BYTES>().0.iter().map(|value| f32::from_le_bytes(*value)).collect();
    (values.iter().all(|value| value.is_finite()) && !model.is_empty()).then_some((KeyHead { values }, model))
}

/// What turns a key's position into its description.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyHead {
    values: Vec<f32>,
}

impl KeyHead {
    fn describe(&self, position: [f32; 2]) -> Option<[f32; KEY_DIMENSION]> {
        let (first, rest) = self.values.split_at_checked(FIRST_WEIGHTS)?;
        let (first_bias, rest) = rest.split_at_checked(KEY_DIMENSION)?;
        let (second, second_bias) = rest.split_at_checked(SECOND_WEIGHTS)?;
        let mut hidden = [0.0; KEY_DIMENSION];
        for (index, slot) in hidden.iter_mut().enumerate() {
            let weights = first.iter().skip(index).step_by(KEY_DIMENSION);
            let mapped: f32 = position.iter().zip(weights).map(|(value, weight)| value * weight).sum();
            *slot = (mapped + first_bias.get(index)?).max(0.0);
        }
        let mut out = [0.0; KEY_DIMENSION];
        for (index, slot) in out.iter_mut().enumerate() {
            let weights = second.iter().skip(index).step_by(KEY_DIMENSION);
            let mapped: f32 = hidden.iter().zip(weights).map(|(value, weight)| value * weight).sum();
            *slot = mapped + second_bias.get(index)?;
        }
        Some(out)
    }
}

/// The model and its key head.
#[derive(Debug)]
pub struct Neural {
    head: KeyHead,
    encoder: Box<dyn Encoder>,
}

impl Neural {
    #[must_use]
    pub fn new(head: KeyHead, encoder: Box<dyn Encoder>) -> Self {
        Self { head, encoder }
    }

    /// What the network sees in this gesture, or `None` if it cannot say:
    /// the geometry then answers alone.
    #[must_use]
    pub fn read(&self, layout: &Layout, samples: &[Sample]) -> Option<Reading> {
        let keys = KeyPlaces::of(layout)?;
        let input = features(samples, layout.pitch(), keys.center)?;
        let mut output = vec![0.0; OUTPUT_VALUES];
        if !self.encoder.encode(&input, &mut output) || output.iter().any(|value| !value.is_finite()) {
            return None;
        }
        let mut logits = vec![0.0; CLASSES * FRAMES];
        let (queries, blanks) = output.split_at_checked(KEY_DIMENSION * FRAMES)?;
        for (row, blank) in logits.as_chunks_mut::<CLASSES>().0.iter_mut().zip(blanks) {
            *row.first_mut()? = *blank;
        }
        for (key, place) in keys.places.iter().enumerate() {
            let description = place.and_then(|position| self.head.describe(position));
            for (frame, row) in logits.as_chunks_mut::<CLASSES>().0.iter_mut().enumerate() {
                *row.get_mut(key + 1)? = description.as_ref().map_or(ABSENT, |description| {
                    let asked = queries.iter().skip(frame).step_by(FRAMES);
                    description.iter().zip(asked).map(|(value, query)| value * query).sum()
                });
            }
        }
        logits.as_chunks_mut::<CLASSES>().0.iter_mut().for_each(|row| log_softmax(row));
        Some(Reading { rows: logits })
    }
}

/// The 27 keys relative to the middle of the layout, in model units.
struct KeyPlaces {
    /// The middle of the keys, in key pitches.
    center: [f32; 2],
    places: [Option<[f32; 2]>; KEYS],
}

impl KeyPlaces {
    fn of(layout: &Layout) -> Option<Self> {
        let pitch = layout.pitch();
        let mut center = [0.0; 2];
        let mut present = 0.0;
        let mut raw = [None; KEYS];
        for (slot, label) in raw.iter_mut().zip(MODEL_KEYS.chars()) {
            if let Some([x, y]) = layout.center(label) {
                let place = [x / pitch, y / pitch];
                center = [center[0] + place[0], center[1] + place[1]];
                present += 1.0;
                *slot = Some(place);
            }
        }
        if present == 0.0 {
            return None;
        }
        center = [center[0] / present, center[1] / present];
        let places = raw.map(|place: Option<[f32; 2]>| {
            place.map(|[x, y]| [(x - center[0]) * POSITION_SCALE, (y - center[1]) * POSITION_SCALE])
        });
        Some(Self { center, places })
    }
}

fn features(samples: &[Sample], pitch: f32, center: [f32; 2]) -> Option<Vec<f32>> {
    let points: Vec<[f32; 3]> =
        samples.iter().map(|s| [s.x / pitch - center[0], s.y / pitch - center[1], s.t_ms as f32]).collect();
    frame_features(&points)
}

/// Resamples x, y, milliseconds points, in pitches from the layout's centre, to [`FRAMES`] frames
/// and describes each one. Public so that training reads gestures exactly as the phone does.
#[must_use]
pub fn frame_features(points: &[[f32; 3]]) -> Option<Vec<f32>> {
    let [first, .., final_point] = points else {
        return None;
    };
    let span = points.len() as f32 - 1.0;
    let duration = (final_point[2] - first[2]).max(0.0);
    let mut lengths = Vec::with_capacity(points.len());
    let mut travelled = 0.0;
    lengths.push(0.0);
    for pair in points.windows(2) {
        if let [a, b] = pair {
            travelled += (b[0] - a[0]).hypot(b[1] - a[1]);
            lengths.push(travelled);
        }
    }
    let along: Vec<f32> = points
        .iter()
        .zip(&lengths)
        .enumerate()
        .map(|(index, (point, length))| {
            let by_index = index as f32 / span;
            let by_length = if travelled > 0.0 { length / travelled } else { by_index };
            let by_time = if duration > 0.0 { ((point[2] - first[2]) / duration).clamp(0.0, 1.0) } else { by_index };
            ARC_SHARE * by_length + (1.0 - ARC_SHARE) * by_time
        })
        .collect();

    let mut frames = [[0.0; 3]; FRAMES];
    let mut segment = 0;
    for (index, frame) in frames.iter_mut().enumerate() {
        let wanted = index as f32 / (FRAMES - 1) as f32;
        while segment + 2 < points.len() && *along.get(segment + 1)? < wanted {
            segment += 1;
        }
        let (start, stop) = (*along.get(segment)?, *along.get(segment + 1)?);
        let gap = stop - start;
        let share = if gap > 0.0 { ((wanted - start) / gap).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b) = (points.get(segment)?, points.get(segment + 1)?);
        *frame = [a[0] + share * (b[0] - a[0]), a[1] + share * (b[1] - a[1]), a[2] + share * (b[2] - a[2])];
    }

    let mut input = vec![0.0; INPUT_VALUES];
    for (index, frame) in frames.iter().enumerate() {
        let before = *frames.get(index.saturating_sub(1))?;
        let after = *frames.get(index + 1).unwrap_or(frame);
        let values = [
            frame[0] * POSITION_SCALE,
            frame[1] * POSITION_SCALE,
            (after[0] - before[0]) / 2.0 * STEP_SCALE,
            (after[1] - before[1]) / 2.0 * STEP_SCALE,
            (after[0] - 2.0 * frame[0] + before[0]) * TURN_SCALE,
            (after[1] - 2.0 * frame[1] + before[1]) * TURN_SCALE,
            (after[2] - before[2]).max(0.0).ln_1p() * TIME_SCALE,
        ];
        for (feature, value) in values.into_iter().enumerate() {
            *input.get_mut(feature * FRAMES + index)? = value;
        }
    }
    input.iter().all(|value| value.is_finite()).then_some(input)
}

fn log_softmax(row: &mut [f32]) {
    let top = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let total: f32 = row.iter().map(|value| (value - top).exp()).sum();
    let shift = top + total.ln();
    for value in row {
        *value -= shift;
    }
}

/// Beyond this gap in nats the smaller term no longer shows in a sum.
const NEGLIGIBLE: f32 = 12.0;
/// Steps of the table per nat. The worst error is half a step at the steepest slope,
/// 0.0039 nat at a gap of 0, which is far under what ranks words.
const STEPS_PER_NAT: f32 = 64.0;

/// `ln(1 + e^-gap)`, sampled: the search adds logs for every hypothesis of every
/// frame, and on a small core `exp` and `ln_1p` were a large part of its cost.
fn gap_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=(NEGLIGIBLE * STEPS_PER_NAT) as u32 + 1)
            .map(|step| (-(step as f32 + 0.5) / STEPS_PER_NAT).exp().ln_1p())
            .collect()
    })
}

/// The log of the sum of two probabilities given as logs.
fn log_add(a: f32, b: f32) -> f32 {
    let (high, low) = if a > b { (a, b) } else { (b, a) };
    let gap = high - low;
    // Written so that the gap between two infinities, a NaN, stays out.
    if gap <= NEGLIGIBLE {
        high + gap_table().get((gap * STEPS_PER_NAT) as usize).copied().unwrap_or(0.0)
    } else {
        high
    }
}

fn class_of(key: char) -> Option<usize> {
    MODEL_KEYS.find(key).map(|index| index + 1)
}

/// A hypothesis: a trie node, with the log-probability of reaching it ending on
/// a blank and on its own letter.
#[derive(Clone, Copy)]
struct Hypothesis {
    node: u32,
    class: usize,
    blank: f32,
    solid: f32,
    total: f32,
}

/// Finds the hypothesis of a node in one frame. A map from nodes, sized by the
/// beam, which unlike an array over all nodes costs nothing with the lexicon's size.
struct Frame {
    slots: Vec<u32>,
    used: Vec<usize>,
    hypotheses: Vec<Hypothesis>,
}

const EMPTY: u32 = u32::MAX;
/// Fibonacci hashing: spreads node numbers, which are close together.
const SPREAD: u32 = 0x9E37_79B1;

impl Frame {
    fn new() -> Self {
        Self { slots: vec![EMPTY; 1 << TABLE_BITS], used: Vec::new(), hypotheses: Vec::new() }
    }

    fn clear(&mut self) {
        for &slot in &self.used {
            if let Some(entry) = self.slots.get_mut(slot) {
                *entry = EMPTY;
            }
        }
        self.used.clear();
        self.hypotheses.clear();
    }

    fn at(&mut self, node: u32, class: usize) -> Option<&mut Hypothesis> {
        let mask = self.slots.len() - 1;
        let mut slot = (node.wrapping_mul(SPREAD) >> (u32::BITS - TABLE_BITS)) as usize;
        loop {
            let entry = *self.slots.get(slot)?;
            if entry == EMPTY {
                *self.slots.get_mut(slot)? = u32::try_from(self.hypotheses.len()).ok()?;
                self.used.push(slot);
                let blank = f32::NEG_INFINITY;
                self.hypotheses.push(Hypothesis { node, class, blank, solid: blank, total: blank });
                return self.hypotheses.last_mut();
            }
            if self.hypotheses.get(entry as usize)?.node == node {
                return self.hypotheses.get_mut(entry as usize);
            }
            slot = (slot + 1) & mask;
        }
    }
}

/// What the network saw in one gesture, as log-probabilities per frame and class.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    rows: Vec<f32>,
}

impl Reading {
    /// The words the network reads in the gesture, best first, at most [`POOL`],
    /// each with its frequency counted `frequency` times.
    /// Sums over alignments, the way the training loss does.
    #[must_use]
    pub fn words<'lexicon>(&self, lexicon: &'lexicon Lexicon, frequency: f32) -> Vec<Candidate<'lexicon>> {
        let classes: Vec<Option<usize>> = lexicon.alphabet().iter().map(|&key| class_of(key)).collect();
        let start = Hypothesis { node: 0, class: 0, blank: 0.0, solid: f32::NEG_INFINITY, total: 0.0 };
        let mut live = vec![start];
        let mut frame = Frame::new();
        let mut leader = 0.0;
        for row in self.rows.as_chunks::<CLASSES>().0 {
            frame.clear();
            let blank_score = row.first().copied().unwrap_or(f32::NEG_INFINITY);
            // The leader staying on a blank is a floor for this frame's best, so
            // what falls below it by the margin would be dropped anyway.
            let worth = leader + blank_score - MARGIN;
            for hypothesis in &live {
                let own = row.get(hypothesis.class).copied().unwrap_or(f32::NEG_INFINITY);
                let Some(stay) = frame.at(hypothesis.node, hypothesis.class) else { continue };
                stay.blank = log_add(stay.blank, hypothesis.total + blank_score);
                if hypothesis.node != 0 {
                    stay.solid = log_add(stay.solid, hypothesis.solid + own);
                }
                for (child, letter) in lexicon.child_ids(hypothesis.node) {
                    let Some(&Some(class)) = classes.get(usize::from(letter)) else { continue };
                    let Some(score) = row.get(class).filter(|score| hypothesis.total + **score >= worth) else { continue };
                    let Some(next) = frame.at(child, class) else { continue };
                    next.solid = log_add(next.solid, hypothesis.total + score);
                }
            }
            let mut best = f32::NEG_INFINITY;
            for hypothesis in &mut frame.hypotheses {
                hypothesis.total = log_add(hypothesis.blank, hypothesis.solid);
                best = best.max(hypothesis.total);
            }
            leader = best;
            live.clear();
            live.extend(frame.hypotheses.iter().filter(|hypothesis| hypothesis.total >= best - MARGIN));
            if live.len() > BEAM {
                live.select_nth_unstable_by(BEAM, |a, b| b.total.total_cmp(&a.total).then(a.node.cmp(&b.node)));
                live.truncate(BEAM);
            }
        }
        let mut found: Vec<Candidate<'lexicon>> = live
            .iter()
            .flat_map(|hypothesis| {
                let score = hypothesis.total;
                lexicon
                    .entries_at(hypothesis.node)
                    .map(move |entry| Candidate { surface: entry.surface, score: score + frequency * entry.weight })
            })
            .collect();
        found.sort_by(best_first);
        // A word with an apostrophe stands on two nodes, and the better one speaks for it.
        let mut seen = HashSet::new();
        found.retain(|candidate| seen.insert(candidate.surface));
        found.truncate(POOL);
        found
    }

    /// The log-probability of one word, for a word the search let go: the better
    /// of its paths, with and without the apostrophe, as in [`Self::words`]. A word
    /// through a key the model does not know gets negative infinity.
    #[must_use]
    pub fn word_score(&self, word: &str) -> f32 {
        let plain = self.path_score(&fold(word));
        apostrophe_path(word).map_or(plain, |keys| plain.max(self.path_score(&keys)))
    }

    fn path_score(&self, keys: &[char]) -> f32 {
        let classes: Option<Vec<usize>> = keys.iter().copied().map(class_of).collect();
        let Some(classes) = classes else {
            return f32::NEG_INFINITY;
        };
        let mut blank = vec![f32::NEG_INFINITY; classes.len() + 1];
        let mut solid = blank.clone();
        if let Some(first) = blank.first_mut() {
            *first = 0.0;
        }
        for row in self.rows.as_chunks::<CLASSES>().0 {
            let blank_score = row.first().copied().unwrap_or(f32::NEG_INFINITY);
            for at in (0..=classes.len()).rev() {
                let total = log_add(blank.get(at).copied().unwrap_or(f32::NEG_INFINITY), solid.get(at).copied().unwrap_or(f32::NEG_INFINITY));
                let own = at.checked_sub(1).and_then(|last| classes.get(last)).and_then(|&class| row.get(class));
                if let Some(slot) = solid.get_mut(at) {
                    *slot = own.map_or(f32::NEG_INFINITY, |own| *slot + own);
                }
                if let (Some(next), Some(slot)) = (classes.get(at).and_then(|&class| row.get(class)), solid.get_mut(at + 1)) {
                    *slot = log_add(*slot, total + next);
                }
                if let Some(slot) = blank.get_mut(at) {
                    *slot = total + blank_score;
                }
            }
        }
        log_add(blank.last().copied().unwrap_or(f32::NEG_INFINITY), solid.last().copied().unwrap_or(f32::NEG_INFINITY))
    }

    /// Ranks the union of what the geometry and the network found. A word only
    /// the network found has no geometric score, so it starts from `mix.floor`,
    /// with its own context.
    #[must_use]
    pub fn blend<'lexicon>(
        &self,
        lexicon: &'lexicon Lexicon,
        geometry: &[Candidate<'lexicon>],
        params: &Params,
        mix: Mix,
        context: impl Fn(&str) -> f32,
    ) -> Vec<Candidate<'lexicon>> {
        let Mix { weight, frequency, floor } = mix;
        let found = self.words(lexicon, frequency);
        // Zero weight on a word the model cannot read must leave its score alone.
        let weighted = |reading: f32| {
            let part = weight * reading;
            if part.is_nan() { 0.0 } else { part }
        };
        let mut blended: Vec<Candidate<'lexicon>> = geometry
            .iter()
            .map(|candidate| {
                let reading = found
                    .iter()
                    .find(|word| word.surface == candidate.surface)
                    .map_or_else(
                        || self.word_score(candidate.surface) + frequency * lexicon.weight(candidate.surface),
                        |word| word.score,
                    );
                Candidate { surface: candidate.surface, score: candidate.score + weighted(reading) }
            })
            .collect();
        for word in &found {
            if !geometry.iter().any(|candidate| candidate.surface == word.surface) {
                let sentence = params.context_weight * context(word.surface);
                blended.push(Candidate { surface: word.surface, score: floor + sentence + weighted(word.score) });
            }
        }
        blended.sort_by(best_first);
        blended
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::layout::Key;

    fn mix(geometry: &[Candidate<'_>], weight: f32, frequency: f32) -> Mix {
        Mix { weight, frequency, floor: floor_of(geometry) }
    }

    /// Rows with one class leading each frame, in turn.
    fn reading_of(keys: &str) -> Reading {
        let labels: Vec<usize> = keys.chars().filter_map(class_of).collect();
        let mut rows = vec![-9.0; CLASSES * FRAMES];
        for (frame, row) in rows.as_chunks_mut::<CLASSES>().0.iter_mut().enumerate() {
            let class = labels.get(frame * labels.len() / FRAMES).copied().unwrap_or(0);
            if let Some(slot) = row.get_mut(class) {
                *slot = 0.0;
            }
        }
        Reading { rows }
    }

    fn lexicon() -> Lexicon {
        Lexicon::new([("cat", 100.0), ("car", 100.0), ("cart", 50.0), ("ou", 40.0), ("où", 200.0)])
    }

    fn layout() -> Layout {
        let keys = "abcdefghijklmnopqrstuvwxyz".chars().enumerate().map(|(index, label)| Key {
            label,
            x: (index % 10) as f32 * 10.0,
            y: (index / 10) as f32 * 10.0,
        });
        Layout::new(keys).unwrap()
    }

    fn line(count: u32) -> Vec<Sample> {
        (0..count).map(|index| Sample { x: index as f32 * 10.0, y: 10.0, t_ms: index * 10 }).collect()
    }

    fn model_file() -> Vec<u8> {
        let mut bytes = MODEL_MAGIC.to_vec();
        bytes.resize(MODEL_MAGIC.len() + MODEL_HEAD_BYTES, 0);
        bytes.push(1);
        bytes
    }

    fn head() -> KeyHead {
        split_model(&model_file()).unwrap().0
    }

    #[derive(Debug)]
    struct Fails;
    impl Encoder for Fails {
        fn encode(&self, _: &[f32], _: &mut [f32]) -> bool {
            false
        }
    }

    #[derive(Debug)]
    struct Writes(f32, AtomicBool);
    impl Encoder for Writes {
        fn encode(&self, input: &[f32], output: &mut [f32]) -> bool {
            self.1.store(input.len() == INPUT_VALUES && output.len() == OUTPUT_VALUES, Ordering::Relaxed);
            output.fill(self.0);
            true
        }
    }

    #[test]
    fn the_search_finds_the_word_the_frames_spell() {
        let lexicon = lexicon();
        let found = reading_of("cat").words(&lexicon, 0.0);
        assert_eq!(found.first().map(|candidate| candidate.surface), Some("cat"));
    }

    #[test]
    fn the_search_agrees_with_the_exact_score() {
        let lexicon = lexicon();
        let reading = reading_of("cat");
        let found = reading.words(&lexicon, 0.0);
        let score = found.first().unwrap().score;
        assert!((score - reading.word_score("cat")).abs() < 1e-3, "{score}");
    }

    #[test]
    fn frequency_separates_words_on_the_same_keys() {
        let lexicon = lexicon();
        let found = reading_of("ou").words(&lexicon, 0.1);
        assert_eq!(found.first().map(|candidate| candidate.surface), Some("où"));
        let unweighted = reading_of("ou").words(&lexicon, 0.0);
        assert_eq!(unweighted.first().map(|candidate| candidate.surface), Some("ou"));
    }

    #[test]
    fn a_word_on_two_paths_comes_once_with_its_better_score() {
        let lexicon = Lexicon::new([("c'est", 100.0), ("cet", 100.0)]);
        for spelled in ["c'est", "cest"] {
            let reading = reading_of(spelled);
            let found = reading.words(&lexicon, 0.0);
            let same: Vec<_> = found.iter().filter(|candidate| candidate.surface == "c'est").collect();
            assert_eq!(same.len(), 1, "{spelled}");
            assert!((same[0].score - reading.word_score("c'est")).abs() < 1e-3, "{spelled}");
        }
    }

    #[test]
    fn a_blend_can_start_from_another_lexicons_floor() {
        let lexicon = lexicon();
        let params = Params::default();
        let geometry: [Candidate<'_>; 0] = [];
        let mix = Mix { weight: 1.0, frequency: 0.0, floor: -40.0 };
        let reading = reading_of("cat");
        let blended = reading.blend(&lexicon, &geometry, &params, mix, |_| 0.0);
        let found = blended.iter().find(|candidate| candidate.surface == "cat").unwrap();
        assert!((found.score - (-40.0 + reading.word_score("cat"))).abs() < 1e-3);
    }

    #[test]
    fn no_geometry_gives_a_floor_of_zero() {
        assert_eq!(floor_of(&[]), 0.0);
        assert_eq!(floor_of(&[Candidate { surface: "a", score: -3.0 }, Candidate { surface: "b", score: -1.0 }]), -3.0);
    }

    #[test]
    fn the_shipped_mix_is_the_tuned_one() {
        let mix = Mix::shipped(&[Candidate { surface: "a", score: -3.0 }]);
        assert_eq!((mix.weight, mix.frequency, mix.floor), (NETWORK_WEIGHT, FREQUENCY_WEIGHT, -3.0));
    }

    #[test]
    fn no_weight_on_an_unreadable_word_leaves_its_score() {
        let lexicon = Lexicon::new([("cat1", 100.0)]);
        let geometry = [Candidate { surface: "cat1", score: 5.0 }];
        let params = Params::default();
        let blended = reading_of("cat").blend(&lexicon, &geometry, &params, mix(&geometry, 0.0, 0.0), |_| 0.0);
        assert_eq!(blended.first().map(|candidate| candidate.score), Some(5.0));
    }

    #[test]
    fn a_word_outside_the_keys_of_the_model_scores_nothing() {
        assert_eq!(reading_of("cat").word_score("cat1"), f32::NEG_INFINITY);
    }

    #[test]
    fn the_blend_brings_in_what_only_the_network_found() {
        let lexicon = lexicon();
        let geometry = [Candidate { surface: "car", score: 5.0 }];
        let params = Params::default();
        let blended = reading_of("cat").blend(&lexicon, &geometry, &params, mix(&geometry, 1.0, 0.0), |_| 0.0);
        assert_eq!(blended.first().map(|candidate| candidate.surface), Some("cat"));
        assert!(blended.iter().any(|candidate| candidate.surface == "car"));
    }

    #[test]
    fn without_weight_the_geometry_decides() {
        let lexicon = lexicon();
        let geometry = [Candidate { surface: "car", score: 5.0 }, Candidate { surface: "ou", score: 1.0 }];
        let params = Params::default();
        let blended = reading_of("cat").blend(&lexicon, &geometry, &params, mix(&geometry, 0.0, 0.0), |_| 0.0);
        assert_eq!(blended.first().map(|candidate| candidate.surface), Some("car"));
    }

    #[test]
    fn context_lifts_a_word_only_the_network_found() {
        let lexicon = lexicon();
        let geometry = [Candidate { surface: "ou", score: 1.0 }];
        let params = Params { context_weight: 1.0, ..Params::default() };
        let lift = |word: &str| if word == "cart" { 90.0 } else { 0.0 };
        let blended = reading_of("cart").blend(&lexicon, &geometry, &params, mix(&geometry, 0.0, 0.0), lift);
        assert_eq!(blended.first().map(|candidate| candidate.surface), Some("cart"));
    }

    #[test]
    fn a_failing_model_leaves_no_reading() {
        let neural = Neural::new(head(), Box::new(Fails));
        assert!(neural.read(&layout(), &line(9)).is_none());
    }

    #[test]
    fn a_model_that_writes_nonsense_leaves_no_reading() {
        let neural = Neural::new(head(), Box::new(Writes(f32::NAN, AtomicBool::new(false))));
        assert!(neural.read(&layout(), &line(9)).is_none());
    }

    #[test]
    fn a_reading_has_one_distribution_per_frame() {
        let encoder = Writes(0.5, AtomicBool::new(false));
        let reading = Neural::new(head(), Box::new(encoder)).read(&layout(), &line(9)).unwrap();
        assert_eq!(reading.rows.len(), CLASSES * FRAMES);
        for row in reading.rows.as_chunks::<CLASSES>().0 {
            let total: f32 = row.iter().map(|value| value.exp()).sum();
            assert!((total - 1.0).abs() < 1e-3, "{total}");
        }
    }

    #[test]
    fn a_key_missing_from_the_layout_can_never_be_read() {
        let keys = [Key { label: 'a', x: 0.0, y: 0.0 }, Key { label: 'b', x: 10.0, y: 0.0 }];
        let layout = Layout::new(keys).unwrap();
        let reading = Neural::new(head(), Box::new(Writes(0.5, AtomicBool::new(false)))).read(&layout, &line(9)).unwrap();
        let (present, absent) = (class_of('a').unwrap(), class_of('c').unwrap());
        let row = reading.rows.as_chunks::<CLASSES>().0.first().unwrap();
        assert!(row.get(absent).unwrap() + 100.0 < *row.get(present).unwrap());
    }

    #[test]
    fn one_sample_cannot_be_read() {
        let neural = Neural::new(head(), Box::new(Writes(0.5, AtomicBool::new(false))));
        assert!(neural.read(&layout(), &line(1)).is_none());
        assert!(neural.read(&layout(), &[]).is_none());
    }

    #[test]
    fn a_still_finger_gives_finite_features() {
        let samples = [Sample { x: 5.0, y: 5.0, t_ms: 0 }, Sample { x: 5.0, y: 5.0, t_ms: 0 }];
        let input = features(&samples, 10.0, [0.0, 0.0]).unwrap();
        assert_eq!(input.len(), INPUT_VALUES);
        assert!(input.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn features_follow_the_gesture_in_key_pitches() {
        let input = features(&line(3), 10.0, [1.0, 0.0]).unwrap();
        // x runs from 0 to 2 pitches, less the center, then the scale.
        assert!((input.first().unwrap() + POSITION_SCALE).abs() < 1e-5);
        assert!((input.get(FRAMES - 1).unwrap() - POSITION_SCALE).abs() < 1e-5);
        // y is the second feature and stays at 1 pitch.
        assert!((input.get(FRAMES).unwrap() - POSITION_SCALE).abs() < 1e-5);
    }

    fn digest() -> Vec<f32> {
        let curve = [(0, 0, 0), (13, 4, 20), (31, 2, 35), (44, 19, 70), (52, 41, 71), (70, 44, 130)];
        let samples = curve.map(|(x, y, t_ms)| Sample { x: x as f32, y: y as f32, t_ms });
        let input = features(&samples, 10.0, [2.0, 1.0]).unwrap();
        input
            .chunks(FRAMES)
            .map(|feature| feature.iter().enumerate().map(|(frame, value)| (frame + 1) as f32 * value).sum())
            .collect()
    }

    /// Pinned after `neural-train parity` showed these features within 1.7e-4 of
    /// the ones the model was trained on, over 11,831 held-out gestures.
    #[test]
    fn features_stay_what_the_model_was_trained_on() {
        let pinned: [f32; FEATURES] = [1_226.826_8, 805.460_27, 820.506_16, 620.438_5, -55.999_996, -35.2, 1_038.676_5];
        for (value, expected) in digest().iter().zip(pinned) {
            assert!((value - expected).abs() < 1e-2, "{value} against {expected}");
        }
    }

    #[test]
    fn a_frame_finds_every_node_back_however_they_collide() {
        let mut frame = Frame::new();
        for node in 0..3000 {
            frame.at(node * 4096, 1).unwrap().blank = node as f32;
        }
        for node in 0..3000 {
            assert_eq!(frame.at(node * 4096, 1).unwrap().blank, node as f32);
        }
        assert_eq!(frame.hypotheses.len(), 3000);
        frame.clear();
        assert_eq!(frame.at(4096, 1).unwrap().blank, f32::NEG_INFINITY);
    }

    #[test]
    fn a_hopeless_word_leaves_the_search() {
        let lexicon = Lexicon::new([("cat", 1.0), ("dog", 1.0)]);
        let found = reading_of("cat").words(&lexicon, 0.0);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn the_blend_scores_are_exactly_the_sum_of_their_parts() {
        let lexicon = lexicon();
        let params = Params { context_weight: 2.0, ..Params::default() };
        let reading = reading_of("cat");
        let geometry = [Candidate { surface: "car", score: 5.0 }, Candidate { surface: "cat", score: 3.0 }];
        let blended = reading.blend(&lexicon, &geometry, &params, mix(&geometry, 0.75, 0.25), |_| 7.0);
        let score = |word: &str| blended.iter().find(|candidate| candidate.surface == word).unwrap().score;
        let weight = |word: &str| lexicon.weight(word);
        assert_eq!(weight("cat"), 100.0);
        assert_eq!(score("car"), 5.0 + 0.75 * (reading.word_score("car") + 0.25 * weight("car")));
        assert!((score("cat") - (3.0 + 0.75 * (reading.word_score("cat") + 0.25 * weight("cat")))).abs() < 1e-3);
        let alone = reading.words(&lexicon, 0.25).into_iter().find(|word| word.surface == "cart");
        if let Some(word) = alone {
            assert_eq!(score("cart"), 3.0 + 2.0 * 7.0 + 0.75 * word.score);
        }
    }

    #[test]
    fn keys_are_placed_around_the_middle_of_the_layout_in_pitches() {
        let keys = [
            Key { label: 'a', x: 0.0, y: 0.0 },
            Key { label: 'b', x: 10.0, y: 0.0 },
            Key { label: 'c', x: 20.0, y: 30.0 },
            Key { label: 'é', x: 0.0, y: 10.0 },
        ];
        let places = KeyPlaces::of(&Layout::new(keys).unwrap()).unwrap();
        assert_eq!(places.center, [1.0, 1.0]);
        assert_eq!(places.places[0], Some([-POSITION_SCALE, -POSITION_SCALE]));
        assert_eq!(places.places[2], Some([POSITION_SCALE, 2.0 * POSITION_SCALE]));
        assert_eq!(places.places[3], None);
    }

    /// Writes one query value and one blank value, each at a known frame.
    #[derive(Debug)]
    struct OneQuery;
    impl Encoder for OneQuery {
        fn encode(&self, _: &[f32], output: &mut [f32]) -> bool {
            output.fill(0.0);
            output[3 * FRAMES + 5] = 2.0;
            output[KEY_DIMENSION * FRAMES + 6] = 4.0;
            true
        }
    }

    #[test]
    fn a_key_scores_what_its_description_shares_with_the_query() {
        let mut bytes = model_file();
        // Second bias of dimension 3 is 1.5, so every key is described by 1.5 there.
        let at = MODEL_MAGIC.len() + (HEAD_VALUES - KEY_DIMENSION + 3) * F32_BYTES;
        bytes.get_mut(at..at + F32_BYTES).unwrap().copy_from_slice(&1.5_f32.to_le_bytes());
        let (head, _) = split_model(&bytes).unwrap();
        let reading = Neural::new(head, Box::new(OneQuery)).read(&layout(), &line(9)).unwrap();
        let rows = reading.rows.as_chunks::<CLASSES>().0;
        let a = class_of('a').unwrap();
        // Frame 5: key a has logit 3 against a blank of 0.
        assert!((rows[5][a] - rows[5][0] - 3.0).abs() < 1e-3);
        // Frame 6: the blank has logit 4 against keys at 0.
        assert!((rows[6][0] - rows[6][a] - 4.0).abs() < 1e-3);
        assert!((rows[7][0] - rows[7][a]).abs() < 1e-3);
    }

    #[test]
    fn the_model_file_rejects_what_it_should() {
        assert!(split_model(b"").is_none());
        assert!(split_model(b"SYNEURO1short").is_none());
        let mut no_model = model_file();
        no_model.pop();
        assert!(split_model(&no_model).is_none());
        let mut wrong = model_file();
        *wrong.first_mut().unwrap() = 0;
        assert!(split_model(&wrong).is_none());
        let mut not_finite = model_file();
        not_finite.get_mut(MODEL_MAGIC.len()..MODEL_MAGIC.len() + F32_BYTES).unwrap().copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(split_model(&not_finite).is_none());
        assert_eq!(split_model(&model_file()).unwrap().1, [1]);
    }

    #[test]
    fn the_key_head_is_two_layers_with_a_rectifier() {
        let mut values = vec![0.0; HEAD_VALUES];
        // First layer: hidden 0 is 2x + 1; hidden 1 is -5 and rectifies to 0.
        *values.first_mut().unwrap() = 2.0;
        *values.get_mut(FIRST_WEIGHTS).unwrap() = 1.0;
        *values.get_mut(FIRST_WEIGHTS + 1).unwrap() = -5.0;
        // Second layer: output 3 is 3 times hidden 0 and 7 times hidden 1, plus 1.
        let second = FIRST_WEIGHTS + KEY_DIMENSION;
        *values.get_mut(second + 3).unwrap() = 3.0;
        *values.get_mut(second + KEY_DIMENSION + 3).unwrap() = 7.0;
        *values.get_mut(second + SECOND_WEIGHTS + 3).unwrap() = 1.0;
        let out = KeyHead { values }.describe([2.0, 0.0]).unwrap();
        assert_eq!(*out.get(3).unwrap(), 3.0 * 5.0 + 1.0);
        assert_eq!(*out.first().unwrap(), 0.0);
    }

    #[test]
    fn the_table_stays_close_to_the_exact_sum() {
        for step in 0..1200 {
            let gap = step as f32 / 100.0;
            let exact = (-gap).exp().ln_1p();
            assert!((log_add(0.0, -gap) - exact).abs() < 1e-2, "{gap}");
            assert!((log_add(-gap, 0.0) - exact).abs() < 1e-2, "{gap}");
        }
    }

    #[test]
    fn lean_changes_only_the_effort() {
        let params = Params::default();
        let lean = lean(&params);
        assert_eq!((lean.refine, lean.rerank, lean.coarse_points), (LEAN_REFINE, LEAN_RERANK, LEAN_COARSE_POINTS));
        assert_eq!(lean.lambda, params.lambda);
        assert_eq!(lean.context_weight, params.context_weight * LEAN_CONTEXT_BOOST);
    }

    #[test]
    fn the_table_keeps_the_error_it_states() {
        const STATED: f32 = 0.0039;
        let worst = (0..1200_u16).map(|step| {
            let gap = f32::from(step) / 100.0;
            (log_add(0.0, -gap) - (-gap).exp().ln_1p()).abs()
        });
        let worst = worst.fold(0.0, f32::max);
        assert!(worst <= STATED + 1e-4, "{worst}");
    }

    #[test]
    fn logs_add_without_inventing_a_nan() {
        assert_eq!(log_add(f32::NEG_INFINITY, f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert!(log_add(-1.0, -1.0) > -0.4);
        assert_eq!(log_add(0.0, -50.0), 0.0);
        assert_eq!(log_add(-50.0, 0.0), 0.0);
    }
}
