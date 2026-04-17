#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_types::decode_screen_chunk_metadata;

fuzz_target!(|data: &[u8]| {
    // Must never panic on arbitrary input.
    let _ = decode_screen_chunk_metadata(data);
});
