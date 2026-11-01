//! Prints what the shell tools share with the code, as `eval`-able assignments,
//! so that each number lives in one place.

use synthboard_core::{FIRST_LANGUAGE, SECOND_LANGUAGE};
use synthswipe::{MODEL_HEAD_BYTES, MODEL_MAGIC};

fn main() {
    println!("first_language={FIRST_LANGUAGE}");
    println!("second_language={SECOND_LANGUAGE}");
    println!("model_head_bytes={MODEL_HEAD_BYTES}");
    println!("model_magic={}", String::from_utf8_lossy(MODEL_MAGIC));
}
