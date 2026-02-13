//! Authentication manager

use anyhow::Result;

use super::jwt::{JwtError, SessionClaims, create_session_token, validate_session_token};
use crate::core::SecretManager;
use crate::core::constants::DEFAULT_USER_ID;
use crate::utils::crypto;

/// Main authentication manager
#[derive(Debug)]
pub struct AuthManager {
    signing_key: Vec<u8>,
    bootstrap_token: String,
    enabled: bool,
}

impl AuthManager {
    /// Initialize the authentication manager
    pub async fn init(secrets: &SecretManager, enabled: bool) -> Result<Self> {
        let signing_key = secrets.get_jwt_signing_key().await?;
        let bootstrap_token = crypto::generate_token(32);

        if enabled {
            tracing::debug!("Authentication enabled");
        } else {
            tracing::warn!("Authentication DISABLED");
        }

        tracing::debug!("Bootstrap token generated");
        Ok(Self {
            signing_key,
            bootstrap_token,
            enabled,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn bootstrap_token(&self) -> &str {
        &self.bootstrap_token
    }

    /// Exchange bootstrap token for JWT session token
    /// Bootstrap auth always creates a session for the default local user
    pub fn exchange_token(&self, token: &str) -> Result<String> {
        if !self.enabled {
            return create_session_token(&self.signing_key, DEFAULT_USER_ID, "disabled");
        }

        if !crypto::constant_time_eq(&self.bootstrap_token, token) {
            anyhow::bail!("Invalid bootstrap token");
        }

        create_session_token(&self.signing_key, DEFAULT_USER_ID, "bootstrap")
    }

    /// Validate a JWT session token
    pub fn validate_session(&self, jwt: &str) -> Result<SessionClaims, JwtError> {
        validate_session_token(jwt, &self.signing_key)
    }
}
