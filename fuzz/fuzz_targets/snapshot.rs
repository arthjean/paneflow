#![no_main]

mod common;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let cols = usize::from(data.first().copied().unwrap_or(79) % 160) + 1;
    let rows = usize::from(data.get(1).copied().unwrap_or(23) % 80) + 1;
    common::differential_replay(data.get(2..).unwrap_or_default(), cols, rows, true);
});
