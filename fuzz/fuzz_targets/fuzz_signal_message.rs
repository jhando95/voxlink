#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_types::SignalMessage;

fuzz_target!(|data: &[u8]| {
    // The parser should never panic on arbitrary input, only return Err.
    let _ = serde_json::from_slice::<SignalMessage>(data);
});
