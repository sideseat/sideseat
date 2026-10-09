use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::Router;
use axum::routing::post;
use tower::ServiceExt;

use super::*;

#[test]
fn exports_are_admitted_while_the_budget_holds_them_and_released_when_answered() {
    let admission = IngestAdmission::new(100);
    let first = admission.try_admit(60).expect("within the budget");
    assert_eq!(admission.in_flight(), 60);
    assert!(admission.try_admit(50).is_none(), "60 + 50 is over 100");
    let second = admission.try_admit(40).expect("60 + 40 fits");
    assert_eq!(admission.in_flight(), 100);
    drop(first);
    assert_eq!(admission.in_flight(), 40);
    drop(second);
    assert_eq!(admission.in_flight(), 0);
}

/// A body larger than the whole budget is admitted when nothing else is in flight, and holds all of it.
#[test]
fn an_export_larger_than_the_budget_is_admitted_alone() {
    let admission = IngestAdmission::new(100);
    let small = admission.try_admit(1).expect("a small export");
    assert!(
        admission.try_admit(500).is_none(),
        "not beside another export"
    );
    drop(small);
    let large = admission.try_admit(500).expect("alone, it is admitted");
    assert_eq!(admission.in_flight(), 100);
    assert!(admission.try_admit(1).is_none(), "and nothing beside it");
    drop(large);
    assert_eq!(admission.in_flight(), 0);
}

/// A body whose bytes the handler is told about, and whether anything read it.
struct Probe {
    read: Arc<AtomicBool>,
}

impl Probe {
    fn body(&self, bytes: usize) -> Body {
        let read = Arc::clone(&self.read);
        Body::from_stream(futures::stream::once(async move {
            read.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(Bytes::from(vec![0u8; bytes]))
        }))
    }
}

fn router(admission: Arc<IngestAdmission>, answered: Arc<AtomicUsize>) -> Router {
    Router::new()
        .route(
            "/traces",
            post(move |body: Bytes| {
                let answered = Arc::clone(&answered);
                async move {
                    answered.fetch_add(body.len(), Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(admission, admit_http))
}

fn export(body: Body, length: Option<usize>) -> Request {
    let mut request = axum::http::Request::post("/traces");
    if let Some(length) = length {
        request = request.header(header::CONTENT_LENGTH, length);
    }
    request.body(body).expect("request")
}

/// Over the budget, an export with a declared length is answered 503 with `Retry-After` and its body is never
/// read; within it, the export reaches the handler and its bytes are released when it is answered.
#[tokio::test]
async fn an_export_over_the_budget_is_refused_before_its_body_is_read() {
    let admission = Arc::new(IngestAdmission::new(1_000));
    let answered = Arc::new(AtomicUsize::new(0));
    let app = router(Arc::clone(&admission), Arc::clone(&answered));
    let held = admission.try_admit(900).expect("another export in flight");

    let probe = Probe {
        read: Arc::new(AtomicBool::new(false)),
    };
    let response = app
        .clone()
        .oneshot(export(probe.body(500), Some(500)))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .map(|v| v.to_str().unwrap()),
        Some(BACKPRESSURE_RETRY_AFTER_SECS.to_string().as_str())
    );
    assert!(!probe.read.load(Ordering::SeqCst), "the body was read");
    assert_eq!(answered.load(Ordering::SeqCst), 0);

    drop(held);
    let response = app
        .oneshot(export(probe.body(500), Some(500)))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(answered.load(Ordering::SeqCst), 500);
    assert_eq!(admission.in_flight(), 0, "released once answered");
}

/// A body without a declared length is read here and held as it arrives: refused when the budget runs out, and
/// passed on whole when it does not.
#[tokio::test]
async fn an_export_without_a_length_is_held_as_it_is_read() {
    let admission = Arc::new(IngestAdmission::new(1_000));
    let answered = Arc::new(AtomicUsize::new(0));
    let app = router(Arc::clone(&admission), Arc::clone(&answered));
    let probe = Probe {
        read: Arc::new(AtomicBool::new(false)),
    };

    let response = app
        .clone()
        .oneshot(export(probe.body(700), None))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(answered.load(Ordering::SeqCst), 700);
    assert_eq!(admission.in_flight(), 0);

    let held = admission.try_admit(900).expect("another export in flight");
    let response = app
        .oneshot(export(probe.body(700), None))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        answered.load(Ordering::SeqCst),
        700,
        "never reached the handler"
    );
    drop(held);
    assert_eq!(admission.in_flight(), 0);
}

/// A declared length over the route's limit is refused before anything is read or held.
#[tokio::test]
async fn an_export_over_the_body_limit_is_refused_unread() {
    let admission = Arc::new(IngestAdmission::new(1_000));
    let app = router(Arc::clone(&admission), Arc::new(AtomicUsize::new(0)));
    let probe = Probe {
        read: Arc::new(AtomicBool::new(false)),
    };
    let response = app
        .oneshot(export(probe.body(1), Some(OTLP_BODY_LIMIT + 1)))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!probe.read.load(Ordering::SeqCst));
}

/// With the budget held whole, an export without a declared length is refused before its body is polled: a
/// client that withholds its first chunk is answered at once, and a compressed body is not decompressed first.
#[tokio::test]
async fn with_the_budget_held_whole_an_unlengthed_export_is_refused_unread() {
    let admission = Arc::new(IngestAdmission::new(100));
    let app = router(Arc::clone(&admission), Arc::new(AtomicUsize::new(0)));
    let probe = Probe {
        read: Arc::new(AtomicBool::new(false)),
    };
    let held = admission.try_admit(100).expect("the whole budget");
    assert!(admission.exhausted());
    let response = app
        .oneshot(export(probe.body(10), None))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!probe.read.load(Ordering::SeqCst), "the body was read");
    drop(held);
}

/// A client that goes away mid-export leaves its bytes held until the export is done with them: the export is
/// still being persisted, and releasing them would admit another in its place while the first is resident.
#[tokio::test]
async fn a_client_that_goes_away_leaves_its_bytes_held_until_the_export_is_done() {
    let admission = Arc::new(IngestAdmission::new(100));
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let app = {
        let (started, finish) = (Arc::clone(&started), Arc::clone(&finish));
        Router::new()
            .route(
                "/traces",
                post(move |_body: Bytes| {
                    let (started, finish) = (Arc::clone(&started), Arc::clone(&finish));
                    async move {
                        started.notify_one();
                        finish.notified().await;
                        StatusCode::OK
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&admission),
                admit_http,
            ))
    };
    let client = tokio::spawn(app.oneshot(export(Body::from(vec![0u8; 40]), Some(40))));
    started.notified().await;
    assert_eq!(admission.in_flight(), 40);
    client.abort();
    let _ = client.await;
    assert_eq!(
        admission.in_flight(),
        40,
        "the bytes went with the connection while the export was still running"
    );
    finish.notify_one();
    for _ in 0..1_000 {
        if admission.in_flight() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(admission.in_flight(), 0, "released once the export is done");
}

/// Closing for a shutdown waits for an admitted export that is still running - its client gone - and then
/// admits nothing more, so the stores it writes to are not closed under it.
#[tokio::test]
async fn closing_waits_for_running_exports_and_then_admits_none() {
    let admission = Arc::new(IngestAdmission::new(100));
    let running = admission.try_admit(40).expect("an export in flight");
    let finished = Arc::new(AtomicBool::new(false));
    let finish = {
        let finished = Arc::clone(&finished);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            finished.store(true, Ordering::SeqCst);
            drop(running);
        })
    };
    let closed = admission
        .close(std::time::Duration::from_secs(5))
        .await
        .expect("closed once the export finished");
    assert!(
        finished.load(Ordering::SeqCst),
        "closed before the export finished"
    );
    assert!(
        admission.try_admit(1).is_none(),
        "admitted an export after closing"
    );
    finish.await.expect("finish");
    drop(closed);

    let stuck = admission.try_admit(10).expect("another export");
    assert!(
        admission
            .close(std::time::Duration::from_millis(20))
            .await
            .is_none(),
        "closing gives up after its timeout"
    );
    drop(stuck);
}
