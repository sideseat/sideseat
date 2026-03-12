use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use reqwest::Response;

use crate::error::ProviderError;

/// Convert a reqwest `Response` into a stream of SSE `data` strings.
/// Filters out `[DONE]` terminators and empty data lines.
pub(crate) fn sse_data_stream(
    response: Response,
) -> impl Stream<Item = Result<String, ProviderError>> {
    response
        .bytes_stream()
        .eventsource()
        .filter_map(|result| async move {
            match result {
                Ok(event) => {
                    let data = event.data;
                    if data.is_empty() || data == "[DONE]" {
                        None
                    } else {
                        Some(Ok(data))
                    }
                }
                Err(e) => Some(Err(ProviderError::Stream(e.to_string()))),
            }
        })
}

/// Check an HTTP response for error status and return an appropriate `ProviderError`.
pub(crate) async fn check_response(response: Response) -> Result<Response, ProviderError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let status_code = status.as_u16();

    if status_code == 429 {
        let retry_after_secs = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::TooManyRequests {
            message: body,
            retry_after_secs,
        });
    }

    let body = response.text().await.unwrap_or_default();

    // Detect context window errors
    let lower = body.to_lowercase();
    if lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("input is too long")
        || lower.contains("maximum context length")
    {
        return Err(ProviderError::ContextWindowExceeded(body));
    }

    // 404 on a model endpoint means the model doesn't exist (wrong name, not enabled,
    // or not yet available in the selected region/tier). Surface as ModelNotFound so
    // callers like try_test_provider can treat it as an auth-success signal.
    if status_code == 404 {
        return Err(ProviderError::ModelNotFound { model: body });
    }

    Err(ProviderError::Api {
        status: status_code,
        message: body,
    })
}
