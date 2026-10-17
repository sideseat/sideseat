/// A TCP relay to PostgreSQL that counts the server's `ReadyForQuery` messages: one per round trip the client
/// waited out, whatever the statement, so the count is the round trips themselves - deterministic, where a timing
/// on a shared host is not. Its address, and the running count.
async fn counting_relay(
    upstream: std::net::SocketAddr,
) -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicU64>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a relay port");
    let address = listener.local_addr().expect("the relay's address");
    let ready = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = Arc::clone(&ready);
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                return;
            };
            let server = tokio::net::TcpStream::connect(upstream)
                .await
                .expect("PostgreSQL");
            let (mut client_read, mut client_write) = client.into_split();
            let (mut server_read, mut server_write) = server.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut client_read, &mut server_write).await;
            });
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                // Backend messages: a type byte, then a big-endian length that counts itself.
                let mut pending: Vec<u8> = Vec::new();
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    let Ok(read) = server_read.read(&mut buffer).await else {
                        return;
                    };
                    if read == 0 || client_write.write_all(&buffer[..read]).await.is_err() {
                        return;
                    }
                    pending.extend_from_slice(&buffer[..read]);
                    while pending.len() >= 5 {
                        let length =
                            u32::from_be_bytes([pending[1], pending[2], pending[3], pending[4]])
                                as usize;
                        if pending.len() < 1 + length {
                            break;
                        }
                        if pending[0] == b'Z' {
                            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        pending.drain(..1 + length);
                    }
                }
            });
        }
    });
    (address, ready)
}

/// The round trips one tenant transaction of one statement costs, counted at the wire: the statement that begins
/// the transaction also sets its project selector, so it is the pool's ping on acquire, the begin, the statement,
/// the commit, and the ping sqlx 0.8 sends when a connection goes back to the pool (`PoolConnection::return_to_pool`,
/// unconditional). Before, it was six: the selector was a `set_config` of its own.
#[tokio::test]
async fn a_tenant_transaction_of_one_statement_is_five_round_trips() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("skipping: {URL_ENV} is not set");
        return;
    };
    // `postgres://user:password@host:port/database[?query]`: the relay stands in for `host:port`.
    let (credentials, rest) = url.split_once('@').expect("a URL with credentials");
    let (authority, path) = rest.split_once('/').expect("a database");
    let database = path.split('?').next().unwrap_or(path);
    let upstream = tokio::net::lookup_host(authority)
        .await
        .expect("resolve PostgreSQL")
        .next()
        .expect("an address");
    let (relay, ready) = counting_relay(upstream).await;
    let relayed = format!(
        "{credentials}@127.0.0.1:{}/{database}?sslmode=disable",
        relay.port()
    );
    let config = PostgresConfig {
        url: relayed,
        max_connections: 1,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let service = Arc::new(
        PostgresService::init(
            &config,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("PostgreSQL through the relay"),
    );
    let governance = TransactionalService::Postgres(Arc::clone(&service)).governance_repository();
    let project = ProjectId::new("default");
    let reserve = || governance.reserve_project_storage(&project, 0, 1 << 40, chrono::Utc::now());
    // Warm: the connection opened, its statement prepared.
    reserve().await.expect("a warm reservation");
    // A connection goes back to the pool, and is pinged, in a task of its own: let it settle before each count.
    let settle = || tokio::time::sleep(std::time::Duration::from_millis(200));
    settle().await;
    let before = ready.load(std::sync::atomic::Ordering::SeqCst);
    const N: u64 = 20;
    for _ in 0..N {
        reserve().await.expect("a reservation");
    }
    settle().await;
    let round_trips = ready.load(std::sync::atomic::Ordering::SeqCst) - before;
    eprintln!(
        "round trips per tenant transaction of one statement: {}",
        round_trips as f64 / N as f64
    );
    assert_eq!(
        round_trips,
        5 * N,
        "{round_trips} round trips for {N} transactions"
    );
}

/// A tenant transaction that does not commit - a statement in it failed, or it was dropped - is rolled back before
/// its connection serves anyone else: on a one-connection pool, the next user sees no open transaction and no
/// selector, and the next tenant transaction its own selector.
#[tokio::test]
async fn an_unfinished_tenant_transaction_never_reaches_the_next_user() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("skipping: {URL_ENV} is not set");
        return;
    };
    let config = PostgresConfig {
        url,
        max_connections: 1,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let service = PostgresService::init(
        &config,
        Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await
    .expect("PostgreSQL");
    let selector = "SELECT coalesce(current_setting('sideseat.project_id', true), '')";
    for unfinished in [
        "a failed statement",
        "dropped",
        "abandoned while beginning",
        "abandoned while committing",
    ] {
        if unfinished == "abandoned while committing" {
            // The commit dropped while it still drains a query stream left mostly unread - the window before COMMIT
            // is even sent.
            for millis in [1, 5, 20] {
                let mut transaction = service
                    .tenant_transaction(&ProjectId::new("tenant-a"))
                    .await
                    .expect("a tenant transaction");
                {
                    use futures::StreamExt as _;
                    let connection: &mut sqlx::PgConnection = &mut transaction;
                    let mut rows =
                        sqlx::query("SELECT repeat('x', 100) FROM generate_series(1, 2000000)")
                            .fetch(connection);
                    let _ = rows.next().await;
                }
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(millis),
                    transaction.commit(),
                )
                .await;
            }
        } else if unfinished == "abandoned while beginning" {
            // Dropped wherever the begin happens to be - before it is sent, while it runs, after it answers: the
            // invariant below must hold whichever.
            for micros in [1, 10, 50, 100, 200, 500, 1000, 2000] {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_micros(micros),
                    service.tenant_transaction(&ProjectId::new("tenant-a")),
                )
                .await;
            }
        } else {
            let mut transaction = service
                .tenant_transaction(&ProjectId::new("tenant-a"))
                .await
                .expect("a tenant transaction");
            let connection: &mut sqlx::PgConnection = &mut transaction;
            if unfinished == "a failed statement" {
                assert!(
                    sqlx::query("SELECT 1 / 0")
                        .execute(connection)
                        .await
                        .is_err()
                );
            }
        }
        // The next user of the only connection: outside any transaction - a savepoint is refused there - and with
        // no selector.
        let mut next_user = service
            .runtime_pool()
            .acquire()
            .await
            .expect("the only connection");
        let savepoint = sqlx::raw_sql("SAVEPOINT probe")
            .execute(&mut *next_user)
            .await;
        let outside = matches!(
            &savepoint,
            Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("25P01")
        );
        assert!(
            outside,
            "{unfinished}: the next user ran inside the unfinished transaction: {savepoint:?}"
        );
        let scope: String = sqlx::query_scalar(selector)
            .fetch_one(&mut *next_user)
            .await
            .expect("the next user's statement");
        assert_eq!(
            scope, "",
            "{unfinished}: the selector reached the next user"
        );
        drop(next_user);
        let mut next = service
            .tenant_transaction(&ProjectId::new("tenant-b"))
            .await
            .expect("the next tenant transaction");
        let connection: &mut sqlx::PgConnection = &mut next;
        let scope: String = sqlx::query_scalar(selector)
            .fetch_one(connection)
            .await
            .expect("its selector");
        assert_eq!(scope, "tenant-b", "{unfinished}");
        next.commit().await.expect("commit");
    }
}

/// A tenant transaction dropped on a thread with no runtime - returned out of `block_on` - still rolls back before its
/// connection is reused, and does not panic.
#[test]
fn a_tenant_transaction_dropped_outside_its_runtime_is_still_rolled_back() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("skipping: {URL_ENV} is not set");
        return;
    };
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let config = PostgresConfig {
        url,
        max_connections: 1,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let service = runtime.block_on(async {
        PostgresService::init(
            &config,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("PostgreSQL")
    });
    let transaction = runtime
        .block_on(service.tenant_transaction(&ProjectId::new("tenant-a")))
        .expect("a tenant transaction");
    drop(transaction);
    let scope: String = runtime.block_on(async {
        sqlx::query_scalar("SELECT coalesce(current_setting('sideseat.project_id', true), '')")
            .fetch_one(service.runtime_pool())
            .await
            .expect("the next user's statement")
    });
    assert_eq!(scope, "", "the selector reached the next user");
}

/// A tenant transaction begun on a runtime that has shut down by the time it is dropped: its rollback task can never
/// run, so its connection is closed, not handed to the next user with the transaction still open.
#[test]
fn a_tenant_transaction_whose_runtime_is_gone_is_closed_not_reused() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("skipping: {URL_ENV} is not set");
        return;
    };
    let live = tokio::runtime::Runtime::new().expect("the pool's runtime");
    let config = PostgresConfig {
        url,
        max_connections: 1,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let service = live.block_on(async {
        PostgresService::init(
            &config,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("PostgreSQL")
    });
    let gone = tokio::runtime::Runtime::new().expect("a short-lived runtime");
    let transaction = gone
        .block_on(service.tenant_transaction(&ProjectId::new("tenant-a")))
        .expect("a tenant transaction");
    gone.shutdown_background();
    live.block_on(async move {
        drop(transaction);
        let mut next_user = service.runtime_pool().acquire().await.expect("the only connection");
        let savepoint = sqlx::raw_sql("SAVEPOINT probe").execute(&mut *next_user).await;
        assert!(
            matches!(&savepoint, Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("25P01")),
            "the next user ran inside the unfinished transaction: {savepoint:?}"
        );
    });
}

/// A transaction begun on a tenant transaction's connection is a savepoint inside it: committing or dropping it
/// leaves the tenant transaction open, with its selector, until the tenant transaction itself ends.
#[tokio::test]
async fn a_transaction_nested_in_a_tenant_transaction_is_a_savepoint() {
    use sqlx::Connection as _;
    let Some((_, postgres)) = pair().await else {
        return;
    };
    let selector = "SELECT coalesce(current_setting('sideseat.project_id', true), '')";
    for inner_ends in ["committed", "dropped"] {
        let mut tenant = postgres
            .0
            .tenant_transaction(&ProjectId::new("tenant-a"))
            .await
            .expect("a tenant transaction");
        {
            let connection: &mut sqlx::PgConnection = &mut tenant;
            let inner = connection.begin().await.expect("a nested transaction");
            if inner_ends == "committed" {
                inner.commit().await.expect("the nested commit");
            } else {
                drop(inner);
            }
        }
        let connection: &mut sqlx::PgConnection = &mut tenant;
        let scope: String = sqlx::query_scalar(selector)
            .fetch_one(&mut *connection)
            .await
            .expect("the selector");
        assert_eq!(
            scope, "tenant-a",
            "{inner_ends}: the nested transaction ended the tenant's"
        );
        let still_open = sqlx::raw_sql("SAVEPOINT probe")
            .execute(&mut *connection)
            .await;
        assert!(
            still_open.is_ok(),
            "{inner_ends}: the tenant transaction is no longer open: {still_open:?}"
        );
        tenant.commit().await.expect("the tenant commit");
    }
}
