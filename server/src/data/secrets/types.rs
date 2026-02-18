use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// -- Scoping --

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretScopeKind {
    Global,
    #[serde(rename = "org")]
    Organization,
    Project,
    User,
}

impl SecretScopeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Organization => "org",
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

impl fmt::Display for SecretScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SecretScopeKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Self::Global),
            "org" => Ok(Self::Organization),
            "project" => Ok(Self::Project),
            "user" => Ok(Self::User),
            _ => Err(format!("unknown scope kind: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecretScope {
    pub kind: SecretScopeKind,
    pub id: Option<String>,
}

impl SecretScope {
    pub fn global() -> Self {
        Self {
            kind: SecretScopeKind::Global,
            id: None,
        }
    }
    pub fn org(id: impl Into<String>) -> Self {
        Self {
            kind: SecretScopeKind::Organization,
            id: Some(id.into()),
        }
    }
    pub fn project(id: impl Into<String>) -> Self {
        Self {
            kind: SecretScopeKind::Project,
            id: Some(id.into()),
        }
    }
    pub fn user(id: impl Into<String>) -> Self {
        Self {
            kind: SecretScopeKind::User,
            id: Some(id.into()),
        }
    }
}

// -- SecretKey --

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct SecretKey {
    pub name: String,
    pub scope: SecretScope,
}

impl SecretKey {
    pub fn global(name: impl Into<String>) -> Self {
        Self::new(name, SecretScope::global())
    }

    pub fn new(name: impl Into<String>, scope: SecretScope) -> Self {
        let name = name.into();
        debug_assert!(!name.is_empty(), "secret name must not be empty");
        debug_assert!(!name.contains('/'), "secret name must not contain '/'");
        if let Some(ref id) = scope.id {
            debug_assert!(!id.contains('/'), "scope id must not contain '/'");
        }
        Self { name, scope }
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope.id {
            None => write!(f, "{}/{}", self.scope.kind, self.name),
            Some(id) => write!(f, "{}/{}/{}", self.scope.kind, id, self.name),
        }
    }
}

impl FromStr for SecretKey {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.splitn(3, '/').collect();
        match parts.as_slice() {
            [kind_str, name] => {
                let kind = SecretScopeKind::from_str(kind_str)?;
                if kind != SecretScopeKind::Global {
                    return Err(format!("scope '{}' requires an id", kind_str));
                }
                Ok(Self {
                    name: name.to_string(),
                    scope: SecretScope::global(),
                })
            }
            [kind_str, id, name] => {
                let kind = SecretScopeKind::from_str(kind_str)?;
                if kind == SecretScopeKind::Global {
                    return Err(format!("global scope does not take an id: {}", s));
                }
                Ok(Self {
                    name: name.to_string(),
                    scope: SecretScope {
                        kind,
                        id: Some(id.to_string()),
                    },
                })
            }
            _ => Err(format!("invalid secret key format: {}", s)),
        }
    }
}

// -- Secret / SecretMetadata --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SecretMetadata {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            created_at: now,
            updated_at: now,
        }
    }
}

impl Default for SecretMetadata {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Secret {
    pub value: String,
    pub metadata: SecretMetadata,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("value", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            metadata: SecretMetadata::new(),
        }
    }
}

// -- SecretVault (local provider internal format) --

pub(crate) const VAULT_VERSION: u32 = 2;

fn default_vault_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SecretVault {
    #[serde(default = "default_vault_version")]
    pub version: u32,
    pub secrets: HashMap<String, Secret>,
}

impl Default for SecretVault {
    fn default() -> Self {
        Self {
            version: VAULT_VERSION,
            secrets: HashMap::new(),
        }
    }
}

impl SecretVault {
    /// Migrate v1 (flat keys) to v2 (scoped keys with "global/" prefix).
    /// Returns true if migration was performed.
    pub fn migrate(&mut self) -> bool {
        if self.version >= VAULT_VERSION {
            return false;
        }
        let old = std::mem::take(&mut self.secrets);
        for (key, secret) in old {
            let new_key = if key.contains('/') {
                key
            } else {
                format!("global/{}", key)
            };
            self.secrets.insert(new_key, secret);
        }
        self.version = VAULT_VERSION;
        true
    }

    pub(crate) fn get_secret(&self, key: &SecretKey) -> Option<Secret> {
        self.secrets.get(&key.to_string()).cloned()
    }

    /// Set a secret, preserving created_at on update
    pub(crate) fn set_secret(&mut self, key: &SecretKey, secret: &Secret) {
        let k = key.to_string();
        if let Some(existing) = self.secrets.get(&k) {
            let mut s = secret.clone();
            s.metadata.created_at = existing.metadata.created_at;
            s.metadata.updated_at = chrono::Utc::now();
            self.secrets.insert(k, s);
        } else {
            self.secrets.insert(k, secret.clone());
        }
    }

    /// Delete a secret, returns true if it existed
    pub(crate) fn delete_secret(&mut self, key: &SecretKey) -> bool {
        self.secrets.remove(&key.to_string()).is_some()
    }

    pub(crate) fn list_secrets(&self, scope: &SecretScope) -> Vec<SecretKey> {
        let prefix = match &scope.id {
            None => format!("{}/", scope.kind),
            Some(id) => format!("{}/{}/", scope.kind, id),
        };
        self.secrets
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .filter_map(|k| k.parse().ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_key_display_global() {
        let key = SecretKey::global("jwt_signing_key");
        assert_eq!(key.to_string(), "global/jwt_signing_key");
    }

    #[test]
    fn test_secret_key_display_scoped() {
        let key = SecretKey::new("api_key", SecretScope::org("acme"));
        assert_eq!(key.to_string(), "org/acme/api_key");
    }

    #[test]
    fn test_secret_key_roundtrip() {
        for path in [
            "global/jwt",
            "org/acme/key",
            "project/p1/token",
            "user/u1/pref",
        ] {
            let key: SecretKey = path.parse().unwrap();
            assert_eq!(key.to_string(), path);
        }
    }

    #[test]
    fn test_secret_key_parse_error() {
        assert!("invalid".parse::<SecretKey>().is_err());
        assert!("org/name".parse::<SecretKey>().is_err());
        assert!("global/extra/name".parse::<SecretKey>().is_err());
    }

    #[test]
    fn test_vault_migration_v1_to_v2() {
        let mut vault = SecretVault {
            version: 1,
            secrets: HashMap::from([
                ("jwt_signing_key".into(), Secret::new("abc")),
                ("api_key_secret".into(), Secret::new("def")),
            ]),
        };
        assert!(vault.migrate());
        assert_eq!(vault.version, VAULT_VERSION);
        assert!(vault.secrets.contains_key("global/jwt_signing_key"));
        assert!(vault.secrets.contains_key("global/api_key_secret"));
        assert!(!vault.secrets.contains_key("jwt_signing_key"));
    }

    #[test]
    fn test_vault_migration_already_v2() {
        let mut vault = SecretVault::default();
        vault
            .secrets
            .insert("global/key".into(), Secret::new("val"));
        assert!(!vault.migrate());
        assert!(vault.secrets.contains_key("global/key"));
    }

    #[test]
    fn test_vault_deserialize_without_version() {
        let json = r#"{"secrets":{"k":{"value":"v","metadata":{"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}}}}"#;
        let vault: SecretVault = serde_json::from_str(json).unwrap();
        assert_eq!(vault.version, 1);
    }

    #[test]
    fn test_vault_deserialize_without_version_triggers_migration() {
        let json = r#"{"secrets":{"jwt_signing_key":{"value":"abc","metadata":{"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}}}}"#;
        let mut vault: SecretVault = serde_json::from_str(json).unwrap();
        assert_eq!(vault.version, 1);
        vault.migrate();
        assert_eq!(vault.version, VAULT_VERSION);
        assert!(vault.secrets.contains_key("global/jwt_signing_key"));
        assert!(!vault.secrets.contains_key("jwt_signing_key"));
    }

    #[test]
    fn test_secret_debug_redacts() {
        let s = Secret::new("super-secret");
        let debug = format!("{:?}", s);
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret"));
    }

    #[test]
    fn test_scope_kind_as_str() {
        assert_eq!(SecretScopeKind::Global.as_str(), "global");
        assert_eq!(SecretScopeKind::Organization.as_str(), "org");
        assert_eq!(SecretScopeKind::Project.as_str(), "project");
        assert_eq!(SecretScopeKind::User.as_str(), "user");
    }

    #[test]
    fn test_scope_kind_roundtrip() {
        for s in ["global", "org", "project", "user"] {
            let kind: SecretScopeKind = s.parse().unwrap();
            assert_eq!(kind.as_str(), s);
        }
    }

    #[test]
    fn test_new_vault_has_current_version() {
        let vault = SecretVault::default();
        assert_eq!(vault.version, VAULT_VERSION);
    }

    #[test]
    fn test_vault_serialization_roundtrip() {
        let mut vault = SecretVault::default();
        vault
            .secrets
            .insert("global/key1".to_string(), Secret::new("value1"));
        let json = serde_json::to_string(&vault).unwrap();
        let deserialized: SecretVault = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, VAULT_VERSION);
        assert_eq!(deserialized.secrets.len(), 1);
        assert_eq!(
            deserialized.secrets.get("global/key1").unwrap().value,
            "value1"
        );
    }
}
