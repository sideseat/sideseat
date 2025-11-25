//! JWT session token creation and validation
//!
//! Handles JWT encoding/decoding for session management with 30-day expiry.

use crate::core::constants::DEFAULT_SESSION_TTL_DAYS;
use crate::error::{Error, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// JWT claims structure for session tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Subject identifier (e.g., "local" for bootstrap auth)
    pub sub: String,
    /// Issued at timestamp (Unix epoch seconds)
    pub iat: i64,
    /// Expiration timestamp (Unix epoch seconds)
    pub exp: i64,
    /// Unique JWT ID for tracking
    pub jti: String,
    /// Authentication method used (e.g., "bootstrap")
    pub auth_method: String,
}

impl SessionClaims {
    /// Create new session claims with default TTL
    pub fn new(auth_method: &str) -> Self {
        let now = Utc::now();
        let exp = now + Duration::days(DEFAULT_SESSION_TTL_DAYS as i64);

        Self {
            sub: "local".to_string(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
            jti: Uuid::new_v4().to_string(),
            auth_method: auth_method.to_string(),
        }
    }

    /// Check if the token has expired
    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp() > self.exp
    }
}

/// Create a signed JWT session token
///
/// # Arguments
/// * `signing_key` - 256-bit key for HS256 signing
/// * `auth_method` - Authentication method identifier (e.g., "bootstrap")
///
/// # Returns
/// Encoded JWT string
pub fn create_session_token(signing_key: &[u8], auth_method: &str) -> Result<String> {
    let claims = SessionClaims::new(auth_method);

    encode(&Header::default(), &claims, &EncodingKey::from_secret(signing_key))
        .map_err(|e| Error::Auth(format!("Failed to create JWT: {}", e)))
}

/// Validate and decode a JWT session token
///
/// # Arguments
/// * `token` - The JWT string to validate
/// * `signing_key` - 256-bit key for HS256 verification
///
/// # Returns
/// Decoded claims if valid, error if invalid or expired
pub fn validate_session_token(token: &str, signing_key: &[u8]) -> Result<SessionClaims> {
    let token_data = decode::<SessionClaims>(
        token,
        &DecodingKey::from_secret(signing_key),
        &Validation::default(),
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
            Error::Auth("Session token has expired".to_string())
        }
        jsonwebtoken::errors::ErrorKind::InvalidSignature => {
            Error::Auth("Invalid session token signature".to_string())
        }
        _ => Error::Auth(format!("Invalid session token: {}", e)),
    })?;

    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> Vec<u8> {
        vec![0u8; 32] // Test key
    }

    #[test]
    fn test_create_and_validate_token() {
        let key = test_key();
        let token = create_session_token(&key, "bootstrap").unwrap();

        let claims = validate_session_token(&token, &key).unwrap();
        assert_eq!(claims.sub, "local");
        assert_eq!(claims.auth_method, "bootstrap");
        assert!(!claims.is_expired());
    }

    #[test]
    fn test_invalid_signature() {
        let key1 = vec![0u8; 32];
        let key2 = vec![1u8; 32];

        let token = create_session_token(&key1, "bootstrap").unwrap();
        let result = validate_session_token(&token, &key2);

        assert!(result.is_err());
    }

    #[test]
    fn test_claims_have_unique_jti() {
        let claims1 = SessionClaims::new("bootstrap");
        let claims2 = SessionClaims::new("bootstrap");

        assert_ne!(claims1.jti, claims2.jti);
    }
}
