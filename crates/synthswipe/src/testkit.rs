//! What the tests of this crate share.

use crate::layout::{Key, Layout};

/// An AZERTY board with keys one unit apart.
pub(crate) fn grid() -> Option<Layout> {
    let rows = ["azertyuiop", "qsdfghjklm", "wxcvbn"];
    let keys = rows.iter().zip([0.0_f32, 1.0, 2.0]).flat_map(|(row, y)| {
        row.chars()
            .zip((0_u8..).map(f32::from))
            .map(move |(label, x)| Key { label, x, y })
    });
    Layout::new(keys).ok()
}
