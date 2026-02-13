//! API key generation and hashing utilities
//!
//! This module provides functions for generating and hashing API keys using
//! HMAC-SHA256 with a server secret for defense-in-depth security.

use hmac::{Hmac, Mac};
use rand::Rng;
use rand::rngs::OsRng;
use sha2::Sha256;

use crate::core::constants::{API_KEY_PREFIX, API_KEY_PREFIX_DISPLAY_LEN, API_KEY_RANDOM_LENGTH};

type HmacSha256 = Hmac<Sha256>;

const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Generate opaque API key: pk-ss-{random_50chars}
/// Uses OsRng (CSPRNG) for cryptographic security
pub fn generate_api_key() -> String {
    let random: String = (0..API_KEY_RANDOM_LENGTH)
        .map(|_| CHARSET[OsRng.gen_range(0..CHARSET.len())] as char)
        .collect();
    format!("{}{}", API_KEY_PREFIX, random)
}

/// HMAC-SHA256 hash of key with server secret (hex encoded)
/// Server secret prevents verification even if DB leaks
pub fn hash_api_key(key: &str, server_secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(server_secret).expect("HMAC accepts any key length");
    mac.update(key.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Extract prefix for display (first 12 chars, e.g., "pk-ss-a1b2c3")
pub fn key_prefix(key: &str) -> String {
    key.chars().take(API_KEY_PREFIX_DISPLAY_LEN).collect()
}

/// Validate key format: pk-ss-{50 alphanumeric chars}
pub fn is_valid_api_key(key: &str) -> bool {
    key.starts_with(API_KEY_PREFIX)
        && key.len() == API_KEY_PREFIX.len() + API_KEY_RANDOM_LENGTH
        && key[API_KEY_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Extract key from auth header (Basic or Bearer)
/// Handles OTEL SDK formats: key:, :key, key:anything, and plain key
pub fn extract_key_from_header(header: &str) -> Option<String> {
    if let Some(encoded) = header.strip_prefix("Basic ") {
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())?;

        // Handle username:password format (OTEL SDK sends key as username or password)
        if let Some((username, password)) = decoded.split_once(':') {
            // Prefer non-empty username, fallback to password
            let key = if !username.is_empty() {
                username
            } else {
                password
            };
            Some(key.to_string())
        } else {
            // No colon - treat whole string as key
            Some(decoded)
        }
    } else {
        header
            .strip_prefix("Bearer ")
            .map(|key| key.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key() {
        let key = generate_api_key();
        assert!(key.starts_with(API_KEY_PREFIX));
        assert_eq!(key.len(), API_KEY_PREFIX.len() + API_KEY_RANDOM_LENGTH);
        assert!(is_valid_api_key(&key));
    }

    #[test]
    fn test_generate_api_key_uniqueness() {
        let key1 = generate_api_key();
        let key2 = generate_api_key();
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_hash_api_key() {
        let key = "pk-ss-a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5";
        let secret = b"test-secret-32-bytes-long-here!";

        let hash1 = hash_api_key(key, secret);
        let hash2 = hash_api_key(key, secret);

        // Same key + secret = same hash
        assert_eq!(hash1, hash2);

        // Hex encoded (64 chars for SHA256)
        assert_eq!(hash1.len(), 64);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));

        // Different secret = different hash
        let hash3 = hash_api_key(key, b"different-secret-here!!!!!!!!!");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_key_prefix() {
        let key = "pk-ss-a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5";
        assert_eq!(key_prefix(key), "pk-ss-a1b2c3");
    }

    #[test]
    fn test_is_valid_api_key() {
        // Valid key
        assert!(is_valid_api_key(
            "pk-ss-a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5"
        ));

        // Too short
        assert!(!is_valid_api_key("pk-ss-a1b2c3"));

        // Wrong prefix
        assert!(!is_valid_api_key(
            "xx-ss-a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4y5"
        ));

        // Invalid characters (uppercase)
        assert!(!is_valid_api_key(
            "pk-ss-A1B2C3D4E5F6G7H8I9J0K1L2M3N4O5P6Q7R8S9T0U1V2W3X4Y5"
        ));

        // Invalid characters (special)
        assert!(!is_valid_api_key(
            "pk-ss-a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0u1v2w3x4-5"
        ));
    }

    #[test]
    fn test_extract_key_from_header_bearer() {
        let key = extract_key_from_header("Bearer pk-ss-abc123");
        assert_eq!(key, Some("pk-ss-abc123".to_string()));
    }

    #[test]
    fn test_extract_key_from_header_basic_username() {
        use base64::Engine;
        // Key as username (OTEL SDK format)
        let encoded = base64::engine::general_purpose::STANDARD.encode("pk-ss-abc123:");
        let key = extract_key_from_header(&format!("Basic {}", encoded));
        assert_eq!(key, Some("pk-ss-abc123".to_string()));
    }

    #[test]
    fn test_extract_key_from_header_basic_password() {
        use base64::Engine;
        // Key as password
        let encoded = base64::engine::general_purpose::STANDARD.encode(":pk-ss-abc123");
        let key = extract_key_from_header(&format!("Basic {}", encoded));
        assert_eq!(key, Some("pk-ss-abc123".to_string()));
    }

    #[test]
    fn test_extract_key_from_header_basic_plain() {
        use base64::Engine;
        // Plain key (no colon)
        let encoded = base64::engine::general_purpose::STANDARD.encode("pk-ss-abc123");
        let key = extract_key_from_header(&format!("Basic {}", encoded));
        assert_eq!(key, Some("pk-ss-abc123".to_string()));
    }

    #[test]
    fn test_extract_key_from_header_invalid() {
        assert!(extract_key_from_header("InvalidHeader pk-ss-abc").is_none());
        assert!(extract_key_from_header("").is_none());
    }
}
