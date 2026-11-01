use std::collections::HashMap;
use std::fmt;

use crate::geometry::{Point, distance, is_usable};

const MIN_PITCH: f32 = 1e-3;

/// One key, positioned by its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// The base letter this key produces.
    pub label: char,
    /// Horizontal centre.
    pub x: f32,
    /// Vertical centre.
    pub y: f32,
}

/// A keyboard described by data, so that adding a layout never needs code.
#[derive(Debug, Clone)]
pub struct Layout {
    by_label: HashMap<char, Point>,
    pitch: f32,
}

/// Why a layout was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// Fewer than two distinct keys.
    TooFewKeys,
    /// A coordinate is NaN, infinite or absurdly large.
    UnusableCoordinate,
    /// Keys are too close together to divide distances by their spacing.
    KeysTooClose,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooFewKeys => "layout needs at least two distinct keys",
            Self::UnusableCoordinate => "layout has an unusable coordinate",
            Self::KeysTooClose => "layout keys are too close together",
        })
    }
}

impl std::error::Error for LayoutError {}

impl Layout {
    /// Builds a layout. When a label repeats, its first key wins.
    ///
    /// # Errors
    /// Returns an error on non-finite coordinates or fewer than two keys.
    pub fn new(keys: impl IntoIterator<Item = Key>) -> Result<Self, LayoutError> {
        let mut by_label = HashMap::new();
        for key in keys {
            if !is_usable(key.x) || !is_usable(key.y) {
                return Err(LayoutError::UnusableCoordinate);
            }
            for label in key.label.to_lowercase() {
                by_label.entry(label).or_insert([key.x, key.y]);
            }
        }
        if by_label.len() < 2 {
            return Err(LayoutError::TooFewKeys);
        }
        let mut gaps: Vec<f32> = by_label
            .values()
            .filter_map(|&center| {
                by_label
                    .values()
                    .map(|&other| distance(center, other))
                    .filter(|&gap| gap > 0.0)
                    .min_by(f32::total_cmp)
            })
            .collect();
        gaps.sort_by(f32::total_cmp);
        let pitch = gaps
            .get(gaps.len() / 2)
            .copied()
            .ok_or(LayoutError::TooFewKeys)?;
        if pitch < MIN_PITCH {
            return Err(LayoutError::KeysTooClose);
        }
        Ok(Self { by_label, pitch })
    }

    /// The typical distance between neighbouring keys.
    #[must_use]
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    #[must_use]
    pub fn center(&self, label: char) -> Option<Point> {
        self.by_label.get(&label).copied()
    }

    /// Labels of the keys within `radius` of `point`, or the nearest key if none is.
    pub(crate) fn keys_near(&self, point: Point, radius: f32) -> Vec<char> {
        let near: Vec<char> = self
            .by_label
            .iter()
            .filter(|&(_, &center)| distance(center, point) <= radius)
            .map(|(&label, _)| label)
            .collect();
        if !near.is_empty() {
            return near;
        }
        self.by_label
            .iter()
            .min_by(|a, b| distance(*a.1, point).total_cmp(&distance(*b.1, point)))
            .map(|(&label, _)| label)
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(label: char, x: f32) -> Key {
        Key { label, x, y: 0.0 }
    }

    #[test]
    fn pitch_is_the_neighbour_gap() {
        let layout = Layout::new([key('a', 0.0), key('b', 2.0), key('c', 4.0)]);
        assert_eq!(layout.map(|layout| layout.pitch()), Ok(2.0));
    }

    #[test]
    fn rejects_nan_and_degenerate_layouts() {
        assert_eq!(
            Layout::new([key('a', f32::NAN), key('b', 1.0)]).map(|_| ()),
            Err(LayoutError::UnusableCoordinate)
        );
        assert_eq!(
            Layout::new([key('a', 1e30), key('b', 1.0)]).map(|_| ()),
            Err(LayoutError::UnusableCoordinate)
        );
        assert_eq!(
            Layout::new([key('a', 0.0), key('b', 1e-6)]).map(|_| ()),
            Err(LayoutError::KeysTooClose)
        );
        assert_eq!(
            Layout::new([key('a', 0.0)]).map(|_| ()),
            Err(LayoutError::TooFewKeys)
        );
        assert_eq!(
            Layout::new([key('a', 0.0), key('b', 0.0)]).map(|_| ()),
            Err(LayoutError::TooFewKeys)
        );
    }

    #[test]
    fn keys_near_falls_back_to_the_nearest_key() {
        let Ok(layout) = Layout::new([key('a', 0.0), key('b', 2.0), key('c', 4.0)]) else {
            return;
        };
        assert_eq!(layout.keys_near([2.1, 0.0], 0.5), ['b']);
        assert_eq!(layout.keys_near([50.0, 0.0], 0.5), ['c']);
    }

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(LayoutError::KeysTooClose.to_string(), "layout keys are too close together");
    }

    #[test]
    fn labels_are_lowercased() {
        let layout = Layout::new([key('A', 0.0), key('B', 1.0)]);
        assert!(layout.is_ok_and(|layout| layout.center('a').is_some()));
    }
}
