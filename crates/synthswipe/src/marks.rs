//! Deliberate marks inside a gesture. A pause says "this letter is in the
//! word" where the straight path would skip it (`pue` over `pe`), and a loop
//! says "this letter is doubled" (`saucisse`). Both come from sample positions
//! and timestamps alone, so no clock is involved.

use std::f32::consts::PI;

use crate::geometry::{Point, Sample, distance};
use crate::layout::Layout;
use crate::lexicon::raw_keys;

/// Time at each end of a gesture where a slow finger means nothing.
const EDGE_MS: u32 = 120;
const PAUSE_MS: u32 = 110;
/// Long enough to be a decision rather than a slowdown.
const STOP_MS: u32 = 150;
/// How many times longer than the usual key a stop lasts.
const STOP_RATIO: u32 = 3;
/// How far a gesture may leave its chord and still count as one straight stroke.
const STRAIGHT_KEYS: f32 = 0.7;
/// Cost of each inner letter a word has beyond the keys stopped on.
const UNSTOPPED_LETTER: f32 = 6.0;
const STOP_REWARD: f32 = 6.0;
const PAUSE_RADIUS_KEYS: f32 = 0.35;
const LOOP_RADIUS_KEYS: f32 = 1.0;
const LOOP_TURN: f32 = 1.6 * PI;
const MIN_SEGMENT_KEYS: f32 = 0.03;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// The finger slowed on this key on its way through.
    Pause(char),
    /// The finger clearly stopped on this key: the word is spelled by its
    /// ends and its stops, and what the stroke crossed meanwhile is transit.
    Stop(char),
    /// The finger circled on this key.
    Loop(char),
}

fn point(sample: &Sample) -> Point {
    [sample.x, sample.y]
}

fn nearest(layout: &Layout, at: Point) -> Option<char> {
    layout.keys_near(at, 0.0).first().copied()
}

fn pauses(samples: &[Sample], layout: &Layout, out: &mut Vec<Mark>) {
    let radius = PAUSE_RADIUS_KEYS * layout.pitch();
    let last = samples.last().map_or(0, |sample| sample.t_ms);
    let mut start = 0;
    while let Some(anchor) = samples.get(start) {
        let held = samples
            .iter()
            .skip(start)
            .take_while(|sample| distance(point(sample), point(anchor)) <= radius)
            .count();
        let end = samples.get(start + held.saturating_sub(1)).map_or(anchor.t_ms, |sample| sample.t_ms);
        let inside = start > 0 && end.saturating_add(EDGE_MS) <= last;
        if inside && end.saturating_sub(anchor.t_ms) >= PAUSE_MS
            && let Some(letter) = nearest(layout, point(anchor)) {
                out.push(Mark::Pause(letter));
            }
        start += held.max(1);
    }
}

/// Whether the whole gesture stays close to the line from its start to its
/// end. Only such a stroke is ambiguous about which keys it meant.
fn is_straight(samples: &[Sample], pitch: f32) -> bool {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return false;
    };
    let (a, b) = (point(first), point(last));
    let length = distance(a, b);
    if length < 2.0 * pitch {
        return false;
    }
    samples.iter().all(|sample| {
        let p = point(sample);
        let cross = (b[0] - a[0]) * (a[1] - p[1]) - (a[0] - p[0]) * (b[1] - a[1]);
        cross.abs() / length <= STRAIGHT_KEYS * pitch
    })
}

/// On a straight stroke, the keys where the finger lingered far longer than
/// on the others. Time on a key is scale-free: it holds for fast and slow users.
fn stops(samples: &[Sample], layout: &Layout, out: &mut Vec<Mark>) {
    if !is_straight(samples, layout.pitch()) {
        return;
    }
    let mut visits: Vec<(char, u32)> = Vec::new();
    for pair in samples.windows(2) {
        let [a, b] = pair else { continue };
        let Some(letter) = nearest(layout, point(a)) else {
            continue;
        };
        let spent = b.t_ms.saturating_sub(a.t_ms);
        match visits.last_mut() {
            Some((current, total)) if *current == letter => *total = total.saturating_add(spent),
            _ => visits.push((letter, spent)),
        }
    }
    let inner = visits.get(1..visits.len().saturating_sub(1)).unwrap_or_default();
    let mut times: Vec<u32> = inner.iter().map(|visit| visit.1).collect();
    times.sort_unstable();
    let usual = times.get(times.len() / 2).copied().unwrap_or(0);
    for &(letter, spent) in inner {
        if spent >= STOP_MS && spent >= usual.saturating_mul(STOP_RATIO) {
            out.push(Mark::Stop(letter));
        }
    }
}

fn turning(a: Point, b: Point, c: Point) -> f32 {
    let first = (b[1] - a[1]).atan2(b[0] - a[0]);
    let second = (c[1] - b[1]).atan2(c[0] - b[0]);
    let turn = second - first;
    (turn + PI).rem_euclid(2.0 * PI) - PI
}

fn loops(samples: &[Sample], layout: &Layout, out: &mut Vec<Mark>) {
    let radius = LOOP_RADIUS_KEYS * layout.pitch();
    let step = MIN_SEGMENT_KEYS * layout.pitch();
    let mut path: Vec<Point> = Vec::with_capacity(samples.len());
    for sample in samples {
        if path.last().is_none_or(|&last| distance(last, point(sample)) >= step) {
            path.push(point(sample));
        }
    }
    let mut start = 0;
    while let Some(&anchor) = path.get(start) {
        let mut turn = 0.0_f32;
        let mut end = start;
        for triple in path.get(start..).unwrap_or_default().windows(3) {
            let [a, b, c] = triple else { continue };
            if distance(*c, anchor) > radius {
                break;
            }
            turn += turning(*a, *b, *c);
            end += 1;
            if turn.abs() >= LOOP_TURN {
                break;
            }
        }
        if turn.abs() >= LOOP_TURN {
            let span = path.get(start..=end + 1).unwrap_or_default();
            let count = span.iter().map(|_| 1.0_f32).sum::<f32>().max(1.0);
            let center = [
                span.iter().map(|p| p[0]).sum::<f32>() / count,
                span.iter().map(|p| p[1]).sum::<f32>() / count,
            ];
            if let Some(letter) = nearest(layout, center) {
                out.push(Mark::Loop(letter));
            }
            start = end + 2;
        } else {
            start += 1;
        }
    }
}

/// Where the gesture means something: the finger slowed down or turned. Fast
/// stretches between these points are transit, however many keys they cross.
pub(crate) fn intent_points(samples: &[Sample], pitch: f32) -> Vec<Point> {
    // Set by replaying the owner's gestures: distances in key pitches, the turn
    // in radians, the speed as a share of the gesture's usual speed.
    const SPAN_KEYS: f32 = 0.7;
    const MIN_TURN: f32 = 0.9;
    const SLOW_RATIO: f32 = 0.45;
    const SPACING_KEYS: f32 = 0.9;
    let span = SPAN_KEYS * pitch;

    let speeds: Vec<f32> = samples
        .windows(3)
        .map(|triple| match triple {
            [a, _, c] => {
                let elapsed = c.t_ms.saturating_sub(a.t_ms).max(1);
                distance(point(a), point(c)) / elapsed as f32
            }
            _ => 0.0,
        })
        .collect();
    let mut sorted = speeds.clone();
    sorted.sort_by(f32::total_cmp);
    let usual = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);

    let mut found: Vec<Point> = Vec::new();
    for (index, sample) in samples.iter().enumerate().skip(1) {
        let here = point(sample);
        let back = samples.get(..index).unwrap_or_default();
        let ahead = samples.get(index + 1..).unwrap_or_default();
        let before = back.iter().rev().find(|s| distance(point(s), here) >= span);
        let after = ahead.iter().find(|s| distance(point(s), here) >= span);
        let (Some(before), Some(after)) = (before, after) else {
            continue;
        };
        let turn = turning(point(before), here, point(after)).abs();
        let slow = speeds.get(index.saturating_sub(1)).is_some_and(|&speed| speed < usual * SLOW_RATIO);
        if (turn >= MIN_TURN || slow) && found.last().is_none_or(|&last| distance(last, here) >= SPACING_KEYS * pitch) {
            found.push(here);
        }
    }
    found
}

/// The mean squared distance, in squared pitches, from each intent point to
/// the nearest key of a word: what the word leaves unexplained.
pub(crate) fn unexplained(points: &[Point], keys: &[Point], pitch: f32) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for &intent in points {
        let nearest = keys
            .iter()
            .map(|&key| distance(intent, key) / pitch)
            .fold(f32::INFINITY, f32::min);
        if nearest.is_finite() {
            total += nearest * nearest;
            count += 1.0;
        }
    }
    if count > 0.0 { total / count } else { 0.0 }
}

/// How many sharp turns of a word's path the gesture never made. `porte` goes
/// back from `r` to `t`, so a straight stroke along the row cannot be `porte`.
pub(crate) fn missing_turns(keys: &[Point], intent: &[Point], pitch: f32) -> f32 {
    // Only long legs and near reversals count: small turns are lost in any
    // real gesture. Distances in key pitches, the turn in radians.
    const MIN_LEG_KEYS: f32 = 0.8;
    const MIN_TURN: f32 = 2.0;
    const MATCH_KEYS: f32 = 1.5;
    let mut missing = 0.0;
    for triple in keys.windows(3) {
        let [a, b, c] = triple else { continue };
        let long_legs = distance(*a, *b) >= MIN_LEG_KEYS * pitch && distance(*b, *c) >= MIN_LEG_KEYS * pitch;
        if long_legs
            && turning(*a, *b, *c).abs() >= MIN_TURN
            && !intent.iter().any(|&point| distance(point, *b) <= MATCH_KEYS * pitch)
        {
            missing += 1.0;
        }
    }
    missing
}

/// Finds the marks of a gesture.
pub(crate) fn detect(samples: &[Sample], layout: &Layout) -> Vec<Mark> {
    let mut marks = Vec::new();
    pauses(samples, layout, &mut marks);
    stops(samples, layout, &mut marks);
    loops(samples, layout, &mut marks);
    marks
}

/// How well a word explains the marks: positive when it does, negative when a
/// marked letter is absent from it.
pub(crate) fn agreement(word: &str, marks: &[Mark]) -> f32 {
    if marks.is_empty() {
        return 0.0;
    }
    let keys = raw_keys(word);
    let inner = keys.get(1..keys.len().saturating_sub(1)).unwrap_or_default();
    let stopped: Vec<char> = marks
        .iter()
        .filter_map(|mark| match *mark {
            Mark::Stop(letter) => Some(letter),
            _ => None,
        })
        .collect();
    let transit = if stopped.is_empty() {
        0.0
    } else {
        inner.iter().filter(|letter| !stopped.contains(letter)).map(|_| UNSTOPPED_LETTER).sum()
    };
    let marked: f32 = marks
        .iter()
        .map(|mark| match *mark {
            Mark::Stop(letter) => {
                if inner.contains(&letter) {
                    STOP_REWARD
                } else if keys.contains(&letter) {
                    0.0
                } else {
                    -STOP_REWARD
                }
            }
            Mark::Pause(letter) => {
                if inner.contains(&letter) {
                    1.0
                } else if keys.contains(&letter) {
                    0.0
                } else {
                    -1.0
                }
            }
            Mark::Loop(letter) => {
                if keys.windows(2).any(|pair| pair == [letter, letter]) {
                    2.0
                } else if keys.contains(&letter) {
                    0.0
                } else {
                    -1.0
                }
            }
        })
        .sum();
    marked - transit
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::testkit::grid;


    fn line(from: [f32; 2], to: [f32; 2], steps: u16, ms_each: u32, t: &mut u32, out: &mut Vec<Sample>) {
        for step in 0..=steps {
            let ratio = f32::from(step) / f32::from(steps.max(1));
            out.push(Sample {
                x: from[0] + (to[0] - from[0]) * ratio,
                y: from[1] + (to[1] - from[1]) * ratio,
                t_ms: *t,
            });
            *t += ms_each;
        }
    }

    #[test]
    fn a_stop_on_a_straight_stroke_is_found() {
        let Some(layout) = grid() else {
            return;
        };
        let (mut samples, mut t) = (Vec::new(), 0);
        line([9.0, 0.0], [6.3, 0.0], 15, 8, &mut t, &mut samples);
        line([6.3, 0.0], [5.7, 0.0], 25, 8, &mut t, &mut samples);
        line([5.7, 0.0], [2.0, 0.0], 20, 8, &mut t, &mut samples);
        assert!(detect(&samples, &layout).contains(&Mark::Stop('u')));
    }

    #[test]
    fn a_slow_corner_is_not_a_stop() {
        let Some(layout) = grid() else {
            return;
        };
        let (mut samples, mut t) = (Vec::new(), 0);
        line([0.0, 0.0], [6.0, 0.0], 20, 8, &mut t, &mut samples);
        line([6.0, 0.0], [6.1, 0.1], 30, 8, &mut t, &mut samples);
        line([6.0, 0.0], [3.0, 2.0], 20, 8, &mut t, &mut samples);
        assert!(!detect(&samples, &layout).iter().any(|mark| matches!(mark, Mark::Stop(_))));
    }

    #[test]
    fn a_steady_swipe_has_no_marks() {
        let Some(layout) = grid() else {
            return;
        };
        let (mut samples, mut t) = (Vec::new(), 0);
        line([9.0, 0.0], [2.0, 0.0], 40, 8, &mut t, &mut samples);
        line([2.0, 0.0], [2.0, 2.0], 20, 8, &mut t, &mut samples);
        assert_eq!(detect(&samples, &layout), []);
    }

    #[test]
    fn a_circle_on_a_key_is_a_loop() {
        let Some(layout) = grid() else {
            return;
        };
        let (mut samples, mut t) = (Vec::new(), 0);
        line([0.0, 1.0], [3.0, 1.0], 20, 8, &mut t, &mut samples);
        for step in 0..=24_u16 {
            let angle = f32::from(step) / 24.0 * 2.0 * PI;
            samples.push(Sample { x: 3.0 - 0.3 * angle.cos() + 0.3, y: 1.0 + 0.3 * angle.sin(), t_ms: t });
            t += 8;
        }
        line([3.0, 1.0], [9.0, 1.0], 30, 8, &mut t, &mut samples);
        assert!(detect(&samples, &layout).contains(&Mark::Loop('f')));
    }

    #[test]
    fn a_turn_and_a_slowdown_are_intent_but_transit_is_not() {
        let (mut samples, mut t) = (Vec::new(), 0);
        line([0.0, 0.0], [5.0, 0.0], 25, 8, &mut t, &mut samples);
        line([5.0, 0.0], [5.0, 3.0], 15, 8, &mut t, &mut samples);
        let points = intent_points(&samples, 1.0);
        assert_eq!(points.len(), 1);
        assert!(distance(points[0], [5.0, 0.0]) < 0.5);

        let (mut straight, mut t) = (Vec::new(), 0);
        line([0.0, 0.0], [9.0, 0.0], 45, 8, &mut t, &mut straight);
        assert_eq!(intent_points(&straight, 1.0), Vec::<Point>::new());
    }

    #[test]
    fn a_word_that_doubles_back_needs_a_gesture_that_did() {
        let straight = [[9.0, 0.0], [8.0, 0.0], [3.0, 0.0], [4.0, 0.0], [2.0, 0.0]];
        assert_eq!(missing_turns(&straight, &[], 1.0) as u8, 2);
        assert_eq!(missing_turns(&straight, &[[3.0, 0.0], [4.0, 0.0]], 1.0) as u8, 0);
        let direct = [[9.0, 0.0], [6.0, 0.0], [2.0, 0.0]];
        assert_eq!(missing_turns(&direct, &[], 1.0) as u8, 0);
    }

    #[test]
    fn a_word_passing_by_an_intent_point_explains_it() {
        let intent = [[5.0, 0.0]];
        let near = [[0.0, 0.0], [5.0, 0.0]];
        let far = [[0.0, 0.0], [2.0, 0.0]];
        assert!(unexplained(&intent, &near, 1.0) < unexplained(&intent, &far, 1.0));
    }

    #[test]
    fn a_clear_stop_spells_pue_and_not_pure_or_pour() {
        let marks = [Mark::Stop('u')];
        assert!(agreement("pue", &marks) > agreement("pure", &marks));
        assert!(agreement("pue", &marks) > agreement("pour", &marks));
        assert!(agreement("pue", &marks) > agreement("porte", &marks));
    }

    #[test]
    fn words_are_rewarded_for_explaining_marks() {
        assert!(agreement("pue", &[Mark::Pause('u')]) > agreement("pe", &[Mark::Pause('u')]));
        assert!(agreement("saucisse", &[Mark::Loop('s')]) > agreement("saucise", &[Mark::Loop('s')]));
        assert!(agreement("été", &[]).abs() < f32::EPSILON);
    }
}
