//! Bootstrap token manager for initial authentication
//!
//! Generates a cryptographically secure token at server startup that can be
//! exchanged for a JWT session token. The token is reusable for the lifetime
//! of the server instance.

use rand::RngCore;
use subtle::ConstantTimeEq;

/// Bootstrap token manager
///
/// Holds a single bootstrap token generated at startup. The token is
/// 32 bytes (64 hex characters) and can be used multiple times while
/// the server is running. A new token is generated on each restart.
#[derive(Debug)]
pub struct BootstrapManager {
    /// The bootstrap token (32 bytes, hex encoded = 64 chars)
    token: String,
}

impl BootstrapManager {
    /// Create a new bootstrap manager with a freshly generated token
    pub fn new() -> Self {
        let token = Self::generate_token();
        tracing::debug!("Bootstrap token generated (length: {} chars)", token.len());
        Self { token }
    }

    /// Get the current bootstrap token
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Validate a token using constant-time comparison
    ///
    /// This prevents timing attacks by ensuring the comparison takes
    /// the same amount of time regardless of how many characters match.
    pub fn validate(&self, token: &str) -> bool {
        // Ensure both tokens are the same length before comparison
        if self.token.len() != token.len() {
            return false;
        }

        self.token.as_bytes().ct_eq(token.as_bytes()).into()
    }

    /// Generate a cryptographically secure random token
    fn generate_token() -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(&bytes)
    }
}

impl Default for BootstrapManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Hex encoding helper (no extra dependency needed)
mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(bytes: &[u8]) -> String {
        let mut result = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            result.push(HEX_CHARS[(byte >> 4) as usize] as char);
            result.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_generation() {
        let manager = BootstrapManager::new();
        let token = manager.token();

        // Token should be 64 hex characters (32 bytes * 2)
        assert_eq!(token.len(), 64);

        // Token should be valid hex
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_token_uniqueness() {
        let manager1 = BootstrapManager::new();
        let manager2 = BootstrapManager::new();

        // Different managers should have different tokens
        assert_ne!(manager1.token(), manager2.token());
    }

    #[test]
    fn test_token_validation() {
        let manager = BootstrapManager::new();
        let token = manager.token().to_string();

        // Correct token should validate
        assert!(manager.validate(&token));

        // Wrong token should not validate
        assert!(!manager.validate("wrong_token"));

        // Empty token should not validate
        assert!(!manager.validate(""));

        // Similar but different token should not validate
        let mut wrong = token.clone();
        wrong.replace_range(0..1, "0");
        if wrong != token {
            assert!(!manager.validate(&wrong));
        }
    }

    #[test]
    fn test_constant_time_comparison() {
        let manager = BootstrapManager::new();

        // Both should take similar time (can't really test timing, but ensure no panics)
        let _ = manager.validate("a".repeat(64).as_str());
        let _ = manager.validate("b".repeat(64).as_str());
        let _ = manager.validate("");
        let _ = manager.validate("short");
    }
}
