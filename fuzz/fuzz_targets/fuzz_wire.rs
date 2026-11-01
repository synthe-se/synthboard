#![no_main]

use boltffi::__private::wire::decode;
use libfuzzer_sys::fuzz_target;
use synthboard_core::{Engine, Event, Field};

// BoltFFI's decoder is the largest code that sees typed text and is not ours.
// It trusts strings to be UTF-8 without checking. Kotlin's encoder guarantees
// that, so ASCII stands in for it here: lengths and tags stay fully hostile.
fuzz_target!(|data: &[u8]| {
    let text: Vec<u8> = data.iter().map(|byte| byte & 0x7f).collect();
    let _ = decode::<Field>(&text);
    if let Ok(event) = decode::<Event>(&text) {
        Engine::new().step(event);
    }
});
