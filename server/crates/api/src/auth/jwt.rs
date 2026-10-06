//! JWT session token handling

use chrono::Duration;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use sideseat_core::constants::DEFAULT_SESSION_TTL_DAYS;
use sideseat_ports::clock::Clock;

/// Why a session token could not be issued or accepted.
///
/// `Expired` is kept apart from the rest because the middleware answers it differently: an expired
/// session tells the browser to sign in again, where an invalid one is a rejection.
#[derive(Debug, Error)]
pub enum JwtError {
    /// Token signature has expired
    #[error("Session token has expired")]
    Expired,
    /// Token signature is invalid
    #[error("Invalid session token signature")]
    InvalidSignature,
    /// Other validation error
    #[error("Invalid session token: {0}")]
    Invalid(String),
    /// Signing failed, which is a fault in this process rather than in the token.
    #[error("Failed to create JWT: {source}")]
    Encode {
        #[source]
        source: jsonwebtoken::errors::Error,
    },
}

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
    pub fn new(user_id: &str, auth_method: &str, clock: &dyn Clock) -> Self {
        let now = clock.now();
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
    clock: &dyn Clock,
) -> Result<String, JwtError> {
    let claims = SessionClaims::new(user_id, auth_method, clock);
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(signing_key),
    )
    .map_err(|source| JwtError::Encode { source })
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
    use chrono::{DateTime, TimeZone, Utc};

    #[derive(Debug)]
    struct SystemClock;

    impl Clock for SystemClock {
        fn now(&self) -> DateTime<Utc> {
            std::time::SystemTime::now().into()
        }
    }

    #[derive(Debug)]
    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn test_key() -> Vec<u8> {
        vec![0u8; 32]
    }

    #[test]
    fn test_create_and_validate() {
        let key = test_key();
        let token = create_session_token(&key, "local", "bootstrap", &SystemClock).unwrap();
        let claims = validate_session_token(&token, &key).unwrap();
        assert_eq!(claims.sub, "local");
        assert_eq!(claims.user_id(), "local");
        assert_eq!(claims.auth_method, "bootstrap");
    }

    #[test]
    fn test_create_with_custom_user() {
        let key = test_key();
        let token = create_session_token(&key, "user123", "oauth", &SystemClock).unwrap();
        let claims = validate_session_token(&token, &key).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.user_id(), "user123");
        assert_eq!(claims.auth_method, "oauth");
    }

    #[test]
    fn test_invalid_signature() {
        let key1 = vec![0u8; 32];
        let key2 = vec![1u8; 32];
        let token = create_session_token(&key1, "local", "bootstrap", &SystemClock).unwrap();
        assert!(validate_session_token(&token, &key2).is_err());
    }

    #[test]
    fn test_unique_jti() {
        let clock = SystemClock;
        let c1 = SessionClaims::new("local", "bootstrap", &clock);
        let c2 = SessionClaims::new("local", "bootstrap", &clock);
        assert_ne!(c1.jti, c2.jti);
    }

    #[test]
    fn claims_use_the_injected_clock() {
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let claims = SessionClaims::new("local", "bootstrap", &FixedClock(now));

        assert_eq!(claims.iat, 1_700_000_000);
        assert_eq!(
            claims.exp,
            1_700_000_000 + Duration::days(DEFAULT_SESSION_TTL_DAYS as i64).num_seconds()
        );
    }
}
