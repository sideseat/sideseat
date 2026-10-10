use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// -- Scoping --

/// The scope half of a secret's key.
///
/// One `strum` declaration for a vocabulary that had four: an `as_str` table, a `Display` built on it, a
/// `FromStr` table, and the `serde` renames. These spellings are the stored key prefix - `global/...`,
/// `org/<id>/...` - in `secrets.json` and in the OS credential store's vault blob, so they are read back
/// by exactly the string they were written with. Matching is case-sensitive, as it was.
#[derive(
    Debug,
    Clone,
    Copy,
    Hash,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::IntoStaticStr,
    strum::VariantArray,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SecretScopeKind {
    Global,
    #[serde(rename = "org")]
    #[strum(to_string = "org")]
    Organization,
    Project,
    User,
}

impl SecretScopeKind {
    /// The stored spelling.
    pub fn as_str(&self) -> &'static str {
        self.into()
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
    #[cfg(test)]
    pub fn project(id: impl Into<String>) -> Self {
        Self {
            kind: SecretScopeKind::Project,
            id: Some(id.into()),
        }
    }
    #[cfg(test)]
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

/// The scope kind, with the refusal wording a malformed key has always been answered with.
fn parse_scope_kind(value: &str) -> Result<SecretScopeKind, String> {
    SecretScopeKind::from_str(value).map_err(|_| format!("unknown scope kind: {}", value))
}

impl FromStr for SecretKey {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.splitn(3, '/').collect();
        match parts.as_slice() {
            [kind_str, name] => {
                let kind = parse_scope_kind(kind_str)?;
                if kind != SecretScopeKind::Global {
                    return Err(format!("scope '{}' requires an id", kind_str));
                }
                Ok(Self {
                    name: name.to_string(),
                    scope: SecretScope::global(),
                })
            }
            [kind_str, id, name] => {
                let kind = parse_scope_kind(kind_str)?;
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
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            created_at: now,
            updated_at: now,
        }
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
    pub fn new(value: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            value: value.into(),
            metadata: SecretMetadata::new(now),
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
        // A key already scoped wins over a flat one that maps onto it, being the later form; decided by rule,
        // not by which the old map happened to yield last.
        let (scoped, flat): (Vec<_>, Vec<_>) = std::mem::take(&mut self.secrets)
            .into_iter()
            .partition(|(key, _)| key.contains('/'));
        self.secrets.extend(scoped);
        for (key, secret) in flat {
            self.secrets
                .entry(format!("global/{key}"))
                .or_insert(secret);
        }
        self.version = VAULT_VERSION;
        true
    }

    pub(crate) fn get_secret(&self, key: &SecretKey) -> Option<Secret> {
        self.secrets.get(&key.to_string()).cloned()
    }

    /// Set a secret, preserving created_at on update
    pub(crate) fn set_secret(&mut self, key: &SecretKey, secret: &Secret, now: DateTime<Utc>) {
        let k = key.to_string();
        if let Some(existing) = self.secrets.get(&k) {
            let mut s = secret.clone();
            s.metadata.created_at = existing.metadata.created_at;
            s.metadata.updated_at = now;
            self.secrets.insert(k, s);
        } else {
            self.secrets.insert(k, secret.clone());
        }
    }

    /// Delete a secret, returns true if it existed
    pub(crate) fn delete_secret(&mut self, key: &SecretKey) -> bool {
        self.secrets.remove(&key.to_string()).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_secret(value: impl Into<String>) -> Secret {
        Secret::new(value, DateTime::UNIX_EPOCH)
    }

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
                ("jwt_signing_key".into(), test_secret("abc")),
                ("api_key_secret".into(), test_secret("def")),
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
            .insert("global/key".into(), test_secret("val"));
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
        let s = test_secret("super-secret");
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

    /// A version-1 vault holding a flat key and its scoped form keeps the scoped one: the later form, chosen by
    /// rule. Chosen by the old map's order, either survived, depending on the process.
    #[test]
    fn migrating_a_flat_key_beside_its_scoped_form_keeps_the_scoped_one() {
        for _ in 0..16 {
            let mut vault = SecretVault {
                version: 1,
                ..SecretVault::default()
            };
            vault.secrets.insert("key".to_string(), test_secret("flat"));
            vault
                .secrets
                .insert("global/key".to_string(), test_secret("scoped"));
            assert!(vault.migrate());
            assert_eq!(vault.secrets.len(), 1);
            assert_eq!(vault.secrets["global/key"].value, "scoped");
        }
    }

    #[test]
    fn test_vault_serialization_roundtrip() {
        let mut vault = SecretVault::default();
        vault
            .secrets
            .insert("global/key1".to_string(), test_secret("value1"));
        let json = serde_json::to_string(&vault).unwrap();
        let deserialized: SecretVault = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, VAULT_VERSION);
        assert_eq!(deserialized.secrets.len(), 1);
        assert_eq!(
            deserialized.secrets.get("global/key1").unwrap().value,
            "value1"
        );
    }

    /// The scope prefixes a stored secret key is written and read back under.
    ///
    /// Four tables carried these before - `as_str`, `Display`, `FromStr` and the `serde` renames - so this
    /// asserts all four directions at once. They are a storage vocabulary: a key written `org/<id>/<name>`
    /// into `secrets.json` or the OS credential store's vault blob is only found again by the same string,
    /// and `Organization` is spelled `org`, not `organization`.
    #[test]
    fn every_scope_prefix_round_trips_and_is_unchanged() {
        use strum::VariantArray;

        let expected = [
            (SecretScopeKind::Global, "global"),
            (SecretScopeKind::Organization, "org"),
            (SecretScopeKind::Project, "project"),
            (SecretScopeKind::User, "user"),
        ];
        for (kind, spelling) in expected {
            assert_eq!(kind.as_str(), spelling);
            assert_eq!(kind.to_string(), spelling);
            assert_eq!(SecretScopeKind::from_str(spelling).unwrap(), kind);
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{spelling}\"")
            );
            assert_eq!(
                serde_json::from_str::<SecretScopeKind>(&format!("\"{spelling}\"")).unwrap(),
                kind
            );
        }
        assert_eq!(SecretScopeKind::VARIANTS.len(), expected.len());

        // Case-sensitive, as the hand-written table was, and the refusal wording is unchanged.
        assert!(SecretScopeKind::from_str("Global").is_err());
        assert_eq!(
            super::parse_scope_kind("organization").unwrap_err(),
            "unknown scope kind: organization"
        );
    }
}
