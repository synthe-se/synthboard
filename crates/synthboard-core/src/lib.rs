//! Keyboard behaviour. No Android and no clock here: the shell feeds events,
//! each with its timestamp, and applies the commands that come back.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

#[macro_use]
pub mod codec;
mod board;
mod emoji;
mod engine;
mod memory;
mod predict;
mod wire;

pub use board::{Alphabet, Layer};
pub use emoji::{Catalog, EmojiGroup, EmojiItem};
pub use engine::{Decoding, Engine, Event, Field, Step};
pub use predict::NextWords;
pub use wire::{Command, DrawnKey, Haptic, Icon};
