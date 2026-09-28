use super::*;

const TEST_NOW: i64 = 1_700_000_000;

#[allow(clippy::too_many_arguments)]
async fn associate_file(
    pool: &SqlitePool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<bool, SqliteError> {
    super::associate_file(
        pool, trace_id, project_id, file_hash, media_type, size_bytes, hash_algo, TEST_NOW,
    )
    .await
}

/// Confirming a duplicated tuple decrements `pending_writers` once, matching the PostgreSQL `UNNEST`.
///
/// Input is treated as a set of associations, so duplicate tuples cannot resolve more than one writer.
/// This matches PostgreSQL's set-based `IN (UNNEST(...))` behaviour.
#[tokio::test]
async fn confirm_with_a_duplicated_tuple_decrements_once() {
    let pool = setup_test_pool().await;
    let (project, trace, hash) = ("default", "t1", "aa");

    // Two in-flight writers.
    for _ in 0..2 {
        associate_file(&pool, trace, project, hash, Some("image/png"), 1, "sha256")
            .await
            .unwrap();
    }

    // Confirm with the same tuple listed twice.
    let assoc = (project.to_string(), trace.to_string(), hash.to_string());
    confirm_trace_file_associations(&pool, &[assoc.clone(), assoc])
        .await
        .unwrap();

    let (pending, durable): (i64, i64) = sqlx::query_as(
            "SELECT pending_writers, durable FROM trace_files WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
        )
        .bind(project)
        .bind(trace)
        .bind(hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(durable, 1, "confirm must mark the association durable");
    assert_eq!(
        pending, 1,
        "a duplicated tuple must decrement pending_writers once, not twice"
    );
}
