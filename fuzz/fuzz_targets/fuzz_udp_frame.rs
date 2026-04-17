#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_types::{MEDIA_PACKET_AUDIO, UDP_KEEPALIVE, UDP_SESSION_TOKEN_LEN};

fuzz_target!(|data: &[u8]| {
    // Model the input-validation prefix of the server's UDP receive loop:
    // require a minimum size, slice the session token, inspect the
    // packet-type byte. Any arithmetic on arbitrary bytes must never panic.
    if data.len() < UDP_SESSION_TOKEN_LEN + 1 {
        return;
    }
    let _token = &data[..UDP_SESSION_TOKEN_LEN];
    let packet_type = data[UDP_SESSION_TOKEN_LEN];
    let _payload = &data[UDP_SESSION_TOKEN_LEN + 1..];
    match packet_type {
        UDP_KEEPALIVE | MEDIA_PACKET_AUDIO => {},
        _ => {},
    }
});
