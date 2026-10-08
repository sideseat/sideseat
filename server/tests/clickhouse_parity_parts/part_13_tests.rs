/// An acknowledged insert is readable at once, whatever the server's profile says about async inserts.
///
/// ClickHouse 26 turns `async_insert` on by default, and a profile can turn `wait_for_async_insert` off, so that
/// an INSERT returns while the rows are only in the server's buffer - and the write path would answer 200 for
/// them. The adapter pins both settings on every query instead of inheriting them. The profile here asks for
/// fire-and-forget with a flush a minute away, so an insert that inherited it would not be readable yet.
#[tokio::test]
async fn an_acknowledged_insert_is_readable_whatever_the_server_profile() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_fire_and_forget";
    let user = "sideseat_parity_fire_and_forget";
    let password = "fire-and-forget";
    // The schema, created by the suite's own user.
    clickhouse_backend(&url, database).await;
    let admin = raw_client(&url, "default");
    for statement in [
        format!("DROP USER IF EXISTS {user}"),
        format!("DROP SETTINGS PROFILE IF EXISTS {user}"),
        format!(
            "CREATE SETTINGS PROFILE {user} SETTINGS async_insert = 1, wait_for_async_insert = 0, \
             async_insert_use_adaptive_busy_timeout = 0, async_insert_busy_timeout_ms = 60000, \
             async_insert_max_data_size = 1000000000, async_insert_max_query_number = 100000"
        ),
        format!(
            "CREATE USER {user} IDENTIFIED WITH plaintext_password BY '{password}' \
             SETTINGS PROFILE '{user}'"
        ),
        format!("GRANT CURRENT GRANTS ON *.* TO {user}"),
    ] {
        admin
            .query(&statement)
            .execute()
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }

    let repository = sideseat_adapter_clickhouse::ClickhouseRepository(Arc::new(
        ClickhouseService::init(
            &ClickhouseConfig {
                url: url.clone(),
                database: database.to_string(),
                user: Some(user.to_string()),
                password: Some(password.to_string()),
                timeout_secs: 30,
                compression: false,
                async_insert: false,
                wait_for_async_insert: true,
                cluster: None,
                distributed: false,
                insert_quorum: 0,
            },
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("clickhouse init under the fire-and-forget profile"),
    ));
    let span = NormalizedSpan {
        project_id: Some("fire-and-forget".to_string()),
        trace_id: "acknowledged-trace".to_string(),
        span_id: "acknowledged-span".to_string(),
        span_name: "step".to_string(),
        timestamp_start: chrono::Utc::now(),
        ..Default::default()
    };
    repository
        .insert_spans(vec![span])
        .await
        .expect("the insert is acknowledged");

    let (spans, _) = repository
        .list_spans(&sideseat_ports::types::ListSpansParams {
            project_id: ProjectId::from("fire-and-forget"),
            page: 1,
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("spans");
    for statement in [
        format!("DROP USER IF EXISTS {user}"),
        format!("DROP SETTINGS PROFILE IF EXISTS {user}"),
    ] {
        admin.query(&statement).execute().await.expect("clean up");
    }
    assert_eq!(
        spans.len(),
        1,
        "an insert that returned is readable: it did not wait on the profile's buffer"
    );
}
