//! Registration store for SDK-driven agent / MCP presence.
//!
//! See `server/protocol/ws-v1/` for the wire protocol.
//!
//! Cluster-wide identity is `(project_id, kind, name)`.
//! Ownership is tracked by `client_id` (stable per SDK process across reconnects).

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::data::topics::TopicMessage;

/// Whether the registration is for an agent, MCP server, swarm, or graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationKind {
    Agent,
    Mcp,
    Swarm,
    Graph,
}

/// Manifest content. The server treats `tools`, `runtime.<extra>`,
/// and `metadata` as opaque JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationManifest {
    pub name: String,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub runtime: Option<serde_json::Value>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// A live entry in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationEntry {
    pub project_id: String,
    pub kind: RegistrationKind,
    pub name: String,
    pub manifest: RegistrationManifest,
    pub owner_client_id: String,
    pub owning_instance_id: String,
    /// Unix-epoch seconds.
    pub last_heartbeat_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplacedOwner {
    pub client_id: String,
    pub instance_id: String,
}

/// Result of an `upsert`.
#[derive(Debug, Clone)]
pub enum UpsertOutcome {
    Inserted,
    UpdatedSameOwner,
    Replaced(DisplacedOwner),
}

/// Presence event broadcast on `presence:{project_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // bounded by WS frame cap
pub enum PresenceEvent {
    Registered(RegistrationEntry),
    Replaced {
        project_id: String,
        kind: RegistrationKind,
        name: String,
        prev_owner: DisplacedOwner,
        new_owner: DisplacedOwner,
    },
    Unregistered {
        project_id: String,
        kind: RegistrationKind,
        name: String,
        owner: DisplacedOwner,
    },
    Expired {
        project_id: String,
        kind: RegistrationKind,
        name: String,
        owner: DisplacedOwner,
    },
}

impl PresenceEvent {
    /// Project this event belongs to. Used to derive the broadcast topic
    /// name (`presence:{project_id}`).
    pub fn project_id(&self) -> &str {
        match self {
            Self::Registered(e) => &e.project_id,
            Self::Replaced { project_id, .. }
            | Self::Unregistered { project_id, .. }
            | Self::Expired { project_id, .. } => project_id,
        }
    }
}

impl TopicMessage for PresenceEvent {
    fn size_bytes(&self) -> usize {
        // Cheap upper-bound: serde_json roundtrip would be exact but allocates.
        // Manifests are bounded by the WS frame cap.
        1024
    }
}

/// Cross-instance control message delivered to the instance that owns a
/// connection. Topic name: `connection_control:{instance_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // bounded by WS frame cap
pub enum ConnectionControl {
    /// Notify a socket that its registration was claimed by a different
    /// `client_id`. Owning instance pushes `replaced` then closes the socket.
    Replaced {
        target_client_id: String,
        kind: RegistrationKind,
        name: String,
    },
    /// Ask the owning instance to dispatch an `agent.invoke` frame to the
    /// socket bound to `target_client_id`.
    Invoke {
        target_client_id: String,
        request_id: String,
        agent_name: String,
        run_input: serde_json::Value,
    },
    /// Ask the owning instance to dispatch an `agent.cancel` frame.
    Cancel {
        target_client_id: String,
        request_id: String,
    },
}

impl TopicMessage for ConnectionControl {
    fn size_bytes(&self) -> usize {
        256
    }
}

/// Storage trait for the registration set.
#[async_trait]
pub trait RegistrationStore: Send + Sync + 'static {
    async fn upsert(
        &self,
        entry: RegistrationEntry,
    ) -> Result<UpsertOutcome, RegistrationStoreError>;

    /// Remove only if the stored owner_client_id matches.
    async fn remove(
        &self,
        project_id: &str,
        kind: RegistrationKind,
        name: &str,
        by_client_id: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError>;

    async fn list(
        &self,
        project_id: &str,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError>;

    /// O(1) lookup for a single entry. Returns `None` if no entry matches.
    async fn find(
        &self,
        project_id: &str,
        kind: RegistrationKind,
        name: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError>;

    /// Remove and return every entry owned by `client_id`. Used on socket
    /// teardown to publish `Unregistered` events efficiently.
    async fn remove_all_for_client(
        &self,
        client_id: &str,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError>;

    /// Refresh the heartbeat timestamp for entries owned by `client_id`.
    async fn touch(&self, client_id: &str) -> Result<(), RegistrationStoreError>;

    /// Remove any entries whose `last_heartbeat_secs + ttl < now_secs`.
    /// Returns the expired entries so the caller can publish events.
    async fn expire_due(
        &self,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RegistrationStoreError {
    #[error("backend error: {0}")]
    Backend(String),
}

// ============================================================================
// In-memory implementation
// ============================================================================

type Key = (String, RegistrationKind, String);

/// Memory implementation. Keeps two secondary indexes so `touch` and `list`
/// are O(set membership) instead of O(N) full scans.
#[derive(Default)]
pub struct MemoryRegistrationStore {
    entries: DashMap<Key, RegistrationEntry>,
    /// project_id -> set of keys
    by_project: DashMap<String, std::collections::HashSet<Key>>,
    /// owner_client_id -> set of keys
    by_client: DashMap<String, std::collections::HashSet<Key>>,
}

impl MemoryRegistrationStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn unindex(&self, key: &Key, prev: &RegistrationEntry) {
        if let Some(mut set) = self.by_project.get_mut(&prev.project_id) {
            set.remove(key);
        }
        if let Some(mut set) = self.by_client.get_mut(&prev.owner_client_id) {
            set.remove(key);
        }
    }
}

#[async_trait]
impl RegistrationStore for MemoryRegistrationStore {
    async fn upsert(
        &self,
        entry: RegistrationEntry,
    ) -> Result<UpsertOutcome, RegistrationStoreError> {
        let key = (entry.project_id.clone(), entry.kind, entry.name.clone());

        // Use `entry()` so the get-or-insert decision is atomic: a concurrent
        // upsert with the same key cannot slip between `get` and `insert`
        // and cause two `Inserted` outcomes for the same logical identity.
        match self.entries.entry(key.clone()) {
            dashmap::Entry::Occupied(mut occ) => {
                let existing = occ.get_mut();
                let outcome = if existing.owner_client_id == entry.owner_client_id {
                    UpsertOutcome::UpdatedSameOwner
                } else {
                    let prev = DisplacedOwner {
                        client_id: existing.owner_client_id.clone(),
                        instance_id: existing.owning_instance_id.clone(),
                    };
                    // Owner changed: move key from old client's index to new.
                    if let Some(mut set) = self.by_client.get_mut(&existing.owner_client_id) {
                        set.remove(&key);
                    }
                    self.by_client
                        .entry(entry.owner_client_id.clone())
                        .or_default()
                        .insert(key.clone());
                    UpsertOutcome::Replaced(prev)
                };
                *existing = entry;
                Ok(outcome)
            }
            dashmap::Entry::Vacant(vac) => {
                // Index BEFORE inserting into `entries` to keep secondary
                // indexes a superset of `entries` for any partial observer.
                self.by_project
                    .entry(entry.project_id.clone())
                    .or_default()
                    .insert(key.clone());
                self.by_client
                    .entry(entry.owner_client_id.clone())
                    .or_default()
                    .insert(key.clone());
                vac.insert(entry);
                Ok(UpsertOutcome::Inserted)
            }
        }
    }

    async fn remove(
        &self,
        project_id: &str,
        kind: RegistrationKind,
        name: &str,
        by_client_id: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError> {
        let key = (project_id.to_string(), kind, name.to_string());
        let removed = self
            .entries
            .remove_if(&key, |_, v| v.owner_client_id == by_client_id)
            .map(|(_, v)| v);
        if let Some(ref entry) = removed {
            self.unindex(&key, entry);
        }
        Ok(removed)
    }

    async fn list(
        &self,
        project_id: &str,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError> {
        let keys: Vec<Key> = match self.by_project.get(project_id) {
            Some(set) => set.iter().cloned().collect(),
            None => return Ok(Vec::new()),
        };
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some(v) = self.entries.get(&k) {
                out.push(v.clone());
            }
        }
        Ok(out)
    }

    async fn find(
        &self,
        project_id: &str,
        kind: RegistrationKind,
        name: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError> {
        let key = (project_id.to_string(), kind, name.to_string());
        Ok(self.entries.get(&key).map(|e| e.clone()))
    }

    async fn touch(&self, client_id: &str) -> Result<(), RegistrationStoreError> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let keys: Vec<Key> = match self.by_client.get(client_id) {
            Some(set) => set.iter().cloned().collect(),
            None => return Ok(()),
        };
        for k in keys {
            if let Some(mut v) = self.entries.get_mut(&k) {
                v.last_heartbeat_secs = now;
            }
        }
        Ok(())
    }

    async fn remove_all_for_client(
        &self,
        client_id: &str,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError> {
        let keys: Vec<Key> = match self.by_client.remove(client_id) {
            Some((_, set)) => set.into_iter().collect(),
            None => return Ok(Vec::new()),
        };
        let mut removed = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some((_, entry)) = self.entries.remove(&k) {
                if let Some(mut set) = self.by_project.get_mut(&entry.project_id) {
                    set.remove(&k);
                }
                removed.push(entry);
            }
        }
        Ok(removed)
    }

    async fn expire_due(
        &self,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError> {
        let cutoff = now_secs.saturating_sub(ttl_secs);
        let mut expired = Vec::new();
        let stale_keys: Vec<Key> = self
            .entries
            .iter()
            .filter(|e| e.last_heartbeat_secs < cutoff)
            .map(|e| e.key().clone())
            .collect();
        for key in stale_keys {
            if let Some((_, v)) = self.entries.remove(&key) {
                self.unindex(&key, &v);
                expired.push(v);
            }
        }
        Ok(expired)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(project: &str, name: &str, owner: &str, instance: &str) -> RegistrationEntry {
        RegistrationEntry {
            project_id: project.into(),
            kind: RegistrationKind::Agent,
            name: name.into(),
            manifest: RegistrationManifest {
                name: name.into(),
                framework: None,
                runtime: None,
                model: None,
                system_prompt: None,
                tools: vec![],
                metadata: serde_json::Value::Null,
            },
            owner_client_id: owner.into(),
            owning_instance_id: instance.into(),
            last_heartbeat_secs: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn upsert_inserted_then_same_owner_then_replaced() {
        let store = MemoryRegistrationStore::new();

        let outcome = store
            .upsert(entry("p", "a", "client-1", "inst-A"))
            .await
            .unwrap();
        assert!(matches!(outcome, UpsertOutcome::Inserted));

        let outcome = store
            .upsert(entry("p", "a", "client-1", "inst-A"))
            .await
            .unwrap();
        assert!(matches!(outcome, UpsertOutcome::UpdatedSameOwner));

        let outcome = store
            .upsert(entry("p", "a", "client-2", "inst-B"))
            .await
            .unwrap();
        match outcome {
            UpsertOutcome::Replaced(prev) => {
                assert_eq!(prev.client_id, "client-1");
                assert_eq!(prev.instance_id, "inst-A");
            }
            other => panic!("expected Replaced, got {other:?}"),
        }

        let listing = store.list("p").await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].owner_client_id, "client-2");
    }

    #[tokio::test]
    async fn remove_only_matches_owner() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p", "a", "client-1", "inst-A"))
            .await
            .unwrap();

        // Wrong owner: no removal.
        let removed = store
            .remove("p", RegistrationKind::Agent, "a", "client-other")
            .await
            .unwrap();
        assert!(removed.is_none());
        assert_eq!(store.list("p").await.unwrap().len(), 1);

        // Correct owner: removed.
        let removed = store
            .remove("p", RegistrationKind::Agent, "a", "client-1")
            .await
            .unwrap();
        assert!(removed.is_some());
        assert!(store.list("p").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn expire_due_returns_and_drops_stale() {
        let store = MemoryRegistrationStore::new();
        let mut e = entry("p", "a", "client-1", "inst-A");
        e.last_heartbeat_secs = 100;
        store.upsert(e).await.unwrap();

        let mut fresh = entry("p", "b", "client-2", "inst-A");
        fresh.last_heartbeat_secs = 1_080;
        store.upsert(fresh).await.unwrap();

        let expired = store.expire_due(1_100, 50).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].name, "a");

        let surviving = store.list("p").await.unwrap();
        assert_eq!(surviving.len(), 1);
        assert_eq!(surviving[0].name, "b");
    }

    #[tokio::test]
    async fn remove_all_for_client_drops_only_that_client() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p", "a", "client-1", "inst-A"))
            .await
            .unwrap();
        store
            .upsert(entry("p", "b", "client-1", "inst-A"))
            .await
            .unwrap();
        store
            .upsert(entry("p", "c", "client-2", "inst-A"))
            .await
            .unwrap();

        let removed = store.remove_all_for_client("client-1").await.unwrap();
        assert_eq!(removed.len(), 2);
        let remaining = store.list("p").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].owner_client_id, "client-2");

        // Idempotent for unknown client.
        let removed_again = store.remove_all_for_client("client-1").await.unwrap();
        assert!(removed_again.is_empty());
    }

    #[tokio::test]
    async fn list_uses_by_project_index() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p1", "a", "client-1", "inst-A"))
            .await
            .unwrap();
        store
            .upsert(entry("p2", "b", "client-2", "inst-A"))
            .await
            .unwrap();
        let p1 = store.list("p1").await.unwrap();
        let p2 = store.list("p2").await.unwrap();
        let p3 = store.list("p3").await.unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p2.len(), 1);
        assert!(p3.is_empty());
    }

    #[tokio::test]
    async fn concurrent_upserts_same_key_yield_one_inserted() {
        // Atomicity contract: only ONE of N concurrent upserts on the same
        // logical key may report `Inserted`; the rest must see the existing
        // entry and report `UpdatedSameOwner` or `Replaced`.
        let store = std::sync::Arc::new(MemoryRegistrationStore::new());
        let mut tasks = Vec::new();
        for i in 0..32 {
            let s = store.clone();
            tasks.push(tokio::spawn(async move {
                s.upsert(entry("p", "racy", &format!("client-{i}"), "inst-A"))
                    .await
                    .unwrap()
            }));
        }
        let mut inserted = 0;
        for t in tasks {
            if matches!(t.await.unwrap(), UpsertOutcome::Inserted) {
                inserted += 1;
            }
        }
        assert_eq!(inserted, 1, "exactly one task should win the Inserted slot");
        assert_eq!(store.list("p").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn find_returns_entry_or_none() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p", "weather", "client-1", "inst-A"))
            .await
            .unwrap();

        let hit = store
            .find("p", RegistrationKind::Agent, "weather")
            .await
            .unwrap();
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().owner_client_id, "client-1");

        let miss = store
            .find("p", RegistrationKind::Agent, "missing")
            .await
            .unwrap();
        assert!(miss.is_none());

        let wrong_kind = store
            .find("p", RegistrationKind::Mcp, "weather")
            .await
            .unwrap();
        assert!(wrong_kind.is_none());
    }

    #[tokio::test]
    async fn touch_updates_heartbeat_for_owner() {
        let store = MemoryRegistrationStore::new();
        let mut e = entry("p", "a", "client-1", "inst-A");
        e.last_heartbeat_secs = 0;
        store.upsert(e).await.unwrap();

        store.touch("client-1").await.unwrap();
        let listing = store.list("p").await.unwrap();
        assert!(listing[0].last_heartbeat_secs > 0);
    }
}
