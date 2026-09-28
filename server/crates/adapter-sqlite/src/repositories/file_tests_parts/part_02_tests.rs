/// Deleting several traces that share a file must release every reference they held.
///
/// The loop decremented once per distinct hash, so deleting three traces that referenced one file
/// removed three associations and one reference - leaving a file nothing points at and nothing will
/// ever collect.
#[tokio::test]
async fn deleting_traces_releases_every_reference_they_held() {
    let pool = setup_test_pool().await;
    let traces = ["trace-a", "trace-b", "trace-c"];
    for trace in traces {
        associate_file(
            &pool,
            trace,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
    }

    let counts = get_file_reference_counts_for_traces(
        &pool,
        "default",
        &traces.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    assert_eq!(counts, vec![(test_hash().to_string(), 3)]);

    delete_trace_files(
        &pool,
        "default",
        &traces.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    let released = sync_ref_count(&pool, "default", test_hash()).await.unwrap();
    assert_eq!(
        released,
        Some(0),
        "all three references released at once, so the file is collectable"
    );
}

/// `ref_count` must equal the number of trace associations, or deleting one trace deletes a file
/// another trace still shows.
///
/// The defect this pins: ingestion incremented once per batch-unique hash while associating per
/// trace, so two traces of one batch sharing a file held two associations and one count. Deleting
/// either took the count to zero and removed the bytes from under the other.
#[tokio::test]
async fn ref_count_matches_the_number_of_trace_associations() {
    let pool = setup_test_pool().await;

    // Two traces of one batch reference the same content-addressed file. Ingestion increments
    // once per association, which is what the loop in `write_and_record_files` now does.
    for trace in ["trace-a", "trace-b"] {
        upsert_file(
            &pool,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        insert_trace_file(&pool, trace, "default", test_hash())
            .await
            .unwrap();
    }

    // Deleting the first trace must leave the file alive for the second.
    delete_trace_files(&pool, "default", &["trace-a".to_string()])
        .await
        .unwrap();
    let after_first = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert_eq!(
        after_first,
        Some(1),
        "the file is still referenced by trace-b, so it must not be collectable"
    );

    // Deleting the second releases it.
    delete_trace_files(&pool, "default", &["trace-b".to_string()])
        .await
        .unwrap();
    let after_second = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert_eq!(
        after_second,
        Some(0),
        "with no trace referencing it the file is collectable"
    );
}
