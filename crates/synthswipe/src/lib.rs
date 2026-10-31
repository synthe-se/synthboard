//! Offline swipe decoder.
//!
//! This crate knows nothing about Android or any host. It takes a trajectory,
//! a layout supplied at runtime and a lexicon, and returns ranked candidates.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

mod context;
mod geometry;
mod grammar;
mod layout;
mod lexicon;
mod marks;
mod shark2;
mod taps;
#[cfg(test)]
mod testkit;

pub use context::{Affinities, Bigrams, SENTENCE_START, spoken_weight, tokens};
pub use geometry::{Sample, signature, signature_distance};
pub use grammar::Grammar;
pub use layout::{Key, Layout, LayoutError};
pub use lexicon::{Entry, Lexicon, fold};
pub use taps::{Correction, correct};
pub use shark2::{Candidate, DecodeError, Params, decode, decode_in_context};
