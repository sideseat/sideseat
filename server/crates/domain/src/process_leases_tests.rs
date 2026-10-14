use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use tempfile::TempDir;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
    }
}

async fn store() -> (TempDir, Arc<dyn ProcessLeases>) {
    let root = TempDir::new().expect("temp dir");
    let sqlite = SqliteService::init(
        &AppStorage::init_for_test(root.path().to_path_buf()),
        Arc::new(TestClock),
    )
    .await
    .expect("sqlite");
    (root, Arc::new(SqliteRepository(Arc::new(sqlite))))
}

async fn live(store: &dyn ProcessLeases, role: ProcessRole) -> Vec<String> {
    store.live_process_leases(role, "").await.expect("leases")
}

/// A repair refused for a running server leaves no lease of its own, which would keep the server's restart out.
#[tokio::test]
async fn a_repair_is_refused_while_a_server_holds_a_live_lease_and_leaves_none() {
    let (_root, store) = store().await;
    store
        .renew_process_lease("server-a", ProcessRole::Server, LEASE_SECS)
        .await
        .unwrap();
    let error = ProcessLease::acquire(
        Arc::clone(&store),
        ProcessRole::RestoreRepair,
        Duration::ZERO,
    )
    .await
    .err()
    .expect("refused");
    assert!(
        matches!(&error, ProcessLeaseError::Excluded { role: "server", processes } if processes == &["server-a"]),
        "{error}"
    );
    assert!(
        live(store.as_ref(), ProcessRole::RestoreRepair)
            .await
            .is_empty()
    );
}

/// A lease that ran out - a server that died without ending it - excludes nothing.
#[tokio::test]
async fn a_lease_that_ran_out_excludes_nothing() {
    let (_root, store) = store().await;
    store
        .renew_process_lease("server-a", ProcessRole::Server, -1)
        .await
        .unwrap();
    let lease = ProcessLease::acquire(
        Arc::clone(&store),
        ProcessRole::RestoreRepair,
        Duration::ZERO,
    )
    .await
    .expect("acquired");
    assert_eq!(
        live(store.as_ref(), ProcessRole::RestoreRepair).await,
        [lease.id().to_string()]
    );
    lease.end().await;
    assert!(
        live(store.as_ref(), ProcessRole::RestoreRepair)
            .await
            .is_empty()
    );
}

/// No server starts while a repair holds its lease, and one starts once the repair has ended it.
#[tokio::test]
async fn a_server_is_refused_while_a_repair_holds_its_lease() {
    let (_root, store) = store().await;
    let repair = ProcessLease::acquire(
        Arc::clone(&store),
        ProcessRole::RestoreRepair,
        Duration::ZERO,
    )
    .await
    .expect("repair");
    let error = ProcessLease::acquire(Arc::clone(&store), ProcessRole::Server, Duration::ZERO)
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            &error,
            ProcessLeaseError::Excluded {
                role: "restore_repair",
                ..
            }
        ),
        "{error}"
    );
    assert!(live(store.as_ref(), ProcessRole::Server).await.is_empty());
    repair.end().await;
    ProcessLease::acquire(store, ProcessRole::Server, Duration::ZERO)
        .await
        .expect("a server starts after the repair")
        .end()
        .await;
}

/// The restore took the server's lease away with the rest of the table, and the server writes it again while the
/// repair settles: the repair's second look sees it.
#[tokio::test]
async fn a_server_lease_written_while_the_repair_settles_refuses_it() {
    let (_root, store) = store().await;
    let server = Arc::clone(&store);
    let renewal = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        server
            .renew_process_lease("server-a", ProcessRole::Server, LEASE_SECS)
            .await
            .unwrap();
    });
    let refused = ProcessLease::acquire(
        Arc::clone(&store),
        ProcessRole::RestoreRepair,
        Duration::from_millis(300),
    )
    .await;
    renewal.await.unwrap();
    assert!(
        matches!(
            refused,
            Err(ProcessLeaseError::Excluded { role: "server", .. })
        ),
        "a server that renewed during the settle was missed"
    );
    assert!(
        live(store.as_ref(), ProcessRole::RestoreRepair)
            .await
            .is_empty()
    );
}

/// Two repairs at once advance the same sequences and replay the same journal against each other.
#[tokio::test]
async fn a_repair_is_refused_while_another_repair_holds_its_lease() {
    let (_root, store) = store().await;
    let first = ProcessLease::acquire(
        Arc::clone(&store),
        ProcessRole::RestoreRepair,
        Duration::ZERO,
    )
    .await
    .expect("first repair");
    let error = ProcessLease::acquire(
        Arc::clone(&store),
        ProcessRole::RestoreRepair,
        Duration::ZERO,
    )
    .await
    .err()
    .expect("refused");
    assert!(
        matches!(&error, ProcessLeaseError::Excluded { role: "restore_repair", processes } if processes == &[first.id().to_string()]),
        "{error}"
    );
    first.end().await;
}

#[derive(Default)]
struct CountingLeases {
    renewals: AtomicUsize,
    failing: AtomicBool,
    repair_live: AtomicBool,
    ended: AtomicUsize,
}

#[async_trait]
impl ProcessLeases for CountingLeases {
    async fn renew_process_lease(&self, _: &str, _: ProcessRole, _: i64) -> Result<(), DataError> {
        self.renewals.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(DataError::Io(std::io::Error::other(
                "the store is unreachable",
            )));
        }
        Ok(())
    }

    async fn end_process_lease(&self, _: &str) -> Result<(), DataError> {
        self.ended.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn live_process_leases(
        &self,
        role: ProcessRole,
        _: &str,
    ) -> Result<Vec<String>, DataError> {
        Ok(
            (role == ProcessRole::RestoreRepair && self.repair_live.load(Ordering::SeqCst))
                .then(|| "restore_repair-a".to_string())
                .into_iter()
                .collect(),
        )
    }
}

/// A renewer for a server at test speed: renewals every 20 ms, lost after 100 ms.
fn renewer(leases: &Arc<CountingLeases>) -> (watch::Sender<bool>, CollectionFence, JoinHandle<()>) {
    let (stop, stopped) = watch::channel(false);
    let (hold, held) = watch::channel(Some(HeldUntil::new(
        Instant::now() + Duration::from_millis(100),
        0,
    )));
    let passes = Arc::new(Passes::default());
    let renewer = tokio::spawn(renew(
        Renewal {
            store: Arc::clone(leases) as Arc<dyn ProcessLeases>,
            id: "server-a".to_string(),
            role: ProcessRole::Server,
            every: Duration::from_millis(20),
            lost_after: Duration::from_millis(100),
            passes: Arc::clone(&passes),
        },
        Instant::now(),
        stopped,
        hold,
    ));
    (
        stop,
        CollectionFence::following_passes(held, passes),
        renewer,
    )
}

async fn until(fence: &CollectionFence, open: bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while fence.is_open() != open {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the fence never became {}",
            if open { "open" } else { "closed" }
        )
    });
}

/// A held lease is renewed until it is stopped - through a drain longer than one lease - and not after.
#[tokio::test]
async fn a_held_lease_is_renewed_until_stopped() {
    let leases = Arc::new(CountingLeases::default());
    let (stop, fence, renewer) = renewer(&leases);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(fence.is_open(), "renewing, the lease is held");
    stop.send(true).unwrap();
    renewer.await.unwrap();
    let renewed = leases.renewals.load(Ordering::SeqCst);
    assert!(renewed >= 5, "renewed {renewed} times");
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        leases.renewals.load(Ordering::SeqCst),
        renewed,
        "renewed after it was stopped"
    );
}

/// Renewals failing for long enough close the fence - the process keeps running, its collectors stop - and once
/// the store answers again the lease is taken again and the fence opens.
#[tokio::test]
async fn a_lost_lease_closes_the_fence_until_it_is_taken_again() {
    let leases = Arc::new(CountingLeases::default());
    let (stop, fence, renewer) = renewer(&leases);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(fence.is_open(), "renewing, the lease is held");
    let term = fence.term().expect("held");
    leases.failing.store(true, Ordering::SeqCst);
    until(&fence, false).await;
    assert!(!renewer.is_finished(), "a lost lease keeps being retried");
    leases.failing.store(false, Ordering::SeqCst);
    until(&fence, true).await;
    assert!(
        !fence.held_through(term),
        "taken again, the lease is a new term"
    );
    stop.send(true).unwrap();
    renewer.await.unwrap();
}

/// A renewal that hangs - the store neither answers nor fails - closes the fence on time all the same.
#[tokio::test]
async fn a_renewal_that_hangs_closes_the_fence_by_its_deadline() {
    struct Hanging;
    #[async_trait]
    impl ProcessLeases for Hanging {
        async fn renew_process_lease(
            &self,
            _: &str,
            _: ProcessRole,
            _: i64,
        ) -> Result<(), DataError> {
            std::future::pending().await
        }
        async fn end_process_lease(&self, _: &str) -> Result<(), DataError> {
            Ok(())
        }
        async fn live_process_leases(
            &self,
            _: ProcessRole,
            _: &str,
        ) -> Result<Vec<String>, DataError> {
            Ok(Vec::new())
        }
    }
    let (_stop, stopped) = watch::channel(false);
    let started = Instant::now();
    let (hold, held) = watch::channel(Some(HeldUntil::new(
        started + Duration::from_millis(100),
        0,
    )));
    tokio::spawn(renew(
        Renewal {
            store: Arc::new(Hanging),
            id: "server-a".to_string(),
            role: ProcessRole::Server,
            every: Duration::from_millis(80),
            lost_after: Duration::from_millis(100),
            passes: Arc::default(),
        },
        started,
        stopped,
        hold,
    ));
    let fence = CollectionFence::following(held);
    until(&fence, false).await;
    assert!(
        started.elapsed() < Duration::from_millis(160),
        "closed {:?} after the last renewal, past its 100 ms deadline",
        started.elapsed()
    );
}

/// A lost lease is not taken again while a restore repair holds one: the fence stays closed, and the server's
/// attempt leaves no lease behind.
#[tokio::test]
async fn a_lost_lease_is_not_taken_again_while_a_repair_holds_one() {
    let leases = Arc::new(CountingLeases::default());
    let (stop, fence, renewer) = renewer(&leases);
    leases.failing.store(true, Ordering::SeqCst);
    until(&fence, false).await;
    leases.repair_live.store(true, Ordering::SeqCst);
    leases.failing.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!fence.is_open(), "taken again beside a repair");
    assert!(
        leases.ended.load(Ordering::SeqCst) >= 1,
        "a refused attempt left its lease"
    );
    leases.repair_live.store(false, Ordering::SeqCst);
    until(&fence, true).await;
    stop.send(true).unwrap();
    renewer.await.unwrap();
}

/// A lease taken again opens its fence only once the passes entered before its loss have left - none can enter
/// meanwhile, so nothing begun after the retake holds it closed - and then at once.
#[tokio::test]
async fn a_retaken_lease_waits_for_the_passes_begun_before_its_loss() {
    let leases = Arc::new(CountingLeases::default());
    let (stop, fence, renewer) = renewer(&leases);
    let before = fence.enter("sweep").expect("open");
    leases.failing.store(true, Ordering::SeqCst);
    until(&fence, false).await;
    leases.failing.store(false, Ordering::SeqCst);
    let renewed = leases.renewals.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        leases.renewals.load(Ordering::SeqCst) > renewed + 3,
        "taken again and renewed"
    );
    assert!(
        !fence.is_open(),
        "opened while a pass begun before the loss ran"
    );
    assert!(fence.enter("after the retake").is_none());
    let status = fence.status();
    assert_eq!(
        status
            .running
            .iter()
            .map(|pass| pass.name)
            .collect::<Vec<_>>(),
        ["sweep"]
    );
    drop(before);
    until(&fence, true).await;
    stop.send(true).unwrap();
    renewer.await.unwrap();
}
