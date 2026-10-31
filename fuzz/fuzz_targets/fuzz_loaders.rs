#![no_main]

use libfuzzer_sys::fuzz_target;
use synthboard_core::{Catalog, NextWords};
use synthswipe::{Affinities, Grammar, tokens};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let table = Affinities::from_bytes(data);
    let words = tokens(&text);
    let score = table.score("je", &text);
    assert!(score.is_finite());
    let grammar = Grammar::parse(&text);
    let last = words.last().map_or("", String::as_str);
    assert!(grammar.score(&words, last).abs() <= 1.0);
    let catalog = Catalog::parse(&text);
    assert!(catalog.search(&text).len() <= 32);
    let next = NextWords::parse(&text);
    assert!(next.after(last).len() <= words.len() + text.split(' ').count());
    let _ = Affinities::to_bytes(data.chunks_exact(5).map(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]), c[4] as i8)).collect());
});
