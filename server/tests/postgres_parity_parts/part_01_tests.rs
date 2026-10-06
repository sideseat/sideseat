use sideseat_ports::traits::{
    FileMetaStore, IdentityStore, ProjectStore, StorageGovernance, TransactionalRepository,
};
use std::collections::HashMap;
use std::sync::Arc;

use sideseat_adapter_postgres::PostgresService;
use sideseat_adapter_sqlite::SqliteService;
use sideseat_core::config::PostgresConfig;
use sideseat_server::app::storage::TransactionalService;

use sideseat_ports::types::{LastOwnerResult, ProjectId};

/// Env var holding a PostgreSQL connection URL, e.g. `postgres://user:pass@127.0.0.1:5433/sideseat`.
const URL_ENV: &str = "SIDESEAT_TEST_POSTGRES_URL";

/// Tables the suite empties between scenarios, children before parents.
///
/// The seeded `default` org, user and project are restored afterwards, because most of the API
/// assumes they exist and the goldens' project id is `default`.
/// Tables the reset leaves alone. Everything else is data a scenario may have written.
///
/// **Derived from the database rather than listed**, which is a correction: the list was hand-maintained and
/// had fallen behind by two tables (`deleted_sessions`, and then `retention_cleanup`). A scenario's rows
/// therefore survived into the next scenario, and the failure that produced was a *different* test - the
/// project list counting rows another scenario had left - which is the worst shape of flake: it points away
/// from its cause. One more table added anywhere and the list is stale again, so it is not a list any more.
const RESET_EXEMPT: &[&str] = &["schema_version"];

fn hash(n: u8) -> String {
    // 64 hex chars, which is what the file columns expect.
    std::iter::repeat_n(format!("{:02x}", n), 32).collect()
}

/// An ordered list of observations, with generated ids replaced by stable labels.
///
/// Without the labels every transcript would differ: ids are cuid2, so the two backends never
/// produce the same ones. With them, "the project I created first" is comparable across backends
/// while still catching a backend that returns the *wrong* project.
#[derive(Default)]
struct Transcript {
    lines: Vec<String>,
    labels: HashMap<String, String>,
}

impl Transcript {
    fn label(&mut self, id: &str) -> String {
        // The seeded ids are the same in both backends and are worth seeing as themselves.
        if id == "default" || id == "local" {
            return id.to_string();
        }
        let next = self.labels.len() + 1;
        self.labels
            .entry(id.to_string())
            .or_insert_with(|| format!("#{next}"))
            .clone()
    }

    fn note(&mut self, what: &str) {
        self.lines.push(what.to_string());
    }

    fn note_id(&mut self, what: &str, id: &str) {
        let label = self.label(id);
        self.lines.push(format!("{what}={label}"));
    }
}

/// Both services, or `None` when no PostgreSQL URL is configured.
async fn pair() -> Option<(
    Arc<SqliteService>,
    sideseat_adapter_postgres::PostgresRepository,
)> {
    let url = match std::env::var(URL_ENV) {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!(
                "skipping PostgreSQL parity: {URL_ENV} is not set (run `make test-postgres`)"
            );
            return None;
        }
    };

    let sqlite_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(":memory:")
        .await
        .expect("in-memory SQLite");
    // `raw_sql`, not `query`: the schema is a multi-statement script, and `query` prepares a single
    // statement - so it stops at the first `;`, including one inside a `--` comment.
    sqlx::raw_sql(sideseat_adapter_sqlite::schema::SCHEMA)
        .execute(&sqlite_pool)
        .await
        .expect("SQLite schema");
    let sqlite = Arc::new(SqliteService::from_pool(
        sqlite_pool,
        Arc::new(sideseat_server::runtime::clock::SystemClock),
    ));

    // Defaults, except the URL: the point is to run the same pool the server runs.
    let config = PostgresConfig {
        url,
        max_connections: 8,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let postgres = Arc::new(
        PostgresService::init(
            &config,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("PostgreSQL connection (is the container up?)"),
    );
    reset_postgres(&postgres).await;

    Some((
        sqlite,
        sideseat_adapter_postgres::PostgresRepository(postgres),
    ))
}

/// The two repositories behind the shared trait, in reference-then-candidate order.
fn repositories(
    sqlite: Arc<SqliteService>,
    postgres: sideseat_adapter_postgres::PostgresRepository,
) -> [Box<dyn TransactionalRepository + Send + Sync>; 2] {
    [
        TransactionalService::Sqlite(sqlite).repository(),
        TransactionalService::Postgres(postgres.0).repository(),
    ]
}

/// Empty the PostgreSQL data tables and restore the seeded rows.
///
/// The container is reused across scenarios, and `--test-threads=1` is what makes that safe; the
/// `make` target passes it for the same reason the ClickHouse one does.
async fn reset_postgres(service: &PostgresService) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname = current_schema() ORDER BY tablename",
    )
    .fetch_all(service.pool())
    .await
    .expect("list the tables to reset");

    let targets: Vec<String> = tables
        .into_iter()
        .filter(|t| !RESET_EXEMPT.contains(&t.as_str()))
        .collect();
    assert!(
        targets.len() > 8,
        "only found {} tables to reset - the query is wrong, not the schema",
        targets.len()
    );

    // One `TRUNCATE ... CASCADE`, so foreign keys do not dictate an order this no longer knows. Deleting table
    // by table needed the list to be in dependency order, which is a second thing to keep correct by hand.
    sqlx::query(&format!("TRUNCATE TABLE {} CASCADE", targets.join(", ")))
        .execute(service.pool())
        .await
        .unwrap_or_else(|e| panic!("truncate {targets:?}: {e}"));
    sqlx::raw_sql(sideseat_adapter_postgres::schema::DEFAULT_DATA)
        .execute(service.pool())
        .await
        .unwrap_or_else(|e| panic!("reseed: {e}"));
}

/// The runtime role must be a real RLS subject, and tenant context must restrict rather than erase reads.
#[tokio::test]
async fn postgres_rls_is_forced_fail_closed_and_bound_per_transaction() {
    let Some((_sqlite, postgres)) = pair().await else {
        return;
    };
    reset_postgres(&postgres).await;

    sqlx::query(
        "INSERT INTO files
             (project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at)
         VALUES
             ('tenant-a', $1, 'application/a', 1, 'sha256', 1, 1, 1),
             ('tenant-b', $2, 'application/b', 1, 'sha256', 1, 1, 1)",
    )
    .bind(hash(41))
    .bind(hash(42))
    .execute(postgres.pool())
    .await
    .expect("seed colliding tenant rows through maintenance");

    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(postgres.runtime_pool())
        .await
        .expect("runtime role");
    assert_eq!(role, sideseat_adapter_postgres::POSTGRES_RUNTIME_ROLE);

    let policy_tables: Vec<(String, bool, bool)> = sqlx::query_as(
        "SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = current_schema()
           AND c.relname = ANY($1::text[])
         ORDER BY c.relname",
    )
    .bind(sideseat_adapter_postgres::schema::TENANT_RLS_TABLES)
    .fetch_all(postgres.pool())
    .await
    .expect("inspect RLS flags");
    assert_eq!(
        policy_tables.len(),
        sideseat_adapter_postgres::schema::TENANT_RLS_TABLES.len()
    );
    assert!(
        policy_tables
            .iter()
            .all(|(_, enabled, forced)| *enabled && *forced),
        "every project table must enable and force RLS: {policy_tables:?}"
    );

    let runtime_owner_pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.relname, pg_get_userbyid(c.relowner)
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = current_schema()
           AND c.relname = ANY($1::text[])
         ORDER BY c.relname",
    )
    .bind(sideseat_adapter_postgres::schema::TENANT_RLS_TABLES)
    .fetch_all(postgres.runtime_pool())
    .await
    .expect("inspect table owners");
    assert!(
        runtime_owner_pairs
            .iter()
            .all(|(_, owner)| owner != sideseat_adapter_postgres::POSTGRES_RUNTIME_ROLE),
        "the runtime role owning a table would silently bypass non-forced future policies: \
         {runtime_owner_pairs:?}"
    );

    let mut tenant_tx = postgres.runtime_pool().begin().await.expect("tenant tx");
    sqlx::query("SELECT set_config('sideseat.project_id', $1, true)")
        .bind("tenant-a")
        .execute(&mut *tenant_tx)
        .await
        .expect("bind tenant context");
    let visible: Vec<String> = sqlx::query_scalar(
        // Deliberately no project predicate: this is the storage backstop oracle.
        "SELECT project_id FROM files ORDER BY project_id",
    )
    .fetch_all(&mut *tenant_tx)
    .await
    .expect("tenant-scoped raw read");
    assert_eq!(visible, vec!["tenant-a"]);
    tenant_tx.commit().await.expect("commit tenant tx");

    let mut unset_tx = postgres.runtime_pool().begin().await.expect("unset tx");
    // A custom GUC is writable by ordinary roles; setting a maintenance-looking value must not grant bypass.
    sqlx::query("SELECT set_config('sideseat.maintenance', 'on', true)")
        .execute(&mut *unset_tx)
        .await
        .expect("set an unprivileged custom option");
    let visible_without_context: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM files ORDER BY project_id")
            .fetch_all(&mut *unset_tx)
            .await
            .expect("fail-closed raw read");
    assert!(
        visible_without_context.is_empty(),
        "a missing tenant context must match no project rows"
    );
    unset_tx.rollback().await.expect("rollback unset tx");

    reset_postgres(&postgres).await;
}

/// PostgreSQL promotes `SUM(BIGINT)` to `NUMERIC`; governance must cast the aggregate before decoding it.
///
/// This is the startup case for a fresh distributed deployment: the seeded project exists, but none of its
/// held-byte tables has rows yet. SQLite's dynamic integer result hid the PostgreSQL-only type mismatch.
#[tokio::test]
async fn empty_postgres_held_bytes_decode_as_zero() {
    let Some((_sqlite, postgres)) = pair().await else {
        return;
    };

    let bytes = postgres
        .held_transactional_bytes(&ProjectId::from("default"))
        .await
        .expect("an empty BIGINT aggregate must decode");
    assert_eq!(bytes, 0);

    reset_postgres(&postgres).await;
}

/// Run one scenario against both backends and require the same transcript.
async fn assert_parity<F, Fut>(name: &str, scenario: F)
where
    F: Fn(Box<dyn TransactionalRepository + Send + Sync>, Transcript) -> Fut,
    Fut: std::future::Future<Output = Transcript>,
{
    let Some((sqlite, postgres)) = pair().await else {
        return;
    };
    let [reference_repo, candidate_repo] = repositories(sqlite, postgres);
    let reference = scenario(reference_repo, Transcript::default()).await;
    let candidate = scenario(candidate_repo, Transcript::default()).await;

    if reference.lines != candidate.lines {
        let mut report = format!("{name}: PostgreSQL disagrees with SQLite\n");
        let len = reference.lines.len().max(candidate.lines.len());
        for i in 0..len {
            let a = reference
                .lines
                .get(i)
                .map(String::as_str)
                .unwrap_or("<end>");
            let b = candidate
                .lines
                .get(i)
                .map(String::as_str)
                .unwrap_or("<end>");
            let mark = if a == b { ' ' } else { '!' };
            report.push_str(&format!("{mark} {i:>3}  sqlite: {a}\n       pg:     {b}\n"));
        }
        panic!("{report}");
    }
}

// ============================================================================
// Scenarios
// ============================================================================

/// `release_trace_files_except`: the survivor keep, and the in-flight writer it must not touch.
///
/// PostgreSQL's version and SQLite's are different statements - `file_hash <> ALL($3::text[])` against an
/// interpolated `NOT IN (?, ?)`, because SQLite has no array type - and only SQLite's had behavioural coverage,
/// since the file tests hardwire it. This is the statement that decides whether a live span keeps its file, so
/// the two must not disagree.
///
/// Three cases, and the empty-keep one is the trap: with no survivors the PostgreSQL predicate becomes
/// `<> ALL('{}')`, which is *true* for every row, while a naive SQLite rendering could emit `NOT IN ()` and be a
/// syntax error - so the shape where everything is releasable is exactly where two hand-written statements
/// diverge.
#[tokio::test]
async fn releasing_all_but_the_survivors_files_behaves_identically() {
    assert_parity("release_trace_files_except", |repo, mut t| async move {
        let keep_hash = hash(1);
        let drop_hash = hash(2);
        let inflight_hash = hash(3);

        for h in [&keep_hash, &drop_hash] {
            repo.upsert_file(&ProjectId::from("default"), h, None, 5, "sha256")
                .await
                .ok();
            repo.insert_trace_file("t1", &ProjectId::from("default"), h)
                .await
                .ok();
        }
        // Through the real path, so this one carries a pending writer.
        repo.associate_file(
            "t1",
            &ProjectId::from("default"),
            &inflight_hash,
            None,
            5,
            "sha256",
        )
        .await
        .ok();

        let mut released = repo
            .release_trace_files_except(
                &ProjectId::from("default"),
                "t1",
                std::slice::from_ref(&keep_hash),
            )
            .await
            .expect("release");
        released.sort();
        t.note(&format!("released: {}", released.len()));
        t.note(&format!(
            "released the survivor's: {}",
            released.contains(&keep_hash)
        ));
        t.note(&format!(
            "released the in-flight one: {}",
            released.contains(&inflight_hash)
        ));

        let mut left = repo
            .get_file_hashes_for_traces(&ProjectId::from("default"), &["t1".to_string()])
            .await
            .expect("read back");
        left.sort();
        t.note(&format!("still associated: {}", left.len()));

        // An empty keep list: everything releasable except the in-flight row.
        let empty = repo
            .release_trace_files_except(&ProjectId::from("default"), "t1", &[])
            .await
            .expect("release with no survivors");
        t.note(&format!("released with no survivors: {}", empty.len()));

        let remaining = repo
            .get_file_hashes_for_traces(&ProjectId::from("default"), &["t1".to_string()])
            .await
            .expect("read back again");
        t.note(&format!("associated after that: {}", remaining.len()));

        // A trace with nothing to release.
        let none = repo
            .release_trace_files_except(&ProjectId::from("default"), "absent", &[])
            .await
            .expect("release for an unknown trace");
        t.note(&format!("released for an unknown trace: {}", none.len()));

        t
    })
    .await;
}

#[tokio::test]
async fn retention_cleanup_intent_behaves_identically() {
    assert_parity("retention_cleanup_intent", |repo, mut t| async move {
        // No project row needed: the candidate table carries no foreign key, deliberately - a cleanup that is
        // owed must remain findable after the project row has gone.

        // Recorded, and idempotent: the same trace twice is one candidate.
        let written = repo
            .record_retention_cleanup(
                &ProjectId::from("p1"),
                &["t1".to_string(), "t2".to_string()],
            )
            .await
            .expect("record");
        t.note(&format!("recorded: {}", written.len()));

        // Idempotent for the *row*, and it bumps the token: re-recording means new work behind the same
        // identity, so a claim an earlier worker still holds must stop matching.
        let again_written = repo
            .record_retention_cleanup(&ProjectId::from("p1"), &["t1".to_string()])
            .await
            .expect("record again");
        t.note(&format!(
            "re-recording bumped the token: {}",
            again_written
                .iter()
                .any(|(trace, token)| trace == "t1" && *token > 1)
        ));

        let mut first = repo.claim_retention_cleanup(10, 600).await.expect("claim");
        first.sort();
        t.note(&format!("claimed: {}", first.len()));

        // Leased, so an immediate second claim finds nothing - this is what stops two replicas doing the same
        // reconciliation, and what stops one re-entering its own.
        let again = repo
            .claim_retention_cleanup(10, 600)
            .await
            .expect("reclaim");
        t.note(&format!("claimed while leased: {}", again.len()));

        // **A stale token must not complete.** This is what stops a worker that paused past its lease from
        // deleting an intent a later retention pass recorded: if that newer work then fails, nothing else knows
        // it is owed. The tokens come from `first` - the claim actually held - because a fresh claim would not
        // find these rows while their lease stands, which is the previous point.
        repo.complete_retention_cleanup(&ProjectId::from("p1"), &[("t2".to_string(), 999)])
            .await
            .expect("stale completion");
        let due = repo
            .claim_retention_cleanup(10, 0)
            .await
            .expect("claim after a stale completion");
        t.note(&format!(
            "still leased after a stale completion: {}",
            due.len()
        ));

        // Completing on the token actually held leaves the other owed.
        let t1_token = first
            .iter()
            .find(|(_, trace, _)| trace == "t1")
            .map(|(_, _, token)| *token)
            .expect("t1 was claimed");
        repo.complete_retention_cleanup(&ProjectId::from("p1"), &[("t1".to_string(), t1_token)])
            .await
            .expect("complete");

        // t2 survived the stale completion and is still recorded; t1 is gone. Asked with a zero lease so the
        // leases above do not hide the answer.
        let owed = repo.claim_retention_cleanup(10, 0).await.expect("claim");
        t.note(&format!(
            "t2 survived the stale completion: {}",
            owed.iter().any(|(_, trace, _)| trace == "t2")
        ));
        t.note(&format!(
            "t1 completed on its own token: {}",
            !owed.iter().any(|(_, trace, _)| trace == "t1")
        ));

        // A limit bounds the claim.
        repo.record_retention_cleanup(
            &ProjectId::from("p1"),
            &["t3".to_string(), "t4".to_string()],
        )
        .await
        .expect("record more");
        let bounded = repo.claim_retention_cleanup(1, 0).await.expect("claim one");
        t.note(&format!("bounded claim size: {}", bounded.len()));

        t
    })
    .await;
}

/// Projects, including the deletion fence: what a claimed project looks like to every read.
#[tokio::test]
async fn projects_behave_identically() {
    assert_parity("projects", |repo, mut t| async move {
        let alpha = repo
            .create_project("default", "Alpha")
            .await
            .expect("create Alpha");
        let beta = repo
            .create_project("default", "Beta")
            .await
            .expect("create Beta");
        t.note_id("created", &alpha.id);
        t.note_id("created", &beta.id);

        let (listed, total) = repo.list_projects_for_org("default", 1, 50).await.unwrap();
        t.note(&format!("listed_total={total}"));
        // Sorted by name before recording, because the *order* of rows that share a `created_at` is broken by
        // `id` - and the two backends generate their ids independently, so a comparison of tie-broken order is a
        // comparison of two random number generators. What parity means here is the same projects with the same
        // names; the newest-first rule and its tie-break are asserted separately, per backend, where the ids are
        // the same ones the assertion was made about.
        let mut listed: Vec<_> = listed;
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        for project in &listed {
            let label = t.label(&project.id);
            t.note(&format!("listed={label} name={}", project.name));
        }

        let renamed = repo
            .update_project(&alpha.id, "Alpha Renamed")
            .await
            .unwrap();
        t.note(&format!(
            "renamed_to={:?}",
            renamed.as_ref().map(|p| p.name.clone())
        ));

        // The fence.
        t.note(&format!(
            "claim={}",
            repo.claim_project_for_deletion(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "claim_again={}",
            repo.claim_project_for_deletion(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "claim_missing={}",
            repo.claim_project_for_deletion("no-such-project")
                .await
                .unwrap()
        ));
        t.note(&format!(
            "accepts_writes_while_claimed={}",
            repo.project_accepts_writes(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "get_claimed_is_none={}",
            repo.get_project(&beta.id).await.unwrap().is_none()
        ));
        let (after_claim, total_after) =
            repo.list_projects_for_org("default", 1, 50).await.unwrap();
        t.note(&format!(
            "listed_after_claim={} total={total_after}",
            after_claim.len()
        ));
        t.note(&format!(
            "rename_claimed_is_none={}",
            repo.update_project(&beta.id, "Nope")
                .await
                .unwrap()
                .is_none()
        ));
        t.note(&format!(
            "stale_at_zero={}",
            repo.get_stale_claimed_projects(0).await.unwrap().len()
        ));
        t.note(&format!(
            "stale_at_a_day={}",
            repo.get_stale_claimed_projects(86_400).await.unwrap().len()
        ));

        fn describe<T>(outcome: &LastOwnerResult<T>) -> &'static str {
            match outcome {
                LastOwnerResult::Success(_) => "success",
                LastOwnerResult::LastOwner => "last_owner",
                LastOwnerResult::NotFound => "not_found",
            }
        }

        // The barrier: repeated evidence, and a sweep that finds data starts it over.
        for pass in 1..=3 {
            t.note(&format!(
                "sweep_clean_{pass}={}",
                repo.record_project_sweep(&beta.id, true, 3, 0)
                    .await
                    .unwrap()
            ));
        }
        t.note(&format!(
            "sweep_found_data={}",
            repo.record_project_sweep(&beta.id, false, 3, 0)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "sweep_after_reset={}",
            repo.record_project_sweep(&beta.id, true, 3, 0)
                .await
                .unwrap()
        ));

        // Organizations carry the same tombstone, and their rows wait for their projects.
        t.note(&format!(
            "projects_of_org={}",
            repo.count_projects_of_organization("default")
                .await
                .unwrap()
        ));
        t.note(&format!(
            "claim_org={}",
            repo.claim_organization_for_deletion("default")
                .await
                .unwrap()
        ));
        t.note(&format!(
            "claim_org_again={}",
            repo.claim_organization_for_deletion("default")
                .await
                .unwrap()
        ));
        t.note(&format!(
            "stale_orgs={}",
            repo.get_stale_claimed_organizations(0).await.unwrap().len()
        ));
        // Membership mutations refuse for a tombstoned organization: writing into one is writing into
        // something no read can see and the cleanup is about to cascade away.
        t.note(&format!(
            "add_member_to_deleting_org_is_err={}",
            repo.add_member("default", "local", "member").await.is_err()
        ));
        t.note(&format!(
            "promote_in_deleting_org={}",
            describe(
                &repo
                    .update_role_atomic("default", "local", "admin")
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "org_reads_absent={}",
            repo.get_organization("default").await.unwrap().is_none()
        ));

        t.note(&format!(
            "deleted={}",
            repo.delete_project(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "delete_again={}",
            repo.delete_project(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "remembered_after_removal={}",
            repo.claim_deleted_projects_for_check(0, 100)
                .await
                .unwrap()
                .len()
        ));
        // The backoff arithmetic, which is exactly the kind of expression that differs between dialects -
        // PostgreSQL has no two-argument `MIN` and no `integer << bigint`, both of which this suite caught.
        // Two quiet reports, each under the token that claimed it - the arithmetic being checked is the
        // backoff, and it must produce the same schedule on both dialects.
        for _ in 0..2 {
            for (id, token) in repo.claim_deleted_projects_for_check(0, 10).await.unwrap() {
                repo.record_deleted_project_check(&ProjectId::from(&id), token, true, 0, 0)
                    .await
                    .unwrap();
            }
        }
        t.note(&format!(
            "due_at_base_after_two_quiet_checks={}",
            repo.claim_deleted_projects_for_check(0, 10)
                .await
                .unwrap()
                .len()
        ));
        t.note(&format!(
            "due_with_no_gap={}",
            repo.claim_deleted_projects_for_check(0, 10)
                .await
                .unwrap()
                .len()
        ));
        t.note(&format!(
            "forget_inside_retention={}",
            repo.forget_deleted_projects(3600).await.unwrap()
        ));
        t.note(&format!(
            "forget_past_retention={}",
            repo.forget_deleted_projects(0).await.unwrap()
        ));
        t.note(&format!(
            "accepts_writes_when_absent={}",
            repo.project_accepts_writes(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "stale_after_delete={}",
            repo.get_stale_claimed_projects(0).await.unwrap().len()
        ));
        t
    })
    .await;
}
