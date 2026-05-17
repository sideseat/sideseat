//! Background TTL sweeper for the registration store.
//!
//! Periodically calls `RegistrationStore::expire_due` and emits
//! `PresenceEvent::Expired` for each removed entry on the project's presence
//! topic. Runs until the shared shutdown signal flips.
//!
//! Horizontal scaling: each instance sweeps its own backing store. The
//! current `MemoryRegistrationStore` is per-process so concurrent sweepers
//! don't conflict. A future cluster-aware (Redis) store will need a
//! distributed lock or a leader election here to avoid N×duplicate `Expired`
//! events.

use std::time::{Duration, SystemTime};

use crate::core::constants::REGISTRATION_TTL_SECS;
use crate::data::registrations::{DisplacedOwner, PresenceEvent};

use super::presence;
use super::state::WsState;

/// Sweep interval — half the TTL keeps p50 detection ≤ TTL/2.
const SWEEP_INTERVAL_SECS: u64 = REGISTRATION_TTL_SECS / 2;

pub fn spawn_sweeper(state: WsState) {
    let mut shutdown_rx = state.shutdown_rx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(SWEEP_INTERVAL_SECS.max(1)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // First tick fires immediately; skip it to avoid an empty pass at startup.
        tick.tick().await;

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = tick.tick() => {
                    let now = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs();
                    let expired = match state
                        .registrations
                        .expire_due(now, REGISTRATION_TTL_SECS)
                        .await
                    {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(error = %e, "ws expiry sweep failed");
                            continue;
                        }
                    };
                    for entry in expired {
                        presence::publish(
                            &state,
                            &PresenceEvent::Expired {
                                project_id: entry.project_id,
                                kind: entry.kind,
                                name: entry.name,
                                owner: DisplacedOwner {
                                    client_id: entry.owner_client_id,
                                    instance_id: entry.owning_instance_id,
                                },
                            },
                        )
                        .await;
                    }
                }
            }
        }
    });
}
