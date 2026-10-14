// The store lease and the acknowledgement of exports: included by `ingest_redelivery.rs`.

/// A server whose store lease is lost acknowledges nothing: an export arriving then answers 503 before it is
/// staged, and the exporter retries it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_while_the_lease_is_lost_is_refused_before_it_is_staged() {
    let (_held, held_until) = tokio::sync::watch::channel(None);
    let harness = harness_fenced(CollectionFence::following(held_until)).await;
    let stores = &harness.stores;
    let export = media_export(0x63);
    let body = export.encode_to_vec();
    let answer = export_signal(
        &stores.traces,
        export,
        stores.context(PROJECT, &received(&body, 0)),
    )
    .await;
    assert!(
        matches!(answer, Err(SignalExportError::StoreUnavailable)),
        "acknowledged while the lease was lost: {answer:?}"
    );
    assert!(
        stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty()
    );
}

/// An export in flight while its server loses the store lease, a restore takes the export's file back - row,
/// association and bytes - and the server takes the lease again is not acknowledged: the lease was not held
/// throughout, and what the export wrote before the restore is gone. The exporter's retry is acknowledged, and
/// its image reads as the bytes sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_in_flight_across_a_lost_lease_and_a_restore_is_retried_not_acknowledged() {
    use sideseat_ports::blobs::FileStorage;

    use tokio::time::{Duration, Instant};

    let (held, held_until) = tokio::sync::watch::channel(Some(HeldUntil::new(
        Instant::now() + Duration::from_secs(600),
        0,
    )));
    let harness = harness_fenced(CollectionFence::following(held_until)).await;
    let stores = &harness.stores;
    let project = sideseat_ports::types::ProjectId::from(PROJECT);
    let [media] = sideseat_domain::raw_payload::media_in(image_text().as_bytes())
        .try_into()
        .expect("one image");
    let hash = media.hash_hex();
    let TransactionalService::Sqlite(sqlite) = stores.database.as_ref() else {
        panic!("embedded stores");
    };
    let export = media_export(0x62);
    let body = export.encode_to_vec();

    // The export writes, then retries its settlement until the restore below is done.
    refuse_settlement(stores, true).await;
    let first = received(&body, 0);
    let exporting = export_signal(
        &stores.traces,
        export.clone(),
        stores.context(PROJECT, &first),
    );
    let restoring = async {
        while !stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .iter()
            .any(|payload| payload.redrive_attempts > 0)
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        held.send(None).unwrap();
        for statement in [
            "DELETE FROM trace_files WHERE project_id = ?",
            "DELETE FROM files WHERE project_id = ?",
        ] {
            sqlx::query(statement)
                .bind(PROJECT)
                .execute(sqlite.pool())
                .await
                .expect("restored");
        }
        // The image's bytes, and only them: the export's staged copy is the staging registry's, which the
        // restore keeps.
        let blobs = sideseat_adapter_blob_storage::FilesystemStorage::new(
            sideseat_core::storage::AppStorage::init_for_test(harness._dir.path().to_path_buf())
                .subdir(sideseat_core::storage::DataSubdir::Files),
        );
        let incarnations = blobs.incarnations(&project, &hash).await.expect("listed");
        assert!(!incarnations.is_empty(), "the export stored its image");
        for incarnation in incarnations {
            blobs
                .delete(
                    &project,
                    sideseat_ports::blobs::ObjectName::new(&hash, incarnation),
                )
                .await
                .expect("restored blob store");
        }
        // Taken again: a later term.
        held.send(Some(HeldUntil::new(
            Instant::now() + Duration::from_secs(600),
            1,
        )))
        .unwrap();
        refuse_settlement(stores, false).await;
    };
    let (answer, ()) = tokio::join!(exporting, restoring);
    assert!(
        matches!(answer, Err(SignalExportError::StoreUnavailable)),
        "acknowledged across a lost lease: {answer:?}"
    );

    export_signal(
        &stores.traces,
        export,
        stores.context(PROJECT, &received(&body, 1)),
    )
    .await
    .expect("the retry is acknowledged");
    assert_eq!(
        stores
            .pipeline
            .file_service()
            .get_file(&project, &hash)
            .await
            .expect("the retry's image is readable")
            .data,
        image()
    );
    stores
        .staging
        .redrive_once(&stores.pipeline, 10)
        .await
        .expect("the sweep");
    assert!(
        stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty(),
        "every attempt's payload is retired once the lease is held"
    );
}
