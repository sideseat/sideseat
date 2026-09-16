//! Debug mode helper for writing OTLP data to JSON lines files

use std::path::Path;
use std::sync::LazyLock;

use chrono::Utc;
use serde::Serialize;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// Global mutex to prevent interleaved writes from concurrent requests.
/// A single mutex is sufficient since debug mode has only 3 files and is for development only.
static WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Debug entry wrapper with metadata
#[derive(Serialize)]
struct DebugEntry<T: Serialize> {
    timestamp: String,
    project_id: String,
    data: T,
}

/// Write OTLP data to a JSON lines debug file.
/// This is fire-and-forget - errors are logged but don't fail the request.
/// Uses a mutex to prevent interleaved writes from concurrent requests.
pub async fn write_debug<T: Serialize>(
    debug_path: &Path,
    filename: &str,
    project_id: &str,
    data: &T,
) {
    let file_path = debug_path.join(filename);
    let entry = DebugEntry {
        timestamp: Utc::now().to_rfc3339(),
        project_id: project_id.to_string(),
        data,
    };

    let json = match serde_json::to_string(&entry) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, filename, "Failed to serialize debug entry");
            return;
        }
    };

    // Serialize file access to prevent interleaved writes
    let _guard = WRITE_LOCK.lock().await;

    let result = async {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?;
        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok::<_, std::io::Error>(())
    }
    .await;

    if let Err(e) = result {
        tracing::warn!(
            error = %e,
            path = %file_path.display(),
            "Failed to write debug entry"
        );
    }
}
