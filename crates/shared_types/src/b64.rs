//! Minimal, dependency-free base64 (RFC 4648, standard alphabet, with padding).
//!
//! Used to carry attachment bytes inside JSON `SignalMessage` control frames.
//! base64 adds ~1.33x overhead versus the ~3.6x of serde_json's default
//! `Vec<u8>`-as-integer-array encoding, so it keeps the attachment path lean
//! without pulling in an external crate.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: u8 = b'=';

/// Encode bytes to a standard base64 string with padding.
pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            PAD as char
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(b2 & 0x3f) as usize] as char
        } else {
            PAD as char
        });
    }
    out
}

/// Decode a standard base64 string (with padding). Returns `None` on any
/// malformed input (invalid character, bad length, or misplaced padding).
pub fn decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    if bytes.is_empty() {
        return Some(Vec::new());
    }

    let mut rev = [255u8; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        rev[c as usize] = i as u8;
    }

    let n_groups = bytes.len() / 4;
    let mut out = Vec::with_capacity(n_groups * 3);
    for (g, chunk) in bytes.chunks_exact(4).enumerate() {
        let is_last = g == n_groups - 1;
        let (c0, c1, c2, c3) = (chunk[0], chunk[1], chunk[2], chunk[3]);

        // Padding is only ever valid in the last group's third/fourth slots.
        if c0 == PAD || c1 == PAD {
            return None;
        }
        let v0 = rev[c0 as usize];
        let v1 = rev[c1 as usize];
        if v0 == 255 || v1 == 255 {
            return None;
        }

        if c2 == PAD {
            if !is_last || c3 != PAD {
                return None;
            }
            out.push((v0 << 2) | (v1 >> 4));
        } else {
            let v2 = rev[c2 as usize];
            if v2 == 255 {
                return None;
            }
            out.push((v0 << 2) | (v1 >> 4));
            out.push((v1 << 4) | (v2 >> 2));
            if c3 != PAD {
                let v3 = rev[c3 as usize];
                if v3 == 255 {
                    return None;
                }
                out.push((v2 << 6) | v3);
            } else if !is_last {
                return None;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 4648 §10 test vectors.
    const VECTORS: &[(&str, &str)] = &[
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];

    #[test]
    fn encode_matches_rfc4648_vectors() {
        for (plain, encoded) in VECTORS {
            assert_eq!(encode(plain.as_bytes()), *encoded, "encode({plain:?})");
        }
    }

    #[test]
    fn decode_matches_rfc4648_vectors() {
        for (plain, encoded) in VECTORS {
            assert_eq!(
                decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "decode({encoded:?})"
            );
        }
    }

    #[test]
    fn roundtrips_all_byte_values() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(decode(&encode(&all)).as_deref(), Some(all.as_slice()));
    }

    #[test]
    fn roundtrips_every_length_up_to_3_blocks() {
        // Exercises all three padding cases across multiple blocks.
        for len in 0..=12usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 7 + 1) as u8).collect();
            let enc = encode(&data);
            assert_eq!(enc.len() % 4, 0, "encoded length must be a multiple of 4");
            assert_eq!(decode(&enc).as_deref(), Some(data.as_slice()), "len {len}");
        }
    }

    #[test]
    fn decode_rejects_invalid_characters() {
        assert_eq!(decode("****"), None);
        assert_eq!(decode("Zg=$"), None);
        assert_eq!(decode("Z g=="), None); // embedded space
    }

    #[test]
    fn decode_rejects_bad_length() {
        assert_eq!(decode("Zg"), None); // length not a multiple of 4
        assert_eq!(decode("Zg=="), Some(b"f".to_vec())); // control: valid
        assert_eq!(decode("Zm9vYg="), None); // 7 chars, not a multiple of 4
    }

    #[test]
    fn decode_rejects_misplaced_padding() {
        assert_eq!(decode("=ooo"), None); // padding in first position
        assert_eq!(decode("Z==="), None); // three pad chars is never valid
    }
}
