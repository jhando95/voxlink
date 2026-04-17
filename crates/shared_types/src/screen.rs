pub const MAX_SCREEN_FRAME_SIZE: usize = 512 * 1024;
/// Per-chunk metadata for oversized screen-share frames:
/// sequence(u32) + chunk_index(u16) + chunk_count(u16).
pub const SCREEN_CHUNK_METADATA_LEN: usize = 8;
/// Chunked screen-share datagrams intentionally stay well below the protocol
/// ceiling so they avoid `EMSGSIZE` and reduce fragmentation pressure.
pub const MAX_UDP_SCREEN_CHUNK_SIZE: usize = 4 * 1024;
pub const MEDIA_PACKET_SCREEN: u8 = 2;
pub const MEDIA_PACKET_SCREEN_CHUNK: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenChunkMetadata {
    pub sequence: u32,
    pub chunk_index: u16,
    pub chunk_count: u16,
}

pub fn encode_screen_chunk_metadata(
    sequence: u32,
    chunk_index: u16,
    chunk_count: u16,
) -> [u8; SCREEN_CHUNK_METADATA_LEN] {
    let mut out = [0u8; SCREEN_CHUNK_METADATA_LEN];
    out[..4].copy_from_slice(&sequence.to_be_bytes());
    out[4..6].copy_from_slice(&chunk_index.to_be_bytes());
    out[6..8].copy_from_slice(&chunk_count.to_be_bytes());
    out
}

pub fn decode_screen_chunk_metadata(raw: &[u8]) -> Option<(ScreenChunkMetadata, &[u8])> {
    if raw.len() < SCREEN_CHUNK_METADATA_LEN {
        return None;
    }
    let sequence = u32::from_be_bytes(raw[..4].try_into().ok()?);
    let chunk_index = u16::from_be_bytes(raw[4..6].try_into().ok()?);
    let chunk_count = u16::from_be_bytes(raw[6..8].try_into().ok()?);
    if chunk_count == 0 || chunk_index >= chunk_count {
        return None;
    }
    Some((
        ScreenChunkMetadata {
            sequence,
            chunk_index,
            chunk_count,
        },
        &raw[SCREEN_CHUNK_METADATA_LEN..],
    ))
}
