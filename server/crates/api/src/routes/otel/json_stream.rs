//! Bounded-memory JSON response construction for large message arrays.
//!
//! The reconstruction pipeline necessarily owns its result, but `Json<T>` would then serialize the whole
//! response into a second contiguous buffer. This writer emits the array one item at a time and keeps the
//! remaining, small object fields as separately serialized fragments.

use axum::body::Body;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

/// Stream one top-level array followed by already-serialized object fields.
pub fn object_array(
    array_field: &'static str,
    len: usize,
    mut item_json: impl FnMut(usize) -> serde_json::Result<String> + Send + 'static,
    trailing_fields: Vec<(&'static str, String)>,
) -> Response {
    let stream = async_stream::stream! {
        let array_field = match serde_json::to_string(array_field) {
            Ok(value) => value,
            Err(error) => {
                yield Err::<String, std::io::Error>(std::io::Error::other(error));
                return;
            }
        };
        yield Ok::<String, std::io::Error>(format!("{{{array_field}:["));
        for index in 0..len {
            if index != 0 {
                yield Ok(",".to_string());
            }
            match item_json(index) {
                Ok(item) => yield Ok(item),
                Err(error) => {
                    yield Err(std::io::Error::other(error));
                    return;
                }
            }
        }
        yield Ok("]".to_string());
        for (name, value) in trailing_fields {
            match serde_json::to_string(name) {
                Ok(name) => yield Ok(format!(",{name}:{value}")),
                Err(error) => {
                    yield Err(std::io::Error::other(error));
                    return;
                }
            }
        }
        yield Ok("}".to_string());
    };

    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        Body::from_stream(stream),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn streamed_object_is_valid_json_without_a_content_length() {
        let response = object_array(
            "items",
            3,
            |index| serde_json::to_string(&format!("item-{index}")),
            vec![("metadata", serde_json::json!({"count": 3}).to_string())],
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );
        assert!(response.headers().get(header::CONTENT_LENGTH).is_none());

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "items": ["item-0", "item-1", "item-2"],
                "metadata": {"count": 3}
            })
        );
    }
}
