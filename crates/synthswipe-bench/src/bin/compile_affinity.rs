//! Compiles word-pair counts into the keyboard's compact context table.
//!
//! Usage: `compile_affinity OUTPUT MIN_COUNT MIN_NATS COUNTS...`. When two
//! languages share a pair, the first file given wins.

use std::error::Error;

use synthswipe::{Affinities, Bigrams};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let output = arguments.next().ok_or("missing OUTPUT")?;
    let min_count: u32 = arguments.next().ok_or("missing MIN_COUNT")?.parse()?;
    let min_nats: f64 = arguments.next().ok_or("missing MIN_NATS")?.parse()?;
    let mut entries = Vec::new();
    for path in arguments {
        Bigrams::parse(&std::fs::read_to_string(path)?).compile_into(min_count, min_nats, &mut entries);
    }
    let bytes = Affinities::to_bytes(entries);
    std::fs::write(output, &bytes)?;
    println!("{} bytes", bytes.len());
    Ok(())
}
