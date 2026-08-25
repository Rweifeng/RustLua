//! Content fingerprinting shared by the import cache and resource database.

/// FNV-1a-derived 128-bit content fingerprint rendered as a 32-hex string.
///
/// Not cryptographic; it only needs to change when the bytes change so the
/// import cache can avoid re-importing identical content.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    let mut a: u64 = 0xcbf29ce484222325;
    let mut b: u64 = 0x9e3779b185ebca87;
    for (i, byte) in bytes.iter().enumerate() {
        a = (a ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        b = (b ^ (u64::from(*byte) + i as u64))
            .rotate_left(7)
            .wrapping_mul(0x9e3779b185ebca87);
    }
    format!("{a:016x}{b:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_bytes_produce_same_hash() {
        assert_eq!(content_hash(b"hello"), content_hash(b"hello"));
        assert_eq!(content_hash(b""), content_hash(b""));
    }

    #[test]
    fn different_bytes_produce_different_hash() {
        assert_ne!(content_hash(b"hello"), content_hash(b"hallo"));
        assert_ne!(content_hash(b""), content_hash(b"x"));
    }

    #[test]
    fn hash_is_stable_hex_string() {
        let hash = content_hash(b"stable input");
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
