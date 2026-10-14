use super::*;

async fn pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::raw_sql(crate::schema::SCHEMA)
        .execute(&pool)
        .await
        .unwrap();
    pool
}

/// A token is never carried twice. A worker that completed a trace's cleanup and paused, its trace then recorded
/// again, must not delete the new intent with its old token - which it did when a re-inserted row restarted its
/// tokens at 1.
#[tokio::test]
async fn a_stale_token_does_not_complete_a_cleanup_recorded_after_its_own() {
    let pool = pool().await;
    let traces = ["t1".to_string()];
    let first = record_retention_cleanup(&pool, "p", &traces, 1_000)
        .await
        .unwrap();
    complete_retention_cleanup(&pool, "p", &first)
        .await
        .unwrap();
    let second = record_retention_cleanup(&pool, "p", &traces, 1_001)
        .await
        .unwrap();
    assert_ne!(first, second, "a fresh token");
    complete_retention_cleanup(&pool, "p", &first)
        .await
        .unwrap();
    assert_eq!(
        claim_retention_cleanup(&pool, 16, 0, 2_000).await.unwrap(),
        vec![("p".to_string(), "t1".to_string(), second[0].1)],
        "the later intent survives the stale completion"
    );
}

/// Tokens are unique across traces and projects too, as stored, so a token completed under one identity can
/// never be the current token of another.
#[tokio::test]
async fn tokens_are_unique_across_every_row() {
    let pool = pool().await;
    for project in ["p", "q"] {
        record_retention_cleanup(
            &pool,
            project,
            &["t1".to_string(), "t2".to_string(), "t1".to_string()],
            1_000,
        )
        .await
        .unwrap();
    }
    let twice = record_retention_cleanup(
        &pool,
        "r",
        &["t2".to_string(), "t1".to_string(), "t2".to_string()],
        1_000,
    )
    .await
    .unwrap();
    assert_eq!(
        twice
            .iter()
            .map(|(trace, _)| trace.as_str())
            .collect::<Vec<_>>(),
        ["t1", "t2"],
        "each trace once, in order"
    );
    let stored: Vec<i64> = claim_retention_cleanup(&pool, 16, 0, 2_000)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, _, token)| token)
        .collect();
    let distinct: std::collections::HashSet<i64> = stored.iter().copied().collect();
    assert_eq!(stored.len(), 6);
    assert_eq!(distinct.len(), stored.len(), "{stored:?}");
}
