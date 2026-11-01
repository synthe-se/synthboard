//! Keyboard behaviour. No Android and no clock here: the shell feeds events,
//! each with its timestamp, and applies the commands that come back.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

#[macro_use]
pub mod codec;
mod board;
mod emoji;
mod engine;
mod memory;
mod predict;
mod wire;

pub use board::{Alphabet, LETTERS_LABEL, Layer, POPUP_HEIGHT_DP};
pub use emoji::{Catalog, EmojiGroup, EmojiItem, SKIN_TONES};
pub use engine::{BAR_SLOTS, Decoding, Engine, Event, FIRST_LANGUAGE, Field, Language, SEARCH_CELLS, SECOND_LANGUAGE, SLOT_CENTERS, Step};
pub use memory::{Memory, contexts};
pub use predict::NextWords;
pub use wire::{Command, DrawnKey, Haptic, Icon, ShiftLook};
