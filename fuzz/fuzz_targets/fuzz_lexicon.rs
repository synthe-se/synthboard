#![no_main]

use libfuzzer_sys::fuzz_target;
use synthswipe::{Lexicon, fold};

fuzz_target!(|text: &str| {
    let words: Vec<&str> = text.split_whitespace().collect();
    let lexicon = Lexicon::new(words.iter().map(|&word| (word, 1.0)));
    for word in &words {
        let keys = fold(word);
        assert!(keys.windows(2).all(|pair| pair[0] != pair[1]));
        if !keys.is_empty() {
            assert!(lexicon.contains(word), "lost {word:?}");
        }
    }
});
