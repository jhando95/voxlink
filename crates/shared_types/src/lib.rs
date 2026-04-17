pub mod view;
pub use view::*;

pub mod state;
pub use state::*;

pub mod message_data;
pub use message_data::*;

pub mod protocol;
pub use protocol::*;

pub mod screen;
pub use screen::*;

pub mod helpers;
pub use helpers::*;

/// Maximum audio frame size in bytes (Opus at 24kbps, 20ms = ~60 bytes typical, 256 max)
pub const MAX_AUDIO_FRAME_SIZE: usize = 4096;
/// Safe media payload budget for a single UDP datagram.
/// Kept below the protocol maximum to leave room for token and sender headers.
pub const MAX_UDP_MEDIA_PAYLOAD_SIZE: usize = 60 * 1024;
pub const MEDIA_PACKET_AUDIO: u8 = 1;

/// UDP session token length in bytes (random, assigned by server on RequestUdp).
pub const UDP_SESSION_TOKEN_LEN: usize = 8;
/// Default UDP relay port (same as WebSocket port + 1).
pub const UDP_DEFAULT_PORT_OFFSET: u16 = 1;
/// UDP keepalive packet type — sent every 15s to keep NAT mappings alive.
pub const UDP_KEEPALIVE: u8 = 0xFE;
/// Interval between UDP keepalive packets.
pub const UDP_KEEPALIVE_INTERVAL_SECS: u64 = 15;

pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SIZE: usize = 960; // 20ms at 48kHz

#[cfg(test)]
mod tests;
