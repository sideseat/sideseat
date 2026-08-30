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
    sqlx::query(crate::data::sqlite::schema::SCHEMA)
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

        t.note(&format!(
            "deleted={}",
            repo.delete_project(None, &beta.id).await.unwrap()
        ));
        t.note(&format!(
            "delete_again={}",
            repo.delete_project(None, &beta.id).await.unwrap()
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
            "released={}",
            repo.delete_trace_files("default", &["trace-1".to_string()])
                .await
                .unwrap()
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
