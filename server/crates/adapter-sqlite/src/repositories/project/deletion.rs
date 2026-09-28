use super::*;

pub async fn record_deleted_traces(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
    now: i64,
) -> Result<(), SqliteError> {
    if trace_ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    insert_trace_tombstones(&mut tx, project_id, trace_ids, now).await?;
    tx.commit().await?;
    Ok(())
}

/// The tombstone rows, inside a transaction the caller owns. One statement shared by the standalone form above
/// and the journalled one, so the two cannot drift about what a tombstone is.
async fn insert_trace_tombstones(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    trace_ids: &[String],
    now: i64,
) -> Result<(), SqliteError> {
    for trace_id in trace_ids {
        sqlx::query(
            "INSERT INTO deleted_traces (project_id, trace_id, deleted_at) VALUES (?, ?, ?)
             ON CONFLICT(project_id, trace_id) DO NOTHING",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Which of these traces are tombstoned.
pub async fn deleted_traces_among(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<std::collections::HashSet<String>, SqliteError> {
    if trace_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    // One statement for the batch: a batch carries a handful of traces and this is on the ingestion hot
    // path, so a query per trace would multiply the round trips by the batch's trace count.
    let placeholders = vec!["?"; trace_ids.len()].join(", ");
    let sql = format!(
        "SELECT trace_id FROM deleted_traces WHERE project_id = ? AND trace_id IN ({placeholders})"
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql).bind(project_id);
    for trace_id in trace_ids {
        query = query.bind(trace_id);
    }
    Ok(query.fetch_all(pool).await?.into_iter().collect())
}

/// Claim a batch of deleted traces whose check is due, leased and exclusive.
///
/// The same shape as the project claim, for the reason stated on the trait method: the pre-write tombstone
/// check and the analytics write are in different stores, so a crash between them leaves spans for a
/// deleted trace and only a sweep collects them. One statement, so a claim is atomic with its lease.
pub async fn claim_deleted_traces_for_check(
    pool: &SqlitePool,
    lease_secs: i64,
    limit: i64,
) -> Result<Vec<(String, String, i64)>, SqliteError> {
    let mut rows: Vec<(String, String, i64)> = sqlx::query_as(
        "UPDATE deleted_traces \
         SET next_check_at = unixepoch() + ?, claim_token = claim_token + 1 \
         WHERE (project_id, trace_id) IN ( \
             SELECT project_id, trace_id FROM deleted_traces \
             WHERE next_check_at <= unixepoch() \
             ORDER BY next_check_at, project_id, trace_id \
             LIMIT ? \
         ) \
         RETURNING project_id, trace_id, claim_token",
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.sort_unstable_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    Ok(rows)
}

/// Record what a deleted trace's check found, matched on the claim token.
pub async fn record_deleted_trace_check(
    pool: &SqlitePool,
    project_id: &str,
    trace_id: &str,
    claim_token: i64,
    was_quiet: bool,
    base_gap_secs: i64,
    max_gap_secs: i64,
) -> Result<(), SqliteError> {
    if was_quiet {
        sqlx::query(
            "UPDATE deleted_traces \
             SET quiet_checks = quiet_checks + 1, \
                 next_check_at = unixepoch() + MIN(? * (1 << MIN(quiet_checks, 20)), ?) \
             WHERE project_id = ? AND trace_id = ? AND claim_token = ?",
        )
        .bind(base_gap_secs)
        .bind(max_gap_secs)
        .bind(project_id)
        .bind(trace_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE deleted_traces SET quiet_checks = 0, next_check_at = unixepoch() + ? \
             WHERE project_id = ? AND trace_id = ? AND claim_token = ?",
        )
        .bind(base_gap_secs)
        .bind(project_id)
        .bind(trace_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Claim a batch of deleted sessions whose check is due, leased and exclusive.
///
/// The same shape as the project claim, for the reason stated on the trait method: the pre-write tombstone
/// check and the analytics write are in different stores, so a crash between them leaves spans for a
/// deleted trace and only a sweep collects them. One statement, so a claim is atomic with its lease.
pub async fn claim_deleted_sessions_for_check(
    pool: &SqlitePool,
    lease_secs: i64,
    limit: i64,
) -> Result<Vec<(String, String, i64)>, SqliteError> {
    let mut rows: Vec<(String, String, i64)> = sqlx::query_as(
        "UPDATE deleted_sessions \
         SET next_check_at = unixepoch() + ?, claim_token = claim_token + 1 \
         WHERE (project_id, session_id) IN ( \
             SELECT project_id, session_id FROM deleted_sessions \
             WHERE next_check_at <= unixepoch() \
             ORDER BY next_check_at, project_id, session_id \
             LIMIT ? \
         ) \
         RETURNING project_id, session_id, claim_token",
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.sort_unstable_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    Ok(rows)
}

/// Record what a deleted session's check found, matched on the claim token.
pub async fn record_deleted_session_check(
    pool: &SqlitePool,
    project_id: &str,
    session_id: &str,
    claim_token: i64,
    was_quiet: bool,
    base_gap_secs: i64,
    max_gap_secs: i64,
) -> Result<(), SqliteError> {
    if was_quiet {
        sqlx::query(
            "UPDATE deleted_sessions \
             SET quiet_checks = quiet_checks + 1, \
                 next_check_at = unixepoch() + MIN(? * (1 << MIN(quiet_checks, 20)), ?) \
             WHERE project_id = ? AND session_id = ? AND claim_token = ?",
        )
        .bind(base_gap_secs)
        .bind(max_gap_secs)
        .bind(project_id)
        .bind(session_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE deleted_sessions SET quiet_checks = 0, next_check_at = unixepoch() + ? \
             WHERE project_id = ? AND session_id = ? AND claim_token = ?",
        )
        .bind(base_gap_secs)
        .bind(project_id)
        .bind(session_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Record that these sessions were deleted. See the trait method for why the trace tombstone alone is
/// insufficient.
pub async fn record_deleted_sessions(
    pool: &SqlitePool,
    project_id: &str,
    session_ids: &[String],
    now: i64,
) -> Result<(), SqliteError> {
    if session_ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    insert_session_tombstones(&mut tx, project_id, session_ids, now).await?;
    tx.commit().await?;
    Ok(())
}

/// The session tombstone rows, inside a transaction the caller owns - see [`insert_trace_tombstones`].
async fn insert_session_tombstones(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    session_ids: &[String],
    now: i64,
) -> Result<(), SqliteError> {
    for session_id in session_ids {
        sqlx::query(
            "INSERT INTO deleted_sessions (project_id, session_id, deleted_at) VALUES (?, ?, ?)
             ON CONFLICT(project_id, session_id) DO NOTHING",
        )
        .bind(project_id)
        .bind(session_id)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Which of these sessions are tombstoned.
pub async fn deleted_sessions_among(
    pool: &SqlitePool,
    project_id: &str,
    session_ids: &[String],
) -> Result<std::collections::HashSet<String>, SqliteError> {
    if session_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let placeholders = vec!["?"; session_ids.len()].join(", ");
    let sql = format!(
        "SELECT session_id FROM deleted_sessions \
         WHERE project_id = ? AND session_id IN ({placeholders})"
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql).bind(project_id);
    for session_id in session_ids {
        query = query.bind(session_id);
    }
    Ok(query.fetch_all(pool).await?.into_iter().collect())
}

/// Re-lease an abandoned project cleanup, if its tombstone is still the value observed.
///
/// Refreshing `deleting_at` leases the work without lifting the fence: the column is non-NULL either way, so
/// every write path stays refused, while a resumer that crashes leaves it looking abandoned again once the
/// window passes. A second replica holding the same reading is refused by the compare.
pub async fn reclaim_stale_project(
    pool: &SqlitePool,
    id: &str,
    observed_deleting_at: i64,
    now: i64,
) -> Result<bool, SqliteError> {
    let result =
        sqlx::query("UPDATE projects SET deleting_at = ? WHERE id = ? AND deleting_at = ?")
            .bind(now)
            .bind(id)
            .bind(observed_deleting_at)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Re-lease an abandoned organization cleanup, if its tombstone is still the value observed.
///
/// Refreshing `deleting_at` leases the work without lifting the fence: the column is non-NULL either way, so
/// every write path stays refused, while a resumer that crashes leaves it looking abandoned again once the
/// window passes. A second replica holding the same reading is refused by the compare.
pub async fn reclaim_stale_organization(
    pool: &SqlitePool,
    id: &str,
    observed_deleting_at: i64,
    now: i64,
) -> Result<bool, SqliteError> {
    let result =
        sqlx::query("UPDATE organizations SET deleting_at = ? WHERE id = ? AND deleting_at = ?")
            .bind(now)
            .bind(id)
            .bind(observed_deleting_at)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Append journal rows inside a transaction the caller already owns.
///
/// Exists so a tombstone or a claim can be made atomic with its record. See
/// [`sideseat_ports::traits::DeletionJournal::record_deleted_traces_journalled`] for why the pair must be one
/// transaction rather than an ordering.
async fn append_journal_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    records: &[sideseat_ports::traits::DeletionRecord],
) -> Result<(), SqliteError> {
    for record in records {
        sqlx::query(
            "INSERT INTO deletion_journal
                 (project_id, cause, scope, target_id, span_id, recorded_at, logical_bytes)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.project_id.as_str())
        .bind(record.cause.as_str())
        .bind(record.scope.as_str())
        .bind(&record.target_id)
        .bind(record.span_id.as_deref())
        .bind(record.recorded_at.timestamp_nanos_opt().unwrap_or(i64::MAX))
        .bind(i64::try_from(record.logical_bytes()).unwrap_or(i64::MAX))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// One `requested` journal record per target.
fn requested(
    project_id: &str,
    scope: sideseat_ports::traits::DeletionScope,
    targets: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<sideseat_ports::traits::DeletionRecord> {
    targets
        .iter()
        .map(|target_id| sideseat_ports::traits::DeletionRecord {
            project_id: ProjectId::from(project_id),
            cause: sideseat_ports::traits::DeletionCause::Requested,
            scope,
            target_id: target_id.clone(),
            span_id: None,
            recorded_at: now,
        })
        .collect()
}

/// [`record_deleted_traces`] and the journal entries, in one transaction.
pub async fn record_deleted_traces_journalled(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), SqliteError> {
    if trace_ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    insert_trace_tombstones(&mut tx, project_id, trace_ids, now.timestamp()).await?;
    append_journal_in_tx(
        &mut tx,
        &requested(
            project_id,
            sideseat_ports::traits::DeletionScope::Trace,
            trace_ids,
            now,
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Both tombstones and both journal scopes, in one transaction.
pub async fn record_deleted_sessions_journalled(
    pool: &SqlitePool,
    project_id: &str,
    session_ids: &[String],
    trace_ids: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), SqliteError> {
    if session_ids.is_empty() && trace_ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    insert_session_tombstones(&mut tx, project_id, session_ids, now.timestamp()).await?;
    insert_trace_tombstones(&mut tx, project_id, trace_ids, now.timestamp()).await?;

    let mut records = requested(
        project_id,
        sideseat_ports::traits::DeletionScope::Session,
        session_ids,
        now,
    );
    records.extend(requested(
        project_id,
        sideseat_ports::traits::DeletionScope::Trace,
        trace_ids,
        now,
    ));
    append_journal_in_tx(&mut tx, &records).await?;
    tx.commit().await?;
    Ok(())
}

/// Journal pressure-selected spans and record their trace cleanup in the same transaction.
async fn record_span_deletions_journalled(
    pool: &SqlitePool,
    project_id: &str,
    spans: &[(String, String)],
    cause: DeletionCause,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<(String, i64)>, SqliteError> {
    if spans.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    for (trace_id, span_id) in spans {
        let record = DeletionRecord {
            project_id: ProjectId::from(project_id),
            cause,
            scope: DeletionScope::Span,
            target_id: trace_id.clone(),
            span_id: Some(span_id.clone()),
            recorded_at: now,
        };
        sqlx::query(
            "INSERT INTO deletion_journal
                 (project_id, cause, scope, target_id, span_id, recorded_at, logical_bytes)
             SELECT ?, ?, ?, ?, ?, ?, ?
             WHERE NOT EXISTS (
                 SELECT 1 FROM deletion_journal
                 WHERE project_id = ? AND scope = 'span' AND target_id = ? AND span_id = ?
             )",
        )
        .bind(project_id)
        .bind(record.cause.as_str())
        .bind(record.scope.as_str())
        .bind(trace_id)
        .bind(span_id)
        .bind(now.timestamp_nanos_opt().unwrap_or(i64::MAX))
        .bind(i64::try_from(record.logical_bytes()).unwrap_or(i64::MAX))
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .execute(&mut *tx)
        .await?;
    }

    let mut trace_ids = spans
        .iter()
        .map(|(trace_id, _)| trace_id.clone())
        .collect::<Vec<_>>();
    trace_ids.sort();
    trace_ids.dedup();
    let mut written = Vec::with_capacity(trace_ids.len());
    for trace_id in trace_ids {
        let token: i64 = sqlx::query_scalar(
            "INSERT INTO retention_cleanup
                 (project_id, trace_id, created_at, attempts, next_attempt_at, claim_token, logical_bytes)
             VALUES (?, ?, ?, 0, 0, 1, ?)
             ON CONFLICT(project_id, trace_id) DO UPDATE SET
                 claim_token = claim_token + 1, next_attempt_at = 0
             RETURNING claim_token",
        )
        .bind(project_id)
        .bind(&trace_id)
        .bind(now.timestamp())
        .bind(
            i64::try_from(retention_cleanup_logical_bytes(project_id, &trace_id))
                .unwrap_or(i64::MAX),
        )
        .fetch_one(&mut *tx)
        .await?;
        written.push((trace_id, token));
    }
    tx.commit().await?;
    Ok(written)
}

pub async fn record_deleted_spans_journalled(
    pool: &SqlitePool,
    project_id: &str,
    spans: &[(String, String)],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<(String, i64)>, SqliteError> {
    record_span_deletions_journalled(pool, project_id, spans, DeletionCause::Requested, now).await
}

pub async fn record_pressure_eviction(
    pool: &SqlitePool,
    project_id: &str,
    spans: &[(String, String)],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<(String, i64)>, SqliteError> {
    record_span_deletions_journalled(pool, project_id, spans, DeletionCause::Pressure, now).await
}

/// [`claim_project_for_deletion`], with the journal entry written **only if the claim was won**.
///
/// One transaction, so there is no state in which the project is fenced without a record or recorded without
/// being fenced. Conditional on winning, which the separate calls could not express: journalling before the claim
/// wrote an entry for every losing caller, and an organization cleanup re-runs while its projects' tombstones
/// remain - so one deletion accumulated permanent, quota-counted records without bound.
pub async fn claim_project_for_deletion_journalled(
    pool: &SqlitePool,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, SqliteError> {
    let mut tx = pool.begin().await?;
    let result =
        sqlx::query("UPDATE projects SET deleting_at = ? WHERE id = ? AND deleting_at IS NULL")
            .bind(now.timestamp())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    let claimed = result.rows_affected() > 0;
    if claimed {
        append_journal_in_tx(
            &mut tx,
            &requested(
                id,
                sideseat_ports::traits::DeletionScope::Project,
                std::slice::from_ref(&id.to_string()),
                now,
            ),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(claimed)
}

/// [`claim_organization_for_deletion`], with its journal entry. See the project twin.
pub async fn claim_organization_for_deletion_journalled(
    pool: &SqlitePool,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, SqliteError> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        "UPDATE organizations SET deleting_at = ? WHERE id = ? AND deleting_at IS NULL",
    )
    .bind(now.timestamp())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let claimed = result.rows_affected() > 0;
    if claimed {
        append_journal_in_tx(
            &mut tx,
            &requested(
                id,
                sideseat_ports::traits::DeletionScope::Organization,
                std::slice::from_ref(&id.to_string()),
                now,
            ),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(claimed)
}
