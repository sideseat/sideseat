//! JWT session token handling

use std::fmt;

use anyhow::{Result, anyhow};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::constants::DEFAULT_SESSION_TTL_DAYS;

/// JWT validation error
#[derive(Debug)]
pub enum JwtError {
    /// Token signature has expired
    Expired,
    /// Token signature is invalid
    InvalidSignature,
    /// Other validation error
    Invalid(String),
}

impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expired => write!(f, "Session token has expired"),
            Self::InvalidSignature => write!(f, "Invalid session token signature"),
            Self::Invalid(msg) => write!(f, "Invalid session token: {}", msg),
        }
    }
}

impl std::error::Error for JwtError {}

/// JWT claims for session tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// User ID (identity only, no org context)
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
    pub auth_method: String,
}

impl SessionClaims {
    pub fn new(user_id: &str, auth_method: &str) -> Self {
        let now = Utc::now();
        let exp = now + Duration::days(DEFAULT_SESSION_TTL_DAYS as i64);

        Self {
            sub: user_id.to_string(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
            jti: Uuid::new_v4().to_string(),
            auth_method: auth_method.to_string(),
        }
    }

    /// Get the user ID from claims
    pub fn user_id(&self) -> &str {
        &self.sub
    }
}

/// Create a signed JWT session token
pub fn create_session_token(
    signing_key: &[u8],
    user_id: &str,
    auth_method: &str,
) -> Result<String> {
    let claims = SessionClaims::new(user_id, auth_method);
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(signing_key),
    )
    .map_err(|e| anyhow!("Failed to create JWT: {}", e))
}

/// Validate and decode a JWT session token
pub fn validate_session_token(token: &str, signing_key: &[u8]) -> Result<SessionClaims, JwtError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    let token_data =
        decode::<SessionClaims>(token, &DecodingKey::from_secret(signing_key), &validation)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
                jsonwebtoken::errors::ErrorKind::InvalidSignature => JwtError::InvalidSignature,
                _ => JwtError::Invalid(e.to_string()),
            })?;

    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> Vec<u8> {
        vec![0u8; 32]
    }

    #[test]
    fn test_create_and_validate() {
        let key = test_key();
        let token = create_session_token(&key, "local", "bootstrap").unwrap();
        let claims = validate_session_token(&token, &key).unwrap();
        assert_eq!(claims.sub, "local");
        assert_eq!(claims.user_id(), "local");
        assert_eq!(claims.auth_method, "bootstrap");
    }

    #[test]
    fn test_create_with_custom_user() {
        let key = test_key();
        let token = create_session_token(&key, "user123", "oauth").unwrap();
        let claims = validate_session_token(&token, &key).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.user_id(), "user123");
        assert_eq!(claims.auth_method, "oauth");
    }

    #[test]
    fn test_invalid_signature() {
        let key1 = vec![0u8; 32];
        let key2 = vec![1u8; 32];
        let token = create_session_token(&key1, "local", "bootstrap").unwrap();
        assert!(validate_session_token(&token, &key2).is_err());
    }

    #[test]
    fn test_unique_jti() {
        let c1 = SessionClaims::new("local", "bootstrap");
        let c2 = SessionClaims::new("local", "bootstrap");
        assert_ne!(c1.jti, c2.jti);
    }
}
