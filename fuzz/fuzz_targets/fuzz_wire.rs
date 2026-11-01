#![no_main]

use libfuzzer_sys::fuzz_target;
use synthboard_core::codec::{decode, encode};
use synthboard_ffi::{Call, Keyboard, handle};

// Everything the shell can send, hostile bytes included, lands here.
fuzz_target!(|data: &[u8]| {
    if let Some(call) = decode::<Call>(data) {
        assert!(decode::<Call>(&encode(&call)).is_some());
    }
    handle(&Keyboard::new(), data, &mut Vec::new());
});
