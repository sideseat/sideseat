//! Authentication manager

use std::sync::Arc;

use thiserror::Error;

use super::jwt::{JwtError, SessionClaims, create_session_token, validate_session_token};
use sideseat_core::constants::DEFAULT_USER_ID;
use sideseat_core::utils::crypto;
use sideseat_ports::clock::Clock;

/// Why a bootstrap token could not be exchanged for a session.
///
/// The two cases differ in whose fault they are: a wrong token is the caller's, and a signing failure
/// is this process's. They were one `anyhow::Error` before, so the route could only answer 401 to both.
#[derive(Debug, Error)]
pub enum TokenExchangeError {
    #[error("Invalid bootstrap token")]
    InvalidBootstrapToken,
    #[error(transparent)]
    Jwt(#[from] JwtError),
}

/// Main authentication manager
#[derive(Debug)]
pub struct AuthManager {
    signing_key: Vec<u8>,
    bootstrap_token: String,
    enabled: bool,
    clock: Arc<dyn Clock>,
}

impl AuthManager {
    /// Initialize the authentication manager
    pub fn new(signing_key: Vec<u8>, enabled: bool, clock: Arc<dyn Clock>) -> Self {
        let bootstrap_token = crypto::generate_token(32);

        if enabled {
            tracing::debug!("Authentication enabled");
        } else {
            tracing::warn!("Authentication DISABLED");
        }

        tracing::debug!("Bootstrap token generated");
        Self {
            signing_key,
            bootstrap_token,
            enabled,
            clock,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn bootstrap_token(&self) -> &str {
        &self.bootstrap_token
    }

    /// Exchange bootstrap token for JWT session token
    /// Bootstrap auth always creates a session for the default local user
    pub fn exchange_token(&self, token: &str) -> Result<String, TokenExchangeError> {
        if !self.enabled {
            return Ok(create_session_token(
                &self.signing_key,
                DEFAULT_USER_ID,
                "disabled",
                self.clock.as_ref(),
            )?);
        }

        if !crypto::constant_time_eq(&self.bootstrap_token, token) {
            return Err(TokenExchangeError::InvalidBootstrapToken);
        }

        Ok(create_session_token(
            &self.signing_key,
            DEFAULT_USER_ID,
            "bootstrap",
            self.clock.as_ref(),
        )?)
    }

    /// Validate a JWT session token
    pub fn validate_session(&self, jwt: &str) -> Result<SessionClaims, JwtError> {
        validate_session_token(jwt, &self.signing_key)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;

    /// Not a fixed instant: `jsonwebtoken` validates `exp` against the real clock, so a token minted
    /// at a pinned past timestamp is expired before it is read back.
    #[derive(Debug)]
    struct SystemClock;

    impl Clock for SystemClock {
        fn now(&self) -> DateTime<Utc> {
            std::time::SystemTime::now().into()
        }
    }

    fn manager(enabled: bool) -> AuthManager {
        AuthManager::new(vec![0u8; 32], enabled, Arc::new(SystemClock))
    }

    /// The wording the `anyhow::bail!` produced, which the route's 401 body repeats.
    #[test]
    fn a_wrong_bootstrap_token_is_refused_by_its_own_variant() {
        let auth = manager(true);
        let error = auth.exchange_token("not-the-token").unwrap_err();

        assert!(matches!(error, TokenExchangeError::InvalidBootstrapToken));
        assert_eq!(error.to_string(), "Invalid bootstrap token");
    }

    #[test]
    fn the_real_bootstrap_token_is_exchanged() {
        let auth = manager(true);
        let token = auth.bootstrap_token().to_string();

        let jwt = auth.exchange_token(&token).unwrap();
        assert_eq!(
            auth.validate_session(&jwt).unwrap().auth_method,
            "bootstrap"
        );
    }

    #[test]
    fn a_disabled_manager_exchanges_anything() {
        let auth = manager(false);

        let jwt = auth.exchange_token("ignored").unwrap();
        assert_eq!(auth.validate_session(&jwt).unwrap().auth_method, "disabled");
    }
}
