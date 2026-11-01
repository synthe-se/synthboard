#![no_main]

use libfuzzer_sys::fuzz_target;
use synthswipe_bench::traces::{Fate, blocks, is_sound, layout_of, merge, next_word_pairs, parse};

// Anyone with the public key can forge a record, so a trace is hostile input.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let lines = text.lines().count();
    assert!(parse(&text).len() <= lines);
    for gesture in parse(&text) {
        assert!(gesture.truth.is_some() || matches!(gesture.fate, Fate::Deleted | Fate::Rephrased));
        let _ = layout_of(&gesture.keys);
    }
    let gestures = parse(&text);
    assert!(next_word_pairs(&gestures).count() < gestures.len().max(1));
    let block_lines: usize = blocks(&text).iter().map(Vec::len).sum();
    assert_eq!(block_lines, lines);
    let (merged, _) = merge(&text, &[text.to_string()]);
    assert!(merged.lines().all(is_sound));
    assert_eq!(merge(&merged, std::slice::from_ref(&merged)).1, 0);
});
