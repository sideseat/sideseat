//! HTTP request handlers

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// Handle 404 Not Found requests with detailed logging
pub async fn handle_404(req: Request) -> impl IntoResponse {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();

    // Extract the body
    let body_bytes = match to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            println!("\x1b[31m[404]\x1b[0m Failed to read request body");
            return StatusCode::NOT_FOUND;
        }
    };

    // Build URL string
    let url = uri.to_string();

    // Convert headers to JSON object
    let mut headers_map = serde_json::Map::new();
    for (name, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            headers_map.insert(name.to_string(), serde_json::Value::String(value_str.to_string()));
        }
    }

    // Parse body as JSON or text
    let body_value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(json) => json,
            Err(_) => {
                // Not valid JSON, try as text
                match String::from_utf8(body_bytes.to_vec()) {
                    Ok(text) => serde_json::Value::String(text),
                    Err(_) => {
                        serde_json::Value::String(format!("<binary {} bytes>", body_bytes.len()))
                    }
                }
            }
        }
    };

    // Create the complete JSON object
    let log_entry = serde_json::json!({
        "status": 404,
        "method": method.to_string(),
        "url": url,
        "headers": headers_map,
        "body": body_value,
    });

    // Print as pretty JSON
    println!("\x1b[31m[404]\x1b[0m");
    if let Ok(pretty) = serde_json::to_string_pretty(&log_entry) {
        println!("{}", pretty);
    }

    StatusCode::NOT_FOUND
}
