#![no_main]

mod common;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    common::replay(data, 80, 24, false);
});
