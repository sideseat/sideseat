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

/// The leases that keep a restore repair and serving apart: live until they run out or end, by role, never the
/// asker's own.
#[tokio::test]
async fn process_leases_behave_identically() {
    use sideseat_ports::traits::ProcessRole;

    assert_parity("process_leases", |repo, mut t| async move {
        repo.renew_process_lease("server-a", ProcessRole::Server, 30)
            .await
            .unwrap();
        repo.renew_process_lease("server-b", ProcessRole::Server, -1)
            .await
            .unwrap();
        repo.renew_process_lease("repair-a", ProcessRole::RestoreRepair, 30)
            .await
            .unwrap();
        for (role, except) in [
            (ProcessRole::Server, ""),
            (ProcessRole::Server, "server-a"),
            (ProcessRole::RestoreRepair, ""),
        ] {
            t.note(&format!(
                "live_{}_except_{except}={:?}",
                role.as_str(),
                repo.live_process_leases(role, except).await.unwrap()
            ));
        }
        repo.renew_process_lease("server-b", ProcessRole::Server, 30)
            .await
            .unwrap();
        repo.end_process_lease("server-a").await.unwrap();
        repo.end_process_lease("not-held").await.unwrap();
        t.note(&format!(
            "live_after_renew_and_end={:?}",
            repo.live_process_leases(ProcessRole::Server, "")
                .await
                .unwrap()
        ));
        t
    })
    .await;
}

#[tokio::test]
async fn retention_cleanup_tokens_are_never_reused_identically() {
    assert_parity("retention_cleanup_tokens", |repo, mut t| async move {
        let project = ProjectId::from("p1");
        let t1 = ["t1".to_string()];

        // Completed, recorded again: the old token completes nothing.
        let first = repo
            .record_retention_cleanup(&project, &t1)
            .await
            .expect("record");
        repo.complete_retention_cleanup(&project, &first)
            .await
            .expect("complete");
        let second = repo
            .record_retention_cleanup(&project, &t1)
            .await
            .expect("record again");
        t.note(&format!("a fresh token: {}", first != second));
        repo.complete_retention_cleanup(&project, &first)
            .await
            .expect("stale completion");
        let owed = repo.claim_retention_cleanup(10, 0).await.expect("claim");
        t.note(&format!(
            "the later intent survives the stale completion: {}",
            owed.iter()
                .any(|(_, trace, token)| trace == "t1" && *token == second[0].1)
        ));

        // Unique across traces and projects.
        let mut tokens: Vec<i64> = second.iter().map(|(_, token)| *token).collect();
        for project in ["p1", "p2"] {
            let written = repo
                .record_retention_cleanup(
                    &ProjectId::from(project),
                    &["t2".to_string(), "t3".to_string()],
                )
                .await
                .expect("record more");
            tokens.extend(written.iter().map(|(_, token)| *token));
        }
        let distinct: std::collections::HashSet<i64> = tokens.iter().copied().collect();
        t.note(&format!(
            "every token distinct: {}",
            distinct.len() == tokens.len()
        ));
        t
    })
    .await;
}

/// Records and completions of overlapping traces in opposite orders, many at once, on PostgreSQL: both lock the
/// cleanup rows in trace order, so none deadlocks and every one succeeds.
#[tokio::test]
async fn retention_cleanup_records_and_completions_do_not_deadlock() {
    let Some((_, postgres)) = pair().await else {
        return;
    };
    let repo = std::sync::Arc::new(postgres);
    let project = ProjectId::from("p1");
    let mut tasks = Vec::new();
    for worker in 0..8 {
        let (repo, project) = (repo.clone(), project.clone());
        tasks.push(tokio::spawn(async move {
            for round in 0..25 {
                let mut traces = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
                if (worker + round) % 2 == 1 {
                    traces.reverse();
                }
                let recorded = repo
                    .record_retention_cleanup(&project, &traces)
                    .await
                    .expect("record");
                let mut completing = recorded;
                if worker % 2 == 1 {
                    completing.reverse();
                }
                repo.complete_retention_cleanup(&project, &completing)
                    .await
                    .expect("complete");
            }
        }));
    }
    for task in tasks {
        task.await.expect("no worker failed");
    }
}

/// What the confirm stamp costs a batch's confirm on PostgreSQL, as an alternating A/B in one run over the service's
/// own pool, so load from outside the process falls on both alike.
///
/// `cargo test -p sideseat-server --test postgres_parity confirm_stamp_bench -- --ignored --nocapture`, with
/// `SIDESEAT_TEST_POSTGRES_URL` set. A is the repository's confirm as it was before stamps, reproduced exactly:
/// grouped by project, one tenant transaction each, deduplicated, one `UPDATE` over the batch. B is the
/// repository's confirm, which also draws a sequence value and applies the larger of each row's stamp and it.
#[tokio::test]
#[ignore = "a measurement, not a check: run it on purpose"]
async fn confirm_stamp_bench() {
    const BATCHES: usize = 400;
    const PER_BATCH: usize = 8;
    let Some((_sqlite, postgres)) = pair().await else {
        return;
    };
    let project = ProjectId::from("default");
    // HEAD's repository confirm and its statement, wrapper and all: grouped by project, one tenant transaction each.
    let unstamped = |associations: Vec<(String, String, String)>| {
        let service = postgres.0.clone();
        async move {
            let mut by_project =
                std::collections::BTreeMap::<&str, Vec<(String, String, String)>>::new();
            for association in &associations {
                by_project
                    .entry(association.0.as_str())
                    .or_default()
                    .push(association.clone());
            }
            let mut confirmed = 0;
            for (project_id, project_associations) in by_project {
                let project_id = ProjectId::from(project_id);
                let mut unique: Vec<&(String, String, String)> =
                    project_associations.iter().collect();
                unique.sort_unstable();
                unique.dedup();
                let projects: Vec<&str> = unique.iter().map(|(p, _, _)| p.as_str()).collect();
                let traces: Vec<&str> = unique.iter().map(|(_, t, _)| t.as_str()).collect();
                let hashes: Vec<&str> = unique.iter().map(|(_, _, h)| h.as_str()).collect();
                let mut transaction = service.tenant_transaction(&project_id).await.unwrap();
                let result = sqlx::query(
                    "UPDATE trace_files SET durable = TRUE, pending_writers = GREATEST(pending_writers - 1, 0) \
                     WHERE (project_id, trace_id, file_hash) IN ( \
                         SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[]) \
                     )",
                )
                .bind(&projects)
                .bind(&traces)
                .bind(&hashes)
                .execute(&mut *transaction)
                .await
                .unwrap();
                transaction.commit().await.unwrap();
                confirmed += result.rows_affected();
            }
            confirmed
        }
    };
    let mut a = Vec::new();
    let mut b = Vec::new();
    for batch in 0..BATCHES {
        let associations: Vec<(String, String, String)> = (0..PER_BATCH)
            .map(|i| {
                (
                    "default".to_string(),
                    format!("bench-{batch}"),
                    format!("{:064x}", batch * PER_BATCH + i),
                )
            })
            .collect();
        for (_, trace_id, file_hash) in &associations {
            postgres
                .associate_file(trace_id, &project, file_hash, None, 5, "sha256")
                .await
                .unwrap();
        }
        let stamped = batch % 2 == 1;
        let started = std::time::Instant::now();
        let confirmed = if stamped {
            postgres
                .confirm_trace_file_associations(&associations)
                .await
                .unwrap()
        } else {
            unstamped(associations).await
        };
        let elapsed = started.elapsed();
        assert_eq!(
            confirmed, PER_BATCH as u64,
            "both arms confirm the whole batch"
        );
        if stamped {
            b.push(elapsed)
        } else {
            a.push(elapsed)
        }
    }
    let summary = |samples: &mut Vec<std::time::Duration>| {
        samples.sort();
        let micros = |d: std::time::Duration| d.as_secs_f64() * 1e6;
        (
            micros(samples[samples.len() / 2]),
            micros(samples[samples.len() * 9 / 10]),
        )
    };
    let (am, ap) = summary(&mut a);
    let (bm, bp) = summary(&mut b);
    eprintln!(
        "PostgreSQL confirm of {PER_BATCH} associations, {} batches each: unstamped median {am:.0} us p90 {ap:.0} us; \
         stamped median {bm:.0} us p90 {bp:.0} us; stamped / unstamped median {:.3}",
        a.len(),
        bm / am
    );
}
