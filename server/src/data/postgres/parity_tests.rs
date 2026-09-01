//! PostgreSQL/SQLite transactional parity.
//!
//! The two transactional backends implement the same `TransactionalRepository` over hand-written SQL
//! in two dialects, and until this file existed the PostgreSQL half had never run against a
//! PostgreSQL server anywhere - `postgres/mod.rs` held an empty `mod tests` saying so. Every one of
//! its seventy-five methods was kept correct by review alone, which is exactly the situation the
//! ClickHouse parity suite was written for after review turned out not to catch a query that is
//! merely *accepted* while answering differently.
//!
//! The failure modes are the same shape as ClickHouse's, and some are specific to this pair:
//!
//! - `?` versus `$n` placeholders: a miscounted or reordered `$n` binds the wrong column, which
//!   still executes.
//! - SQLite's `INTEGER` is PostgreSQL's `BIGINT`, and its dynamic typing forgives a comparison that
//!   PostgreSQL rejects or coerces.
//! - `INSERT OR REPLACE` versus `ON CONFLICT DO UPDATE`: the SQLite form deletes and reinserts, so
//!   it resets columns the PostgreSQL form leaves alone.
//! - `ON DELETE CASCADE` reaches different rows in the two schemas whenever a foreign key was
//!   declared in one and forgotten in the other.
//! - Concurrency, which single-writer SQLite cannot express at all: PostgreSQL runs two writers, so
//!   a read-then-write that is safe under SQLite's busy error is a lost update here. That is a real
//!   defect this suite found in review and now guards - see [`the_file_fence_holds_against_a_
//!   concurrent_association`].
//!
//! # How parity is stated
//!
//! A scenario is a program run against a repository; its **transcript** is the sequence of
//! observations it made. Parity is transcript equality. Generated ids are replaced by stable labels
//! on first sight, and timestamps by the fact that they exist, so the comparison is about behaviour
//! rather than about clock values or cuid2 output. SQLite is the reference: it is the default
//! backend, and its behaviour is what the goldens and the UI were built against.
//!
//! # Not covered, worth knowing before trusting a green run
//!
//! - Read paths that only differ at scale: pagination past the first page with many rows, and the
//!   ordering of rows sharing a `created_at` second.
//! - The credential secret path (encryption lives above the repository) and OAuth auth-method
//!   lookups.
//! - Connection-pool behaviour: statement timeout, acquire timeout, `max_lifetime` recycling.
//! - Migration *upgrade* paths. The suite runs the current schema; it does not create a v1 database
//!   and walk it forward, so an `ALTER TABLE` arm that is wrong is only caught if the resulting
//!   shape differs from what fresh `SCHEMA` produces.
//! - Anything the transactional repository does not own: analytics rows, file bytes.
//!
//! Needs a live PostgreSQL. Skips with a message when `SIDESEAT_TEST_POSTGRES_URL` is unset, so
//! `cargo test` stays green on a checkout with no container:
//!
//! ```bash
//! make test-postgres     # starts a container, runs this, removes it
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::config::PostgresConfig;
use crate::data::TransactionalService;
use crate::data::postgres::PostgresService;
use crate::data::sqlite::SqliteService;
use crate::data::traits::TransactionalRepository;
use crate::data::types::LastOwnerResult;

/// Env var holding a PostgreSQL connection URL, e.g. `postgres://user:pass@127.0.0.1:5433/sideseat`.
const URL_ENV: &str = "SIDESEAT_TEST_POSTGRES_URL";

/// Tables the suite empties between scenarios, children before parents.
///
/// The seeded `default` org, user and project are restored afterwards, because most of the API
/// assumes they exist and the goldens' project id is `default`.
const DATA_TABLES: &[&str] = &[
    "deleted_traces",
    "credential_project_permissions",
    "credentials",
    "api_keys",
    "favorites",
    "trace_files",
    "files",
    "projects",
    "auth_methods",
    "organization_members",
    "users",
    "organizations",
];

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
async fn pair() -> Option<(Arc<SqliteService>, Arc<PostgresService>)> {
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
    sqlx::raw_sql(crate::data::sqlite::schema::SCHEMA)
        .execute(&sqlite_pool)
        .await
        .expect("SQLite schema");
    let sqlite = Arc::new(SqliteService::from_pool(sqlite_pool));

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
        PostgresService::init(&config)
            .await
            .expect("PostgreSQL connection (is the container up?)"),
    );
    reset_postgres(&postgres).await;

    Some((sqlite, postgres))
}

/// The two repositories behind the shared trait, in reference-then-candidate order.
fn repositories(
    sqlite: Arc<SqliteService>,
    postgres: Arc<PostgresService>,
) -> [Box<dyn TransactionalRepository + Send + Sync>; 2] {
    [
        TransactionalService::Sqlite(sqlite).repository(),
        TransactionalService::Postgres(postgres).repository(),
    ]
}

/// Empty the PostgreSQL data tables and restore the seeded rows.
///
/// The container is reused across scenarios, and `--test-threads=1` is what makes that safe; the
/// `make` target passes it for the same reason the ClickHouse one does.
async fn reset_postgres(service: &PostgresService) {
    for table in DATA_TABLES {
        sqlx::query(&format!("DELETE FROM {table}"))
            .execute(service.pool())
            .await
            .unwrap_or_else(|e| panic!("clear {table}: {e}"));
    }
    sqlx::raw_sql(crate::data::postgres::schema::DEFAULT_DATA)
        .execute(service.pool())
        .await
        .unwrap_or_else(|e| panic!("reseed: {e}"));
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

/// Projects, including the deletion fence: what a claimed project looks like to every read.
#[tokio::test]
async fn projects_behave_identically() {
    assert_parity("projects", |repo, mut t| async move {
        let alpha = repo
            .create_project(None, "default", "Alpha")
            .await
            .expect("create Alpha");
        let beta = repo
            .create_project(None, "default", "Beta")
            .await
            .expect("create Beta");
        t.note_id("created", &alpha.id);
        t.note_id("created", &beta.id);

        let (listed, total) = repo
            .list_projects_for_org(None, "default", 1, 50)
            .await
            .unwrap();
        t.note(&format!("listed_total={total}"));
        for project in &listed {
            let label = t.label(&project.id);
            t.note(&format!("listed={label} name={}", project.name));
        }

        let renamed = repo
            .update_project(None, &alpha.id, "Alpha Renamed")
            .await
            .unwrap();
        t.note(&format!(
            "renamed_to={:?}",
            renamed.as_ref().map(|p| p.name.clone())
        ));

        // The fence.
        t.note(&format!(
            "claim={}",
            repo.claim_project_for_deletion(None, &beta.id)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "claim_again={}",
            repo.claim_project_for_deletion(None, &beta.id)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "claim_missing={}",
            repo.claim_project_for_deletion(None, "no-such-project")
                .await
                .unwrap()
        ));
        t.note(&format!(
            "accepts_writes_while_claimed={}",
            repo.project_accepts_writes(&beta.id).await.unwrap()
        ));
        t.note(&format!(
            "get_claimed_is_none={}",
            repo.get_project(None, &beta.id).await.unwrap().is_none()
        ));
        let (after_claim, total_after) = repo
            .list_projects_for_org(None, "default", 1, 50)
            .await
            .unwrap();
        t.note(&format!(
            "listed_after_claim={} total={total_after}",
            after_claim.len()
        ));
        t.note(&format!(
            "rename_claimed_is_none={}",
            repo.update_project(None, &beta.id, "Nope")
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
            repo.add_member(None, "default", "local", "member")
                .await
                .is_err()
        ));
        t.note(&format!(
            "promote_in_deleting_org={}",
            describe(
                &repo
                    .update_role_atomic(None, "default", "local", "admin")
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "org_reads_absent={}",
            repo.get_organization(None, "default")
                .await
                .unwrap()
                .is_none()
        ));

        t.note(&format!(
            "deleted={}",
            repo.delete_project(None, &beta.id).await.unwrap()
        ));
        t.note(&format!(
            "delete_again={}",
            repo.delete_project(None, &beta.id).await.unwrap()
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
                repo.record_deleted_project_check(&id, token, true, 0, 0)
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

/// Files, their references and their deletion fence - the machinery a dangling reference comes from.
#[tokio::test]
async fn files_and_references_behave_identically() {
    assert_parity("files", |repo, mut t| async move {
        let a = hash(0xa1);
        let b = hash(0xb2);

        // Two traces referencing one file, and one referencing another.
        repo.associate_file("trace-1", "default", &a, Some("image/png"), 1024, "sha256")
            .await
            .unwrap();
        repo.associate_file("trace-2", "default", &a, Some("image/png"), 1024, "sha256")
            .await
            .unwrap();
        repo.associate_file("trace-2", "default", &b, None, 64, "sha256")
            .await
            .unwrap();
        // Idempotent: the same trace naming the same file twice is one reference.
        repo.associate_file("trace-1", "default", &a, Some("image/png"), 1024, "sha256")
            .await
            .unwrap();

        let file_a = repo.get_file("default", &a).await.unwrap();
        t.note(&format!(
            "a_ref_count={:?} size={:?} media={:?}",
            file_a.as_ref().map(|f| f.ref_count),
            file_a.as_ref().map(|f| f.size_bytes),
            file_a.as_ref().and_then(|f| f.media_type.clone())
        ));
        t.note(&format!(
            "exists_a={} exists_missing={}",
            repo.file_exists("default", &a).await.unwrap(),
            repo.file_exists("default", &hash(0xcc)).await.unwrap()
        ));
        t.note(&format!(
            "storage_bytes={}",
            repo.get_project_storage_bytes("default").await.unwrap()
        ));

        let mut hashes = repo
            .get_file_hashes_for_traces("default", &["trace-2".to_string()])
            .await
            .unwrap();
        hashes.sort();
        t.note(&format!("hashes_for_trace_2={}", hashes.len()));
        let mut counted = repo
            .get_file_reference_counts_for_traces("default", &["trace-1".to_string()])
            .await
            .unwrap();
        counted.sort();
        t.note(&format!(
            "counts_for_trace_1={:?}",
            counted.iter().map(|(_, n)| *n).collect::<Vec<_>>()
        ));

        // Deleting one trace releases only its own references.
        t.note(&format!(
            // Sorted: the set is what matters, and the two dialects return rows in their own order.
            "released={:?}",
            {
                let mut released = repo
                    .delete_trace_files("default", &["trace-1".to_string()])
                    .await
                    .unwrap();
                released.sort();
                released
            }
        ));
        t.note(&format!(
            "a_after_release={:?}",
            repo.get_file("default", &a)
                .await
                .unwrap()
                .map(|f| f.ref_count)
        ));

        // The fence: claiming, refusing an association through it, releasing.
        t.note(&format!(
            "claim_referenced={}",
            repo.claim_file_for_deletion("default", &a).await.unwrap()
        ));
        repo.delete_trace_files("default", &["trace-2".to_string()])
            .await
            .unwrap();
        t.note(&format!(
            "claim_unreferenced={}",
            repo.claim_file_for_deletion("default", &a).await.unwrap()
        ));
        t.note(&format!(
            "claim_again={}",
            repo.claim_file_for_deletion("default", &a).await.unwrap()
        ));
        t.note(&format!(
            "associate_through_fence_is_err={}",
            repo.associate_file("trace-3", "default", &a, None, 1024, "sha256")
                .await
                .is_err()
        ));
        t.note(&format!(
            "stale_at_zero={}",
            repo.get_stale_claimed_files(0).await.unwrap().len()
        ));
        t.note(&format!(
            "stale_at_a_day={}",
            repo.get_stale_claimed_files(86_400).await.unwrap().len()
        ));
        t.note(&format!(
            "delete_if_unreferenced={}",
            repo.delete_file_if_unreferenced("default", &a)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "gone={}",
            repo.get_file("default", &a).await.unwrap().is_none()
        ));

        // The release path, on the file that is still there.
        t.note(&format!(
            "claim_b={}",
            repo.claim_file_for_deletion("default", &b).await.unwrap()
        ));
        repo.release_deletion_claim("default", &b).await.unwrap();
        t.note(&format!(
            "associate_after_release_ok={}",
            repo.associate_file("trace-4", "default", &b, None, 64, "sha256")
                .await
                .is_ok()
        ));

        // A count that drifted, recomputed from the associations that exist.
        repo.decrement_ref_count("default", &b).await.unwrap();
        repo.decrement_ref_count("default", &b).await.unwrap();
        t.note(&format!(
            "b_after_two_decrements={:?}",
            repo.get_file("default", &b)
                .await
                .unwrap()
                .map(|f| f.ref_count)
        ));
        t.note(&format!(
            "synced={:?}",
            repo.sync_ref_count("default", &b).await.unwrap()
        ));
        t.note(&format!(
            "b_after_sync={:?}",
            repo.get_file("default", &b)
                .await
                .unwrap()
                .map(|f| f.ref_count)
        ));
        let mut orphans = repo.get_orphan_files().await.unwrap();
        orphans.sort();
        t.note(&format!("orphans={}", orphans.len()));
        t
    })
    .await;
}

/// An organization and its members: roles, the atomic updates, and what deleting it reaches.
#[tokio::test]
async fn organizations_and_members_behave_identically() {
    assert_parity("orgs", |repo, mut t| async move {
        let owner = repo
            .create_user(None, "owner@example.com", Some("Owner"))
            .await
            .expect("create owner");
        let member = repo
            .create_user(None, "member@example.com", Some("Member"))
            .await
            .expect("create member");
        t.note_id("owner", &owner.id);
        t.note_id("member", &member.id);

        let org = repo
            .create_organization_with_owner(None, "Acme", "acme", &owner.id)
            .await
            .expect("create org");
        t.note_id("org", &org.id);
        let project = repo
            .create_project(None, &org.id, "Acme Project")
            .await
            .unwrap();
        t.note_id("project", &project.id);

        repo.add_member(None, &org.id, &member.id, "member")
            .await
            .unwrap();
        let (mut members, total) = repo.list_members(&org.id, 1, 50).await.unwrap();
        members.sort_by(|a, b| a.role.cmp(&b.role));
        t.note(&format!("members={} total={total}", members.len()));
        for m in &members {
            t.note(&format!("member_role={}", m.role));
        }
        t.note(&format!(
            "membership_role={:?}",
            repo.get_membership(None, &org.id, &member.id)
                .await
                .unwrap()
                .map(|m| m.role)
        ));

        // Atomic role change and removal: the last owner must not be demoted or removed.
        fn describe<T>(outcome: &LastOwnerResult<T>) -> &'static str {
            match outcome {
                LastOwnerResult::Success(_) => "success",
                LastOwnerResult::LastOwner => "last_owner",
                LastOwnerResult::NotFound => "not_found",
            }
        }
        t.note(&format!(
            "promote={}",
            describe(
                &repo
                    .update_role_atomic(None, &org.id, &member.id, "admin")
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "demote_last_owner={}",
            describe(
                &repo
                    .update_role_atomic(None, &org.id, &owner.id, "member")
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "remove_last_owner={}",
            describe(
                &repo
                    .remove_member_atomic(None, &org.id, &owner.id)
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "remove_member={}",
            describe(
                &repo
                    .remove_member_atomic(None, &org.id, &member.id)
                    .await
                    .unwrap()
            )
        ));

        let mut ids = repo.list_project_ids(&org.id).await.unwrap();
        ids.sort();
        t.note(&format!("project_ids={}", ids.len()));

        let (for_user, _) = repo
            .list_projects_for_user(None, &owner.id, 1, 50)
            .await
            .unwrap();
        t.note(&format!("projects_for_owner={}", for_user.len()));

        // Deleting the org must take its projects with it, in both schemas.
        t.note(&format!(
            "org_deleted={}",
            repo.delete_organization(None, &org.id).await.unwrap()
        ));
        t.note(&format!(
            "project_gone={}",
            repo.get_project(None, &project.id).await.unwrap().is_none()
        ));
        t.note(&format!(
            "membership_gone={}",
            repo.get_membership(None, &org.id, &owner.id)
                .await
                .unwrap()
                .is_none()
        ));
        t
    })
    .await;
}

// ============================================================================
// Concurrency: what SQLite cannot express
// ============================================================================

/// The file fence must survive two writers, at the one interleaving that breaks it.
///
/// Launching two tasks and hoping proves nothing - they serialise, and the test passes against the very
/// code it is meant to catch (verified: it did). So the interleaving is pinned by holding a transaction
/// open at the point where the other writer must be blocked.
///
/// The hazard is specific to PostgreSQL under READ COMMITTED. An `UPDATE` that blocks on a locked row
/// re-checks its qualification when the lock frees - but a subquery in that qualification is evaluated
/// against the *statement's original* snapshot. So a claim written as one `UPDATE ... WHERE NOT EXISTS
/// (SELECT 1 FROM trace_files ...)` cannot see the association that committed while it waited: it claims
/// a file that is now referenced, and cleanup deletes bytes a committed row points at. The span then
/// renders as broken content and nothing on it says why.
///
/// SQLite cannot express this at all - one writer, and a read-then-write transaction fails with a busy
/// error - which is why the defect lived in the PostgreSQL half alone.
#[tokio::test]
async fn the_file_fence_holds_against_a_concurrent_association() {
    let Some((_, postgres)) = pair().await else {
        return;
    };
    let file = hash(0x5c);

    // A file that exists and is unreferenced: claimable, and associable.
    postgres
        .associate_file("old-trace", "default", &file, None, 128, "sha256")
        .await
        .unwrap();
    postgres
        .delete_trace_files("default", &["old-trace".to_string()])
        .await
        .unwrap();

    // Writer one: an association, up to the point of committing. These statements mirror
    // `associate_file`'s order - lock the file row, then insert the reference - because what is being
    // tested is the fence's behaviour against *a reference committed while the claim waited*, whatever
    // the code that commits it looks like.
    let mut association = postgres.pool().begin().await.unwrap();
    let _: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT deleting_at FROM files WHERE project_id = $1 AND file_hash = $2 FOR UPDATE",
    )
    .bind("default")
    .bind(&file)
    .fetch_optional(&mut *association)
    .await
    .unwrap();
    sqlx::query("INSERT INTO trace_files (trace_id, project_id, file_hash) VALUES ($1, $2, $3)")
        .bind("new-trace")
        .bind("default")
        .bind(&file)
        .execute(&mut *association)
        .await
        .unwrap();

    // Writer two: the claim. It must block on the row lock writer one holds.
    let claim = {
        let repo = Arc::clone(&postgres);
        let file = file.clone();
        tokio::spawn(async move { repo.claim_file_for_deletion("default", &file).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !claim.is_finished(),
        "the claim answered without waiting for the row lock, so this test is not exercising the \
         interleaving it exists for"
    );

    // The reference becomes visible. The claim's wait ends here.
    association.commit().await.unwrap();
    let claimed = claim.await.unwrap().unwrap();

    assert!(
        !claimed,
        "a file with a committed reference was claimed for deletion, so cleanup is about to delete \
         bytes that a span still points at"
    );
    assert_eq!(
        postgres.sync_ref_count("default", &file).await.unwrap(),
        Some(1),
        "and the reference that won is the one counted"
    );
}

/// A project cannot be created under an organization whose deletion has committed.
///
/// PostgreSQL-specific, and it is why the check is a locking read rather than a `WHERE` clause on the
/// insert. `INSERT ... SELECT ... WHERE deleting_at IS NULL` reads under its own snapshot, and the claim
/// updates only a non-key column - so its row lock is `FOR NO KEY UPDATE`, which is *compatible* with the
/// key-share lock a foreign-key insert takes. The insert would neither block nor see the tombstone, and a
/// brand-new live project would appear under an organization whose cleanup had already listed its
/// projects: the caller gets 201 and then 404, and an ingest that passed the project fence in between
/// leaves data with no row to find it by.
///
/// The interleaving is pinned rather than hoped for: the creation is started while a transaction holds the
/// organization locked, so it must block, and the tombstone commits before it is released.
#[tokio::test]
async fn a_project_cannot_be_created_under_a_deleting_organization() {
    let Some((_, postgres)) = pair().await else {
        return;
    };

    // Writer one: take the organization's row lock, as the claim does, and hold it.
    let mut claim = postgres.pool().begin().await.unwrap();
    let _: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = $1 FOR UPDATE")
            .bind("default")
            .fetch_optional(&mut *claim)
            .await
            .unwrap();
    sqlx::query("UPDATE organizations SET deleting_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().timestamp())
        .bind("default")
        .execute(&mut *claim)
        .await
        .unwrap();

    // Writer two: a creation that must wait for that lock.
    let creation = {
        let repo = Arc::clone(&postgres);
        tokio::spawn(async move { repo.create_project(None, "default", "Sneaky").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !creation.is_finished(),
        "the creation answered without waiting for the organization's row, so it is reading a snapshot \
         and this test is not exercising the interleaving it exists for"
    );

    claim.commit().await.unwrap();
    let outcome = creation.await.unwrap();
    assert!(
        outcome.is_err(),
        "a project was created under an organization whose deletion had committed; it would be live, \
         accept writes, and be cascaded away without leaving a deletion record"
    );
}

/// A member cannot be added while the organization's deletion commits underneath.
///
/// The sequential case - mutating an already-tombstoned organization - was fixed first, and it is the
/// easier half. This is the race: the mutation reads a live organization, the deletion commits, and the
/// mutation writes anyway, because the parent row still exists and the foreign key is satisfied. The caller
/// is told success for a membership in an organization no read can see and the cascade is about to remove.
///
/// The interleaving is pinned rather than hoped for, as the project-creation test does it: the mutation is
/// started while a transaction holds the organization's row locked, so it must block, and the tombstone
/// commits before the lock is released.
#[tokio::test]
async fn a_member_cannot_be_added_while_the_organization_is_being_deleted() {
    let Some((_, postgres)) = pair().await else {
        return;
    };

    let mut claim = postgres.pool().begin().await.unwrap();
    let _: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = $1 FOR UPDATE")
            .bind("default")
            .fetch_optional(&mut *claim)
            .await
            .unwrap();
    sqlx::query("UPDATE organizations SET deleting_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().timestamp())
        .bind("default")
        .execute(&mut *claim)
        .await
        .unwrap();

    let user = postgres
        .create_user(None, "joiner@example.com", Some("Joiner"))
        .await
        .expect("create user");
    let addition = {
        let repo = Arc::clone(&postgres);
        let user_id = user.id.clone();
        tokio::spawn(async move { repo.add_member(None, "default", &user_id, "member").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !addition.is_finished(),
        "the mutation answered without waiting for the organization's row, so its liveness check is \
         outside its transaction and this test is not exercising the interleaving it exists for"
    );

    claim.commit().await.unwrap();
    assert!(
        addition.await.unwrap().is_err(),
        "a member was added to an organization whose deletion had committed"
    );
}

/// Two replicas sweeping at once cannot claim the same deleted project.
///
/// `WHERE project_id IN (SELECT ... LIMIT n)` is not enough on PostgreSQL, and the mechanism is the same
/// stale-subquery one as the file claim's: the subquery is evaluated against the statement's snapshot, so a
/// replica whose outer update blocks on a row another replica is updating resumes with a subquery result
/// that still lists it - and the outer condition only compares `project_id`, which has not changed. Both
/// replicas return the same id and both do the storage work, which for fifty S3 listings is exactly the
/// cost this scheduler exists to bound. `FOR UPDATE SKIP LOCKED` on the inner select is what makes the
/// claim exclusive.
///
/// The interleaving is pinned: a transaction holds the row locked while a second claim runs, so a claim
/// that respects the lock returns nothing and one that reads a stale snapshot returns the id.
#[tokio::test]
async fn two_replicas_cannot_claim_the_same_deleted_project() {
    let Some((_, postgres)) = pair().await else {
        return;
    };

    let project = postgres
        .create_project(None, "default", "Swept")
        .await
        .unwrap();
    postgres
        .claim_project_for_deletion(None, &project.id)
        .await
        .unwrap();
    postgres
        .record_project_sweep(&project.id, true, 1, 0)
        .await
        .unwrap();

    // Replica A: claim the row and hold it, as a sweep in progress does.
    let mut replica_a = postgres.pool().begin().await.unwrap();
    let held: Vec<(String,)> = sqlx::query_as(
        "UPDATE deleted_projects SET last_checked_at = extract(epoch from now())::bigint          WHERE project_id IN (              SELECT project_id FROM deleted_projects              WHERE next_check_at IS NULL OR next_check_at <= extract(epoch from now())::bigint              ORDER BY next_check_at LIMIT 10 FOR UPDATE SKIP LOCKED          ) RETURNING project_id",
    )
    .fetch_all(&mut *replica_a)
    .await
    .unwrap();
    assert_eq!(held.len(), 1, "replica A claims the only due id");

    // Replica B: the same claim, concurrently. It must find nothing rather than the same id.
    let claimed_by_b = postgres
        .claim_deleted_projects_for_check(600, 10)
        .await
        .unwrap();
    assert!(
        claimed_by_b.is_empty(),
        "two replicas claimed the same deleted project, so both will do its storage cleanup: {:?}",
        claimed_by_b
    );

    replica_a.commit().await.unwrap();
}

/// Two concurrent deletions of one project: exactly one owns it.
///
/// The claim is what makes a project's cleanup single-owner, and a cleanup that runs twice deletes
/// analytics rows and file bytes twice over while both callers report success to their user.
#[tokio::test]
async fn only_one_concurrent_project_claim_wins() {
    let Some((_, postgres)) = pair().await else {
        return;
    };
    let project = postgres
        .create_project(None, "default", "Contested")
        .await
        .unwrap();

    let mut winners = 0;
    let mut handles = Vec::new();
    for _ in 0..4 {
        let repo = Arc::clone(&postgres);
        let id = project.id.clone();
        handles.push(tokio::spawn(async move {
            repo.claim_project_for_deletion(None, &id).await
        }));
    }
    for handle in handles {
        if handle.await.unwrap().unwrap_or(false) {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one caller must own the deletion; {winners} did"
    );
}

/// A deleted trace stays deleted, even against an ingest that commits afterwards.
///
/// The race the tombstone closes: files and their associations are written *before* the analytics row
/// that references them, so a batch in flight when `delete_traces` runs commits after the deletion has
/// reclaimed the bytes - producing a span with a `#!B64!#` reference to nothing, for a trace the caller
/// was already told 204 for. Nothing bounds how late that commit is; a queued batch can be redelivered
/// minutes later.
///
/// Compared as a transcript because the two implementations are hand-written in different dialects (a
/// per-row loop against `UNNEST`, `IN (?, ?)` against `= ANY($2)`), and a disagreement here is either a
/// deleted trace resurrected on one backend or a live one dropped on the other.
#[tokio::test]
async fn trace_deletion_tombstones_behave_identically() {
    assert_parity("deleted_traces", |repo, mut t| async move {
        let asked = vec!["trace-a".to_string(), "trace-b".to_string()];
        // Sorted, because the answer is a set and the two dialects return rows in their own order.
        let show =
            |t: &mut Transcript,
             what: &str,
             found: Result<std::collections::HashSet<String>, crate::data::DataError>| {
                match found {
                    Ok(set) => {
                        let mut ids: Vec<String> = set.into_iter().collect();
                        ids.sort();
                        t.note(&format!("{what}={ids:?}"));
                    }
                    Err(e) => t.note(&format!("{what}=error({e})")),
                }
            };

        // Nothing deleted yet.
        let found = repo.deleted_traces_among("p1", &asked).await;
        show(&mut t, "empty", found);

        // One deleted, and only that one refused.
        let recorded = repo
            .record_deleted_traces("p1", &["trace-a".to_string()])
            .await;
        t.note(&format!("record a ok={}", recorded.is_ok()));
        let found = repo.deleted_traces_among("p1", &asked).await;
        show(&mut t, "after a", found);

        // The deletion route may be retried, so re-recording must not conflict.
        let again = repo
            .record_deleted_traces("p1", &["trace-a".to_string()])
            .await;
        t.note(&format!("record a again ok={}", again.is_ok()));
        let found = repo.deleted_traces_among("p1", &asked).await;
        show(&mut t, "after retry", found);

        // A trace id comes from the client, so the same id in another project is untouched.
        let found = repo.deleted_traces_among("p2", &asked).await;
        show(&mut t, "other project", found);

        // A batch of several, and an empty ask.
        let both = repo.record_deleted_traces("p1", &asked).await;
        t.note(&format!("record both ok={}", both.is_ok()));
        let found = repo.deleted_traces_among("p1", &asked).await;
        show(&mut t, "after both", found);
        let found = repo.deleted_traces_among("p1", &[]).await;
        show(&mut t, "empty ask", found);
        t
    })
    .await;
}

/// Releasing an association drops its file's reference count, so the orphan sweeper can reclaim it.
///
/// The leak this closes: a file's bytes and association are written *before* the analytics row that
/// references them - deliberately, so the surviving failure is a reclaimable orphan rather than a
/// dangling reference. But an association keeps `ref_count` above zero, and the orphan sweeper selects
/// on `ref_count = 0`, so a batch whose analytics write failed left the file holding the project's quota
/// with nothing able to reclaim it.
///
/// Two properties, both compared across the dialects: releasing the association this batch created drops
/// the count to zero and makes the file an orphan, and releasing does **not** touch another trace's
/// association for the same file - which is what stops the compensation from orphaning a committed
/// batch's content.
#[tokio::test]
async fn releasing_a_created_association_behaves_identically() {
    assert_parity("release association", |repo, mut t| async move {
        let project = "default";
        let file_hash = hash(0xc3);

        // Two traces share one file, which is the case that makes precision matter.
        for trace in ["t-keep", "t-drop"] {
            let created = repo
                .associate_file(trace, project, &file_hash, Some("image/png"), 10, "sha256")
                .await
                .expect("associate");
            t.note(&format!("associate {trace} new={created}"));
            repo.sync_ref_count(project, &file_hash)
                .await
                .expect("sync");
        }
        let counts = repo.get_file(project, &file_hash).await.expect("file row");
        t.note(&format!(
            "refs after two associations={:?}",
            counts.map(|f| f.ref_count)
        ));

        // Release only the one the failed batch created.
        let released = repo
            .release_trace_file_association(project, "t-drop", &file_hash)
            .await
            .expect("release");
        repo.sync_ref_count(project, &file_hash)
            .await
            .expect("sync");
        t.note(&format!("released={released}"));
        let counts = repo.get_file(project, &file_hash).await.expect("file row");
        t.note(&format!(
            "refs after release={:?}",
            counts.map(|f| f.ref_count)
        ));

        // The other trace still holds it, so it is not an orphan yet.
        let orphans = repo.get_orphan_files().await.expect("orphans");
        t.note(&format!(
            "orphan while still referenced={}",
            orphans.iter().any(|(p, h)| p == project && h == &file_hash)
        ));

        // Release the survivor too, and now it is reclaimable.
        repo.release_trace_file_association(project, "t-keep", &file_hash)
            .await
            .expect("release the other");
        repo.sync_ref_count(project, &file_hash)
            .await
            .expect("sync");
        let orphans = repo.get_orphan_files().await.expect("orphans");
        t.note(&format!(
            "orphan once unreferenced={}",
            orphans.iter().any(|(p, h)| p == project && h == &file_hash)
        ));

        // Releasing something that is not there is not an error - the failure path may run twice.
        let again = repo
            .release_trace_file_association(project, "t-drop", &file_hash)
            .await
            .expect("releasing twice must not error");
        t.note(&format!("release again={again}"));
        t
    })
    .await;
}

/// Two batches sharing one association: a commit by either survives the other's release - on both backends.
///
/// The concurrency case the pending-writer count exists for, and the orphan a boolean flag produced: two
/// batches reference the same `(project, trace, hash)`, one commits and one fails. The failing batch's
/// release must not delete the row the committed batch depends on. Recorded as a transcript so PostgreSQL
/// and SQLite are held to the identical sequence.
#[tokio::test]
async fn a_shared_association_survives_a_peer_release_identically() {
    assert_parity(
        "shared association survives peer release",
        |repo, mut t| async move {
            let project = "default";
            let file_hash = hash(0xd4);

            // Two batches reference the same association on the same trace: two in-flight writers, one row.
            for _ in 0..2 {
                repo.associate_file(
                    "t-shared",
                    project,
                    &file_hash,
                    Some("image/png"),
                    10,
                    "sha256",
                )
                .await
                .expect("associate");
                repo.sync_ref_count(project, &file_hash)
                    .await
                    .expect("sync");
            }
            let counts = repo.get_file(project, &file_hash).await.expect("file row");
            t.note(&format!(
                "refs after two batches={:?}",
                counts.map(|f| f.ref_count)
            ));

            // One batch commits (confirm), the other fails (release) - the interleaving that used to orphan it.
            repo.confirm_trace_file_associations(&[(
                project.to_string(),
                "t-shared".to_string(),
                file_hash.clone(),
            )])
            .await
            .expect("confirm");
            let released = repo
                .release_trace_file_association(project, "t-shared", &file_hash)
                .await
                .expect("release");
            repo.sync_ref_count(project, &file_hash)
                .await
                .expect("sync");
            t.note(&format!("peer release deleted the row={released}"));

            let counts = repo.get_file(project, &file_hash).await.expect("file row");
            t.note(&format!(
                "refs after the peer released={:?}",
                counts.map(|f| f.ref_count)
            ));
            let orphans = repo.get_orphan_files().await.expect("orphans");
            t.note(&format!(
                "orphaned despite a committed batch={}",
                orphans.iter().any(|(p, h)| p == project && h == &file_hash)
            ));
            t
        },
    )
    .await;
}

/// The deleted-trace sweep claims exclusively, leases, and backs off - identically on both backends.
///
/// This is the sweep that covers the one window a pre-write tombstone check cannot: the tombstone is a
/// row in this store and the spans go to the analytics store, so a crash between the check and the
/// post-write re-check leaves spans for a deleted trace. The properties that make the sweep bounded and
/// safe are all in SQL, written twice in two dialects (`unixepoch()` against `EXTRACT(EPOCH FROM now())`,
/// `1 << n` against `2::bigint ^ n`, and PostgreSQL's `FOR UPDATE SKIP LOCKED`), so a divergence here is
/// either a record swept twice at once or one that stops being swept at all.
#[tokio::test]
async fn the_deleted_trace_sweep_schedule_behaves_identically() {
    assert_parity("deleted trace sweep", |repo, mut t| async move {
        // A fresh tombstone is due immediately: `next_check_at` defaults to 0, and a record that queued
        // behind the backlog would hide its late spans for as long as it waited.
        repo.record_deleted_traces("p1", &["t1".to_string(), "t2".to_string()])
            .await
            .expect("record");
        let claimed = repo
            .claim_deleted_traces_for_check(300, 10)
            .await
            .expect("claim");
        let mut ids: Vec<String> = claimed.iter().map(|(_, t, _)| t.clone()).collect();
        ids.sort();
        t.note(&format!("first claim={ids:?}"));
        t.note(&format!(
            "tokens={:?}",
            claimed.iter().map(|(_, _, tok)| *tok).collect::<Vec<_>>()
        ));

        // Leased: the claim pushed the due time out, so an immediate second claim finds nothing. Without
        // this a batch of storage work outrunning one sweep interval would be re-claimed while it ran.
        let again = repo
            .claim_deleted_traces_for_check(300, 10)
            .await
            .expect("claim again");
        t.note(&format!("claim while leased={}", again.len()));

        // A quiet check backs off; anything found brings it back to the base interval. Reported against
        // the claim token, so a worker whose lease expired cannot overwrite the new holder's schedule.
        for (project_id, trace_id, token) in &claimed {
            repo.record_deleted_trace_check(project_id, trace_id, *token, true, 60, 3600)
                .await
                .expect("record quiet");
        }
        let after_quiet = repo
            .claim_deleted_traces_for_check(300, 10)
            .await
            .expect("claim after quiet");
        t.note(&format!("claim after backoff={}", after_quiet.len()));

        // A stale token changes nothing.
        let (project_id, trace_id, token) = &claimed[0];
        repo.record_deleted_trace_check(project_id, trace_id, token - 1, false, 0, 3600)
            .await
            .expect("stale report must not error");
        let after_stale = repo
            .claim_deleted_traces_for_check(300, 10)
            .await
            .expect("claim after a stale report");
        t.note(&format!("claim after stale report={}", after_stale.len()));

        // A report with the right token and nothing-was-quiet brings it due again at the base interval.
        repo.record_deleted_trace_check(project_id, trace_id, *token, false, 0, 3600)
            .await
            .expect("record found");
        let after_found = repo
            .claim_deleted_traces_for_check(300, 10)
            .await
            .expect("claim after finding something");
        let mut due: Vec<String> = after_found.iter().map(|(_, tr, _)| tr.clone()).collect();
        due.sort();
        t.note(&format!("due after finding something={due:?}"));
        t
    })
    .await;
}

/// Discarding a dropped trace's provisional association reclaims a stuck increment identically on both.
///
/// The crash-leak the discard path exists to close: a writer increments `pending_writers` and crashes, a
/// redelivery drops the trace, and a decrement-release leaves the count stuck above zero forever. Discard
/// deletes the non-durable row outright; a durable row is kept. Both backends must agree.
#[tokio::test]
async fn discarding_a_dropped_provisional_association_behaves_identically() {
    assert_parity("discard dropped provisional", |repo, mut t| async move {
        let project = "default";
        let hash = hash(0xe5);

        // Two increments, no confirm (a crashed writer plus a redelivery): pending high, not durable.
        for _ in 0..2 {
            repo.associate_file("t-drop", project, &hash, Some("image/png"), 10, "sha256")
                .await
                .expect("associate");
            repo.sync_ref_count(project, &hash).await.expect("sync");
        }
        let deleted = repo
            .discard_provisional_trace_file_association(project, "t-drop", &hash)
            .await
            .expect("discard");
        repo.sync_ref_count(project, &hash).await.expect("sync");
        t.note(&format!("discard deleted the provisional row={deleted}"));
        let counts = repo.get_file(project, &hash).await.expect("file row");
        t.note(&format!(
            "refs after discard={:?}",
            counts.map(|f| f.ref_count)
        ));

        // A durable association must survive discard.
        repo.associate_file("t-keep", project, &hash, Some("image/png"), 10, "sha256")
            .await
            .expect("associate keep");
        repo.confirm_trace_file_associations(&[(
            project.to_string(),
            "t-keep".to_string(),
            hash.clone(),
        )])
        .await
        .expect("confirm");
        let kept = repo
            .discard_provisional_trace_file_association(project, "t-keep", &hash)
            .await
            .expect("discard durable");
        t.note(&format!("discard removed the durable row={kept}"));
        t
    })
    .await;
}
