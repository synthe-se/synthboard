#![no_main]

use libfuzzer_sys::fuzz_target;
use synthboard_core::{Memory, contexts};

// Saved memory is opened from a file, which may be damaged in any way.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let known = |word: &str| word.len() % 2 == 0;
    let mut memory = Memory::parse(&text, known);
    let saved = memory.to_text();
    let again = Memory::parse(&saved, known);
    assert_eq!(again.next_after("<s>").len(), memory.next_after("<s>").len());
    let history: Vec<String> = text.split_whitespace().take(4).map(str::to_lowercase).collect();
    let previous = history.last().map_or("<s>", String::as_str).to_owned();
    let word = history.first().cloned().unwrap_or_default();
    assert!(memory.habit_after(&contexts(&history), &previous, &word).is_finite());
    let _ = memory.phrases(&history, &previous);
    let _ = memory.trace_matches(&[[0.0, 0.0], [1.0, 1.0]]);
    memory.learn_after(&history, &previous, &word, known);
    memory.unlearn(&history, &previous, &word, known);
    memory.keep(&word, known);
    memory.forget(&word, known);
    assert!(memory.is_blocked(&word));
});
