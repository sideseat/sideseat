//! Authentication manager
//!
//! Orchestrates the authentication flow, managing bootstrap tokens,
//! JWT signing keys, and session validation.

use super::bootstrap::BootstrapManager;
use super::jwt::{SessionClaims, create_session_token, validate_session_token};
use crate::core::SecretManager;
use crate::core::constants::SECRET_KEY_JWT_SIGNING;
use crate::error::{Error, Result};
use rand::RngCore;

/// Main authentication manager
///
/// Handles all authentication operations including:
/// - Bootstrap token generation and validation
/// - JWT session token creation and validation
/// - Signing key management via SecretManager (OS keychain)
#[derive(Debug)]
pub struct AuthManager {
    /// JWT signing key (256-bit)
    signing_key: Vec<u8>,
    /// Bootstrap token manager
    bootstrap: BootstrapManager,
    /// Whether authentication is enabled
    enabled: bool,
}

impl AuthManager {
    /// Initialize the authentication manager
    ///
    /// Loads or creates the JWT signing key from SecretManager and
    /// generates a new bootstrap token for this server session.
    pub async fn init(secrets: &SecretManager, enabled: bool) -> Result<Self> {
        let signing_key = Self::load_or_create_signing_key(secrets).await?;
        let bootstrap = BootstrapManager::new();

        if enabled {
            tracing::debug!("Authentication enabled");
        } else {
            tracing::warn!("Authentication DISABLED - all requests will be allowed");
        }

        Ok(Self { signing_key, bootstrap, enabled })
    }

    /// Check if authentication is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the current bootstrap token
    ///
    /// This token is printed to the terminal and can be used to
    /// authenticate via the web UI.
    pub fn bootstrap_token(&self) -> &str {
        self.bootstrap.token()
    }

    /// Exchange a bootstrap token for a JWT session token
    ///
    /// # Arguments
    /// * `bootstrap_token` - The token from the terminal URL
    ///
    /// # Returns
    /// A JWT session token if the bootstrap token is valid, or if auth is disabled
    pub fn exchange_token(&self, bootstrap_token: &str) -> Result<String> {
        // If auth is disabled, accept any token
        if !self.enabled {
            return create_session_token(&self.signing_key, "disabled");
        }

        if !self.bootstrap.validate(bootstrap_token) {
            return Err(Error::Auth("Invalid bootstrap token".to_string()));
        }

        create_session_token(&self.signing_key, "bootstrap")
    }

    /// Validate a JWT session token
    ///
    /// # Arguments
    /// * `jwt` - The JWT session token from the cookie
    ///
    /// # Returns
    /// The decoded claims if valid
    pub fn validate_session(&self, jwt: &str) -> Result<SessionClaims> {
        validate_session_token(jwt, &self.signing_key)
    }

    /// Load or create the JWT signing key
    ///
    /// Attempts to load an existing key from SecretManager (OS keychain).
    /// If not found, generates a new 256-bit key and stores it.
    /// Note: On first run, the keychain will prompt once to store the new key.
    /// Click "Always Allow" to avoid future prompts.
    async fn load_or_create_signing_key(secrets: &SecretManager) -> Result<Vec<u8>> {
        // Try to load existing key - this won't prompt if key doesn't exist (returns NoEntry)
        match secrets.get_value(SECRET_KEY_JWT_SIGNING).await {
            Ok(Some(key_hex)) => {
                if let Ok(key) = Self::decode_hex(&key_hex)
                    && key.len() == 32
                {
                    tracing::debug!("Loaded existing JWT signing key from secret storage");
                    return Ok(key);
                }
                tracing::warn!("Stored JWT signing key has invalid format, generating new key");
            }
            Ok(None) => {
                tracing::debug!("No existing JWT signing key found, generating new key");
            }
            Err(e) => {
                tracing::warn!("Failed to read JWT signing key: {}, generating new key", e);
            }
        }

        // Generate new key
        let key = Self::generate_signing_key();
        let key_hex = Self::encode_hex(&key);

        // Store the new key (this will prompt on macOS - click "Always Allow")
        secrets.set_api_key(SECRET_KEY_JWT_SIGNING, &key_hex, Some("sideseat")).await?;

        tracing::debug!("Generated and stored new JWT signing key in secret storage");
        Ok(key)
    }

    /// Generate a new 256-bit signing key
    fn generate_signing_key() -> Vec<u8> {
        let mut key = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        key
    }

    /// Decode a hex string to bytes
    fn decode_hex(hex: &str) -> Result<Vec<u8>> {
        if !hex.len().is_multiple_of(2) {
            return Err(Error::Auth("Invalid hex string length".to_string()));
        }

        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for i in (0..hex.len()).step_by(2) {
            let byte = u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| Error::Auth("Invalid hex character".to_string()))?;
            bytes.push(byte);
        }
        Ok(bytes)
    }

    /// Encode bytes to a hex string
    fn encode_hex(bytes: &[u8]) -> String {
        const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
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
    fn test_hex_roundtrip() {
        let original = vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let hex = AuthManager::encode_hex(&original);
        let decoded = AuthManager::decode_hex(&hex).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_signing_key_generation() {
        let key = AuthManager::generate_signing_key();
        assert_eq!(key.len(), 32);
    }
}
