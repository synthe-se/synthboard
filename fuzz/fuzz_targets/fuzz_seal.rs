#![no_main]

use libfuzzer_sys::fuzz_target;
use synthboard_seal::{from_hex, generate_keys, open, seal, to_hex};

fuzz_target!(|data: &[u8]| {
    assert_eq!(from_hex(&to_hex(data)).as_deref(), Some(data));
    let _ = open(&[7; 32], data);
    let _ = from_hex(&String::from_utf8_lossy(data));
    if let Ok((secret, public)) = generate_keys() {
        assert_eq!(seal(&public, data).ok().and_then(|record| open(&secret, &record).ok()).as_deref(), Some(data));
    }
});
