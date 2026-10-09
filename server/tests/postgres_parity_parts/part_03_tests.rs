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
                    &ProjectId::from(project),
                    &file_hash,
                    Some("image/png"),
                    10,
                    "sha256",
                )
                .await
                .expect("associate");
                repo.sync_ref_count(&ProjectId::from(project), &file_hash)
                    .await
                    .expect("sync");
            }
            let counts = repo
                .get_file(&ProjectId::from(project), &file_hash)
                .await
                .expect("file row");
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
                .release_trace_file_association(&ProjectId::from(project), "t-shared", &file_hash)
                .await
                .expect("release");
            repo.sync_ref_count(&ProjectId::from(project), &file_hash)
                .await
                .expect("sync");
            t.note(&format!("peer release deleted the row={released}"));

            let counts = repo
                .get_file(&ProjectId::from(project), &file_hash)
                .await
                .expect("file row");
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
        repo.record_deleted_traces(
            &ProjectId::from("p1"),
            &["t1".to_string(), "t2".to_string()],
        )
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
            repo.record_deleted_trace_check(
                &ProjectId::from(project_id),
                trace_id,
                *token,
                true,
                60,
                3600,
            )
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
        repo.record_deleted_trace_check(
            &ProjectId::from(project_id),
            trace_id,
            token - 1,
            false,
            0,
            3600,
        )
        .await
        .expect("stale report must not error");
        let after_stale = repo
            .claim_deleted_traces_for_check(300, 10)
            .await
            .expect("claim after a stale report");
        t.note(&format!("claim after stale report={}", after_stale.len()));

        // A report with the right token and nothing-was-quiet brings it due again at the base interval.
        repo.record_deleted_trace_check(
            &ProjectId::from(project_id),
            trace_id,
            *token,
            false,
            0,
            3600,
        )
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

/// A server whose commits return before they are flushed is refused at startup, naming the setting: an OTLP 200
/// would rest on a transaction a crash can lose. `synchronous_commit = off` set on the database - a default the
/// server's own configuration does not show - and `fsync = off` on the server are each refused, and the same
/// server is accepted once they are restored. The suite runs one scenario at a time, so changing them is safe.
#[tokio::test]
async fn a_server_that_does_not_flush_its_commits_is_refused() {
    let url = match std::env::var(URL_ENV) {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("skipping PostgreSQL commit durability: {URL_ENV} is not set");
            return;
        }
    };
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect as the owner");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&admin)
        .await
        .expect("database");
    let config = PostgresConfig {
        url: url.clone(),
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let clock = || -> Arc<dyn sideseat_ports::clock::Clock> {
        Arc::new(sideseat_server::runtime::clock::SystemClock)
    };
    let scenarios = [
        (
            format!("ALTER DATABASE \"{database}\" SET synchronous_commit = off"),
            format!("ALTER DATABASE \"{database}\" RESET synchronous_commit"),
            "synchronous_commit = off",
            "synchronous_commit",
        ),
        (
            "ALTER SYSTEM SET fsync = off".to_string(),
            "ALTER SYSTEM RESET fsync".to_string(),
            "fsync = off",
            "fsync",
        ),
    ];
    for (set, reset, named, setting) in scenarios {
        sqlx::raw_sql(&set)
            .execute(&admin)
            .await
            .expect("misconfigure");
        sqlx::query("SELECT pg_reload_conf()")
            .execute(&admin)
            .await
            .expect("reload");
        // A reload is asynchronous: wait until a new session sees the setting.
        for _ in 0..50 {
            let seen: String = sqlx::query_scalar(&format!("SELECT current_setting('{setting}')"))
                .fetch_one(
                    &sqlx::postgres::PgPoolOptions::new()
                        .max_connections(1)
                        .connect(&url)
                        .await
                        .expect("a new session"),
                )
                .await
                .expect("setting");
            if seen == "off" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let refused = PostgresService::init(&config, clock()).await;
        // Restored before anything is asserted, so a failure here cannot leave the server misconfigured.
        sqlx::raw_sql(&reset)
            .execute(&admin)
            .await
            .expect("restore");
        sqlx::query("SELECT pg_reload_conf()")
            .execute(&admin)
            .await
            .expect("reload");
        let error = match refused {
            Ok(_) => panic!("a server with {named} was accepted"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(named), "{error}");
    }
    for _ in 0..50 {
        if PostgresService::init(&config, clock()).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the restored server was still refused");
}
