/// One sampled point of a gesture, in the same units as its layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Horizontal position.
    pub x: f32,
    /// Vertical position.
    pub y: f32,
    /// Milliseconds since the gesture started.
    pub t_ms: u32,
}

pub(crate) type Point = [f32; 2];

/// Largest accepted coordinate. Any real screen fits well inside it, and it
/// keeps every squared distance in the decoder finite, so scores never reach NaN.
const MAX_COORDINATE: f32 = 1e6;

/// Below this size a shape counts as a point, since dividing by a subnormal
/// side overflows to infinity.
const MIN_SHAPE_SIDE: f32 = 1e-6;

/// Whether a coordinate is finite and small enough to compute with safely.
pub(crate) fn is_usable(value: f32) -> bool {
    value.abs() <= MAX_COORDINATE
}

pub(crate) fn distance([ax, ay]: Point, [bx, by]: Point) -> f32 {
    // Not `hypot`: its overflow protection costs far more than this hot path
    // can afford, and `is_usable` already bounds every coordinate.
    let (dx, dy) = (ax - bx, ay - by);
    (dx * dx + dy * dy).sqrt()
}

fn path_length(path: &[Point]) -> f32 {
    path.windows(2)
        .map(|pair| match pair {
            [a, b] => distance(*a, *b),
            _ => 0.0,
        })
        .sum()
}

/// Rewrites `out` as `count` points evenly spaced along `path`.
pub(crate) fn resample(path: &[Point], count: u16, out: &mut Vec<Point>) {
    out.clear();
    let Some(&first) = path.first() else {
        return;
    };
    let count_usize = usize::from(count);
    let length = path_length(path);
    let step = length / f32::from(count.saturating_sub(1).max(1));
    out.push(first);
    if step > 0.0 {
        let mut walked = 0.0;
        let mut target = step;
        for pair in path.windows(2) {
            let [a, b] = pair else { continue };
            let segment = distance(*a, *b);
            if segment <= 0.0 {
                continue;
            }
            while walked + segment >= target && out.len() < count_usize {
                let ratio = (target - walked) / segment;
                out.push([a[0] + (b[0] - a[0]) * ratio, a[1] + (b[1] - a[1]) * ratio]);
                target += step;
            }
            walked += segment;
        }
    }
    let last = path.last().copied().unwrap_or(first);
    out.resize(count_usize, last);
}

/// Translates to the centroid and scales the larger bounding box side to 1.
pub(crate) fn normalize_shape(points: &[Point], out: &mut Vec<Point>) {
    out.clear();
    let [mut min_x, mut min_y] = [f32::INFINITY; 2];
    let [mut max_x, mut max_y] = [f32::NEG_INFINITY; 2];
    let [mut sum_x, mut sum_y] = [0.0_f32; 2];
    for &[x, y] in points {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
        sum_x += x;
        sum_y += y;
    }
    let side = (max_x - min_x).max(max_y - min_y);
    let scale = if side > MIN_SHAPE_SIDE { 1.0 / side } else { 1.0 };
    let total = points.iter().map(|_| 1.0_f32).sum::<f32>().max(1.0);
    let (center_x, center_y) = (sum_x / total, sum_y / total);
    out.extend(
        points
            .iter()
            .map(|&[x, y]| [(x - center_x) * scale, (y - center_y) * scale]),
    );
}

/// Points in a gesture signature.
const SIGNATURE_POINTS: u16 = 16;

/// A compact, layout-relative form of a gesture, to compare later gestures to.
#[must_use]
pub fn signature(samples: &[Sample], pitch: f32) -> Vec<[f32; 2]> {
    let path: Vec<Point> = samples.iter().map(|s| [s.x / pitch, s.y / pitch]).collect();
    let mut out = Vec::new();
    resample(&path, SIGNATURE_POINTS, &mut out);
    out
}

/// The mean distance between two signatures, in key pitches.
#[must_use]
pub fn signature_distance(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    let pairs = a.len().min(b.len()).max(1);
    let total: f32 = a.iter().zip(b).map(|(&p, &q)| distance(p, q)).sum();
    total / f32::from(u16::try_from(pairs).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_spaces_points_evenly() {
        let mut out = Vec::new();
        resample(&[[0.0, 0.0], [10.0, 0.0]], 11, &mut out);
        assert_eq!(out.len(), 11);
        for (index, point) in out.iter().enumerate() {
            let expected = f32::from(u16::try_from(index).unwrap_or(0));
            assert!((point[0] - expected).abs() < 1e-3);
        }
    }

    #[test]
    fn resample_of_a_stationary_path_repeats_the_point() {
        let mut out = Vec::new();
        resample(&[[3.0, 4.0], [3.0, 4.0]], 5, &mut out);
        assert_eq!(out, vec![[3.0, 4.0]; 5]);
    }

    #[test]
    fn normalize_shape_stays_finite_on_a_subnormal_gesture() {
        let mut out = Vec::new();
        normalize_shape(&[[0.0, 0.0], [1e-44, 0.0]], &mut out);
        assert!(out.iter().flatten().all(|value| value.is_finite()));
    }

    #[test]
    fn normalize_shape_ignores_position_and_scale() {
        let mut small = Vec::new();
        let mut large = Vec::new();
        normalize_shape(&[[0.0, 0.0], [1.0, 1.0]], &mut small);
        normalize_shape(&[[50.0, 50.0], [90.0, 90.0]], &mut large);
        assert_eq!(small, large);
    }
}
