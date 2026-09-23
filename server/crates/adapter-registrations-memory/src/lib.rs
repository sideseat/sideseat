//! In-process implementation of the SDK registration-store port.

use std::collections::HashSet;

use async_trait::async_trait;
use dashmap::DashMap;
use sideseat_ports::registrations::{
    DisplacedOwner, RegistrationEntry, RegistrationKind, RegistrationStore, RegistrationStoreError,
    UpsertOutcome,
};
use sideseat_ports::types::ProjectId;

type Key = (ProjectId, RegistrationKind, String);

#[derive(Default)]
pub struct MemoryRegistrationStore {
    entries: DashMap<Key, RegistrationEntry>,
    by_project: DashMap<ProjectId, HashSet<Key>>,
    by_client: DashMap<String, HashSet<Key>>,
    by_connection: DashMap<String, HashSet<Key>>,
}

impl MemoryRegistrationStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn unindex(&self, key: &Key, previous: &RegistrationEntry) {
        if let Some(mut set) = self.by_project.get_mut(&previous.project_id) {
            set.remove(key);
        }
        if let Some(mut set) = self.by_client.get_mut(&previous.owner_client_id) {
            set.remove(key);
        }
        if let Some(mut set) = self.by_connection.get_mut(&previous.owner_connection_id) {
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
        match self.entries.entry(key.clone()) {
            dashmap::Entry::Occupied(mut occupied) => {
                let existing = occupied.get_mut();
                if existing.owner_connection_id != entry.owner_connection_id {
                    if let Some(mut set) = self.by_connection.get_mut(&existing.owner_connection_id)
                    {
                        set.remove(&key);
                    }
                    self.by_connection
                        .entry(entry.owner_connection_id.clone())
                        .or_default()
                        .insert(key.clone());
                }
                let outcome = if existing.owner_client_id == entry.owner_client_id {
                    UpsertOutcome::UpdatedSameOwner
                } else {
                    let previous = DisplacedOwner {
                        client_id: existing.owner_client_id.clone(),
                        instance_id: existing.owning_instance_id.clone(),
                    };
                    if let Some(mut set) = self.by_client.get_mut(&existing.owner_client_id) {
                        set.remove(&key);
                    }
                    self.by_client
                        .entry(entry.owner_client_id.clone())
                        .or_default()
                        .insert(key);
                    UpsertOutcome::Replaced(previous)
                };
                *existing = entry;
                Ok(outcome)
            }
            dashmap::Entry::Vacant(vacant) => {
                self.by_project
                    .entry(entry.project_id.clone())
                    .or_default()
                    .insert(key.clone());
                self.by_client
                    .entry(entry.owner_client_id.clone())
                    .or_default()
                    .insert(key.clone());
                self.by_connection
                    .entry(entry.owner_connection_id.clone())
                    .or_default()
                    .insert(key);
                vacant.insert(entry);
                Ok(UpsertOutcome::Inserted)
            }
        }
    }

    async fn remove(
        &self,
        project_id: &ProjectId,
        kind: RegistrationKind,
        name: &str,
        by_client_id: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError> {
        let key = (project_id.clone(), kind, name.to_owned());
        let removed = self
            .entries
            .remove_if(&key, |_, value| value.owner_client_id == by_client_id)
            .map(|(_, value)| value);
        if let Some(entry) = &removed {
            self.unindex(&key, entry);
        }
        Ok(removed)
    }

    async fn list(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError> {
        let keys: Vec<Key> = match self.by_project.get(project_id) {
            Some(set) => set.iter().cloned().collect(),
            None => return Ok(Vec::new()),
        };
        Ok(keys
            .into_iter()
            .filter_map(|key| self.entries.get(&key).map(|entry| entry.clone()))
            .collect())
    }

    async fn find(
        &self,
        project_id: &ProjectId,
        kind: RegistrationKind,
        name: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError> {
        let key = (project_id.clone(), kind, name.to_owned());
        Ok(self.entries.get(&key).map(|entry| entry.clone()))
    }

    async fn remove_all_for_connection(
        &self,
        connection_id: &str,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError> {
        let keys: Vec<Key> = match self.by_connection.remove(connection_id) {
            Some((_, set)) => set.into_iter().collect(),
            None => return Ok(Vec::new()),
        };
        let mut removed = Vec::with_capacity(keys.len());
        for key in keys {
            let Some((_, entry)) = self
                .entries
                .remove_if(&key, |_, value| value.owner_connection_id == connection_id)
            else {
                continue;
            };
            if let Some(mut set) = self.by_project.get_mut(&entry.project_id) {
                set.remove(&key);
            }
            if let Some(mut set) = self.by_client.get_mut(&entry.owner_client_id) {
                set.remove(&key);
            }
            removed.push(entry);
        }
        Ok(removed)
    }

    async fn touch(&self, client_id: &str, now_secs: u64) -> Result<(), RegistrationStoreError> {
        let keys: Vec<Key> = match self.by_client.get(client_id) {
            Some(set) => set.iter().cloned().collect(),
            None => return Ok(()),
        };
        for key in keys {
            if let Some(mut entry) = self.entries.get_mut(&key) {
                entry.last_heartbeat_secs = now_secs;
            }
        }
        Ok(())
    }

    async fn expire_due(
        &self,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError> {
        let cutoff = now_secs.saturating_sub(ttl_secs);
        let stale: Vec<Key> = self
            .entries
            .iter()
            .filter(|entry| entry.last_heartbeat_secs < cutoff)
            .map(|entry| entry.key().clone())
            .collect();
        let mut expired = Vec::with_capacity(stale.len());
        for key in stale {
            if let Some((_, entry)) = self
                .entries
                .remove_if(&key, |_, entry| entry.last_heartbeat_secs < cutoff)
            {
                self.unindex(&key, &entry);
                expired.push(entry);
            }
        }
        Ok(expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_ports::registrations::{RegistrationManifest, UpsertOutcome};

    fn entry(project: &str, name: &str, owner: &str, instance: &str) -> RegistrationEntry {
        entry_on(project, name, owner, &format!("conn-{owner}"), instance)
    }

    fn entry_on(
        project: &str,
        name: &str,
        owner: &str,
        connection: &str,
        instance: &str,
    ) -> RegistrationEntry {
        RegistrationEntry {
            project_id: ProjectId::from(project),
            kind: RegistrationKind::Agent,
            name: name.into(),
            manifest: RegistrationManifest {
                name: name.into(),
                framework: None,
                runtime: None,
                model: None,
                system_prompt: None,
                tools: Vec::new(),
                metadata: serde_json::Value::Null,
            },
            owner_client_id: owner.into(),
            owner_connection_id: connection.into(),
            owning_instance_id: instance.into(),
            last_heartbeat_secs: 100,
        }
    }

    #[tokio::test]
    async fn upsert_inserted_then_same_owner_then_replaced() {
        let store = MemoryRegistrationStore::new();

        assert!(matches!(
            store
                .upsert(entry("p", "agent", "client-1", "instance-a"))
                .await
                .unwrap(),
            UpsertOutcome::Inserted
        ));
        assert!(matches!(
            store
                .upsert(entry("p", "agent", "client-1", "instance-a"))
                .await
                .unwrap(),
            UpsertOutcome::UpdatedSameOwner
        ));
        match store
            .upsert(entry("p", "agent", "client-2", "instance-b"))
            .await
            .unwrap()
        {
            UpsertOutcome::Replaced(previous) => {
                assert_eq!(previous.client_id, "client-1");
                assert_eq!(previous.instance_id, "instance-a");
            }
            other => panic!("expected replacement, got {other:?}"),
        }

        let listing = store.list(&ProjectId::from("p")).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].owner_client_id, "client-2");
    }

    #[tokio::test]
    async fn remove_only_matches_owner() {
        let store = MemoryRegistrationStore::new();
        let project = ProjectId::from("p");
        store
            .upsert(entry("p", "agent", "client-1", "instance-a"))
            .await
            .unwrap();

        assert!(
            store
                .remove(&project, RegistrationKind::Agent, "agent", "other-client",)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.list(&project).await.unwrap().len(), 1);

        assert!(
            store
                .remove(&project, RegistrationKind::Agent, "agent", "client-1",)
                .await
                .unwrap()
                .is_some()
        );
        assert!(store.list(&project).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn expiration_drops_only_stale_entries() {
        let store = MemoryRegistrationStore::new();
        let mut stale = entry("p", "stale", "client-1", "instance");
        stale.last_heartbeat_secs = 100;
        store.upsert(stale).await.unwrap();
        let mut fresh = entry("p", "fresh", "client-2", "instance");
        fresh.last_heartbeat_secs = 1_080;
        store.upsert(fresh).await.unwrap();

        let expired = store.expire_due(1_100, 50).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].name, "stale");
        let survivors = store.list(&ProjectId::from("p")).await.unwrap();
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].name, "fresh");
    }

    #[tokio::test]
    async fn remove_all_for_connection_is_scoped_and_idempotent() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p", "a", "client-1", "instance"))
            .await
            .unwrap();
        store
            .upsert(entry("p", "b", "client-1", "instance"))
            .await
            .unwrap();
        store
            .upsert(entry("p", "c", "client-2", "instance"))
            .await
            .unwrap();

        assert_eq!(
            store
                .remove_all_for_connection("conn-client-1")
                .await
                .unwrap()
                .len(),
            2
        );
        let remaining = store.list(&ProjectId::from("p")).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].owner_client_id, "client-2");
        assert!(
            store
                .remove_all_for_connection("conn-client-1")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn reconnect_teardown_preserves_the_live_owner() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry_on("p", "agent", "client", "old", "instance"))
            .await
            .unwrap();
        store
            .upsert(entry_on("p", "agent", "client", "new", "instance"))
            .await
            .unwrap();
        assert!(
            store
                .remove_all_for_connection("old")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.list(&ProjectId::from("p")).await.unwrap().len(), 1);
        assert_eq!(
            store.remove_all_for_connection("new").await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn listing_is_scoped_by_project() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p1", "a", "client-1", "instance"))
            .await
            .unwrap();
        store
            .upsert(entry("p2", "b", "client-2", "instance"))
            .await
            .unwrap();

        assert_eq!(store.list(&ProjectId::from("p1")).await.unwrap().len(), 1);
        assert_eq!(store.list(&ProjectId::from("p2")).await.unwrap().len(), 1);
        assert!(
            store
                .list(&ProjectId::from("missing"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn concurrent_upserts_have_one_insert_winner() {
        let store = std::sync::Arc::new(MemoryRegistrationStore::new());
        let mut tasks = Vec::new();
        for index in 0..32 {
            let store = std::sync::Arc::clone(&store);
            tasks.push(tokio::spawn(async move {
                store
                    .upsert(entry_on(
                        "p",
                        "agent",
                        &format!("client-{index}"),
                        &format!("conn-{index}"),
                        "instance",
                    ))
                    .await
                    .unwrap()
            }));
        }
        let mut inserted = 0;
        for task in tasks {
            if matches!(task.await.unwrap(), UpsertOutcome::Inserted) {
                inserted += 1;
            }
        }
        assert_eq!(inserted, 1);
    }

    #[tokio::test]
    async fn find_and_find_by_name_obey_kind_precedence() {
        let store = MemoryRegistrationStore::new();
        let project = ProjectId::from("p");
        let mut graph = entry("p", "shared", "graph-client", "instance");
        graph.kind = RegistrationKind::Graph;
        store.upsert(graph).await.unwrap();
        let mut agent = entry("p", "shared", "agent-client", "instance");
        agent.kind = RegistrationKind::Agent;
        store.upsert(agent).await.unwrap();
        let mut mcp = entry("p", "tools", "mcp-client", "instance");
        mcp.kind = RegistrationKind::Mcp;
        store.upsert(mcp).await.unwrap();

        assert_eq!(
            store
                .find(&project, RegistrationKind::Graph, "shared")
                .await
                .unwrap()
                .unwrap()
                .owner_client_id,
            "graph-client"
        );
        assert!(
            store
                .find(&project, RegistrationKind::Swarm, "shared")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .find_by_name(&project, "shared")
                .await
                .unwrap()
                .unwrap()
                .kind,
            RegistrationKind::Agent
        );
        assert!(
            store
                .find_by_name(&project, "tools")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn touch_uses_the_callers_clock_value() {
        let store = MemoryRegistrationStore::new();
        store
            .upsert(entry("p", "agent", "client", "instance"))
            .await
            .unwrap();

        store.touch("client", 42_000).await.unwrap();

        let listing = store.list(&ProjectId::from("p")).await.unwrap();
        assert_eq!(listing[0].last_heartbeat_secs, 42_000);
    }
}
