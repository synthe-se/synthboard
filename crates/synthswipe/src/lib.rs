//! Offline swipe decoder.
//!
//! This crate knows nothing about Android or any host. It takes a trajectory,
//! a layout supplied at runtime and a lexicon, and returns ranked candidates.

#![forbid(unsafe_code)]

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

mod context;
mod geometry;
mod grammar;
mod layout;
mod lexicon;
mod marks;
mod neural;
mod shark2;
mod taps;
#[cfg(test)]
mod testkit;

pub use context::{Affinities, Bigrams, SENTENCE_START, spoken_weight, tokens};
pub use geometry::{Sample, signature, signature_distance};
pub use grammar::Grammar;
pub use layout::{Key, Layout, LayoutError, is_key_label};
pub use lexicon::{Entry, Lexicon, fold};
pub use taps::{Correction, correct};
pub use neural::{Encoder, FEATURES, FRAMES, INPUT_VALUES, FREQUENCY_WEIGHT, KeyHead, MODEL_HEAD_BYTES, MODEL_MAGIC, Mix, NETWORK_WEIGHT, Neural, OUTPUT_VALUES, POOL, Reading, floor_of, lean, split_model};
pub use shark2::{Candidate, DecodeError, Params, best_first, better, decode, decode_in_context, fits_gesture};
