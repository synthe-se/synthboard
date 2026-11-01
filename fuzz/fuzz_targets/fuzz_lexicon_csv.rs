#![no_main]

use libfuzzer_sys::fuzz_target;
use synthswipe::Lexicon;

// The word list is an asset, and loaders accept any bytes.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let lexicon = Lexicon::from_csv(&text);
    for word in text.split([',', '\n']).take(32) {
        let found = lexicon.contains(word);
        assert_eq!(found, lexicon.surface(word).is_some());
        let _ = lexicon.languages(word);
        assert!(lexicon.weight(word).is_finite());
    }
});
