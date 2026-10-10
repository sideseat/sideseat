use sideseat_domain::sideml::feed::FeedResult;

use super::{build_messages_response, stream_messages_response};

/// A view with no message states no start: never the instant it was asked, which made two asks of one span's
/// empty view different answers.
#[test]
fn an_empty_view_states_no_start_time() {
    let view = build_messages_response(&FeedResult::default(), None, Vec::new());
    assert_eq!(view.metadata.start_time, None);
    assert_eq!(view.metadata.end_time, None);
    let json = serde_json::to_string(&view.metadata).expect("serialise");
    assert!(json.contains("\"start_time\":null"), "{json}");
}

/// The same view, asked twice, is the same bytes: the streamed answer the message routes send holds nothing of
/// when it was asked.
#[tokio::test]
async fn two_asks_of_one_view_are_the_same_bytes() {
    let processed = std::sync::Arc::new(FeedResult::default());
    let mut bodies = Vec::new();
    for _ in 0..2 {
        let response =
            stream_messages_response(std::sync::Arc::clone(&processed), None, Vec::new())
                .expect("a response");
        bodies.push(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("the body"),
        );
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert_eq!(bodies[0], bodies[1]);
}
