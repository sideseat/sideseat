//! Cryptographic utility functions

use anyhow::{Result, bail};
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Generate a cryptographically secure random key
pub fn generate_key(len: usize) -> Vec<u8> {
    let mut key = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Generate a 256-bit (32 byte) signing key
pub fn generate_signing_key() -> Vec<u8> {
    generate_key(32)
}

/// Generate a cryptographically secure random hex token
pub fn generate_token(byte_len: usize) -> String {
    encode_hex(&generate_key(byte_len))
}

/// Constant-time string comparison to prevent timing attacks
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Decode a hex string to bytes
pub fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        bail!("Invalid hex string length");
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16)
            .map_err(|_| anyhow::anyhow!("Invalid hex character"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Encode bytes to a hex string
pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(HEX_CHARS[(byte >> 4) as usize] as char);
        result.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    result
}

/// Calculate SHA256 hash and return as hex string
pub fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key_length() {
        assert_eq!(generate_key(16).len(), 16);
        assert_eq!(generate_key(32).len(), 32);
        assert_eq!(generate_key(64).len(), 64);
    }

    #[test]
    fn test_generate_signing_key() {
        let key = generate_signing_key();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_encode_hex() {
        assert_eq!(encode_hex(&[0x00]), "00");
        assert_eq!(encode_hex(&[0xff]), "ff");
        assert_eq!(
            encode_hex(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]),
            "0123456789abcdef"
        );
        assert_eq!(encode_hex(&[]), "");
    }

    #[test]
    fn test_decode_hex() {
        assert_eq!(decode_hex("00").unwrap(), vec![0x00]);
        assert_eq!(decode_hex("ff").unwrap(), vec![0xff]);
        assert_eq!(decode_hex("FF").unwrap(), vec![0xff]);
        assert_eq!(
            decode_hex("0123456789abcdef").unwrap(),
            vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]
        );
        assert_eq!(decode_hex("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn test_decode_hex_invalid_length() {
        assert!(decode_hex("0").is_err());
        assert!(decode_hex("abc").is_err());
    }

    #[test]
    fn test_decode_hex_invalid_char() {
        assert!(decode_hex("gg").is_err());
        assert!(decode_hex("0z").is_err());
    }

    #[test]
    fn test_roundtrip() {
        let original = vec![0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0];
        let hex = encode_hex(&original);
        let decoded = decode_hex(&hex).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_random() {
        let original = generate_signing_key();
        let hex = encode_hex(&original);
        let decoded = decode_hex(&hex).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_generate_token() {
        let token = generate_token(32);
        assert_eq!(token.len(), 64); // 32 bytes = 64 hex chars
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_token_uniqueness() {
        let t1 = generate_token(32);
        let t2 = generate_token(32);
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq("hello", "hello"));
        assert!(!constant_time_eq("hello", "world"));
        assert!(!constant_time_eq("hello", "hell"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn test_sha256_hex() {
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
