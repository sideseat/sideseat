//! Storing and removing a secret, as a port.
//!
//! Scoped to what the domain does with secrets - write one, delete one - and deliberately **not** reading them:
//! the provider service stores a credential and hands out a handle, and the code that needs the plaintext is the
//! connector that dials the provider. A port with a `get` would invite the plaintext to travel further than it
//! has to.
//!
//! The key and scope types stay in the adapter, because they encode *where* a backend puts things - keychain
//! item, AWS path, Vault mount - which is an implementation fact. What the port needs is a string key and the
//! bytes, so that is what it takes.

use async_trait::async_trait;

/// Writing and removing secrets.
#[async_trait]
pub trait SecretWriter: Send + Sync {
    /// Store `value` under `key`, replacing anything already there.
    async fn put(&self, key: &str, value: &str) -> Result<(), String>;

    /// Remove `key`. Succeeds whether or not it was there, because a delete that has already happened is the
    /// state the caller wanted.
    async fn remove(&self, key: &str) -> Result<(), String>;
}
