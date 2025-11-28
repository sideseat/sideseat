//! OpenTelemetry collector for AI agent observability
//!
//! This module provides OTLP ingestion (HTTP + gRPC), SQLite index + Parquet storage,
//! GenAI semantic conventions with extensible framework detection, and real-time SSE updates.
//!
//! ## Architecture
//!
//! - **OtelManager** - Central orchestrator for all OTel components
//! - **Ingestion** - HTTP and gRPC OTLP endpoints with validation
//! - **Normalize** - Framework detection and field extraction
//! - **Storage** - SQLite index + Parquet bulk storage
//! - **Query** - SQLite indexed queries + DataFusion analytics
//! - **Realtime** - SSE subscriptions with filtered events

pub mod error;
pub mod health;

pub mod ingest;
pub mod normalize;
pub mod query;
pub mod realtime;
pub mod schema;
pub mod storage;

mod disk;

pub use error::{OtelError, OtelResult};
pub use health::{OtelHealthStatus, OtelStats};

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, watch};

use sqlx::SqlitePool;

use crate::core::config::OtelConfig;
use crate::core::{DataSubdir, StorageManager};

use self::disk::DiskMonitor;
use self::normalize::{NormalizedSpan, Normalizer};
use self::query::QueryEngine;
use self::realtime::{
    EventBroadcaster, EventPayload, SpanEvent as RtSpanEvent, SseManager, TraceEvent,
};
use self::storage::{RetentionManager, TraceStorageManager, WriteBuffer};

/// Central orchestrator for all OTel components
pub struct OtelManager {
    pub config: OtelConfig,
    pub storage: Arc<TraceStorageManager>,
    pub buffer: Arc<WriteBuffer>,
    pub normalizer: Arc<Normalizer>,
    pub sender: mpsc::Sender<NormalizedSpan>,
    pub query_engine: Arc<QueryEngine>,
    pub sse: Arc<SseManager>,
    pub broadcaster: Arc<EventBroadcaster>,
    pub disk_monitor: Arc<DiskMonitor>,
    pub start_time: Instant,
    shutdown_tx: watch::Sender<bool>,
}

impl OtelManager {
    /// Initialize OtelManager with all components
    ///
    /// # Arguments
    /// * `storage_manager` - Storage manager for directory paths
    /// * `config` - OTel configuration
    /// * `pool` - SQLite connection pool from DatabaseManager
    pub async fn init(
        storage_manager: &StorageManager,
        config: OtelConfig,
        pool: SqlitePool,
    ) -> OtelResult<Self> {
        let data_dir = storage_manager.data_subdir(DataSubdir::Traces);
        tokio::fs::create_dir_all(&data_dir).await?;

        let trace_storage =
            Arc::new(TraceStorageManager::init(data_dir.clone(), &config, pool.clone()).await?);
        let normalizer = Arc::new(Normalizer::new());
        let buffer = Arc::new(WriteBuffer::new(
            config.ingestion.buffer_max_spans,
            config.ingestion.buffer_max_bytes,
        ));
        let query_engine = Arc::new(QueryEngine::new(pool));

        let broadcaster = EventBroadcaster::new(1024);
        let sse = Arc::new(SseManager::new(
            broadcaster.clone(),
            config.sse.max_connections,
            config.sse.timeout_secs,
            config.sse.keepalive_secs,
        ));
        let broadcaster = Arc::new(broadcaster);

        let disk_monitor = Arc::new(DiskMonitor::new(
            data_dir.clone(),
            config.disk.warning_percent,
            config.disk.critical_percent,
        ));

        let (tx, rx) = mpsc::channel(config.ingestion.channel_capacity);

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);

        let manager = Self {
            config,
            storage: trace_storage,
            buffer,
            normalizer,
            sender: tx,
            query_engine,
            sse,
            broadcaster,
            disk_monitor,
            start_time: Instant::now(),
            shutdown_tx,
        };

        manager.spawn_background_tasks(rx);

        tracing::debug!("OtelManager initialized");
        Ok(manager)
    }

    fn spawn_background_tasks(&self, mut rx: mpsc::Receiver<NormalizedSpan>) {
        let buffer = Arc::clone(&self.buffer);
        let storage = Arc::clone(&self.storage);
        let broadcaster = Arc::clone(&self.broadcaster);
        let flush_batch_size = self.config.ingestion.flush_batch_size;
        let attribute_config = self.config.attributes.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        // Ingestion consumer task
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(span) = rx.recv() => {
                        let event = TraceEvent::NewSpan(RtSpanEvent {
                            trace_id: span.trace_id.clone(),
                            span_id: span.span_id.clone(),
                            parent_span_id: span.parent_span_id.clone(),
                            span_name: span.span_name.clone(),
                            service_name: span.service_name.clone(),
                            detected_framework: span.detected_framework.clone(),
                            detected_category: span.detected_category.clone(),
                            start_time_ns: span.start_time_unix_nano,
                            end_time_ns: span.end_time_unix_nano,
                            duration_ns: span.duration_ns,
                            status_code: span.status_code as i32,
                            gen_ai_agent_name: span.gen_ai_agent_name.clone(),
                            gen_ai_tool_name: span.gen_ai_tool_name.clone(),
                            gen_ai_request_model: span.gen_ai_request_model.clone(),
                            usage_input_tokens: span.usage_input_tokens,
                            usage_output_tokens: span.usage_output_tokens,
                        });
                        broadcaster.broadcast(EventPayload::new(event));

                        let should_flush = buffer.push(span).await;
                        if should_flush || buffer.count() >= flush_batch_size {
                            let spans = buffer.drain().await;
                            if !spans.is_empty() {
                                Self::flush_with_retry(&storage, spans, &attribute_config).await;
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // Periodic flush task - ensures data is persisted even during low traffic
        let buffer = Arc::clone(&self.buffer);
        let storage = Arc::clone(&self.storage);
        let flush_interval =
            std::time::Duration::from_millis(self.config.ingestion.flush_interval_ms);
        let attribute_config = self.config.attributes.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(flush_interval);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Use atomic drain_if_not_empty to prevent race with ingestion consumer
                        if let Some(spans) = buffer.drain_if_not_empty().await {
                            Self::flush_with_retry(&storage, spans, &attribute_config).await;
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // Disk usage monitoring
        let disk_monitor = Arc::clone(&self.disk_monitor);
        let shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move { disk_monitor.run(shutdown_rx).await });

        // Storage retention (FIFO cleanup when disk limits exceeded)
        let retention_days = self.config.retention.days;
        let max_mb = self.config.retention.max_mb;
        let retention_check_interval = self.config.retention.check_interval_secs;
        let retention_pool = self.storage.pool().clone();
        let shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            let retention = Arc::new(RetentionManager::new(
                max_mb,
                retention_days,
                retention_check_interval,
                retention_pool,
            ));
            retention.run(shutdown_rx).await;
        });

        // Parquet writer pool cleanup (remove old day writers from memory)
        let parquet_pool = Arc::clone(self.storage.parquet_pool());
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            // Cleanup every hour, keep writers for last 7 days
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        parquet_pool.cleanup_old_writers(7).await;
                        tracing::debug!("Cleaned up old parquet writers");
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // SSE connection cleanup
        let sse = Arc::clone(&self.sse);
        sse.start_cleanup_task();

        // WAL checkpoints are managed by DatabaseManager at the server level
    }

    /// Flush spans with exponential backoff retry on failure
    /// Retries up to 3 times with delays of 100ms, 500ms, 2s
    async fn flush_with_retry(
        storage: &TraceStorageManager,
        spans: Vec<NormalizedSpan>,
        attribute_config: &crate::core::config::OtelAttributeConfig,
    ) {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let span_count = spans.len();
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            match Self::flush_spans(storage, spans.clone(), attribute_config).await {
                Ok(()) => return,
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES - 1 {
                        // Exponential backoff: 100ms, 500ms, 2500ms
                        let delay_ms = BASE_DELAY_MS * 5u64.pow(attempt);
                        tracing::warn!(
                            "Flush attempt {} failed, retrying in {}ms: {}",
                            attempt + 1,
                            delay_ms,
                            last_error.as_ref().unwrap()
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        // All retries exhausted - log error with span count for visibility
        if let Some(e) = last_error {
            tracing::error!(
                "Flush failed after {} retries, {} spans lost: {}",
                MAX_RETRIES,
                span_count,
                e
            );
        }
    }

    /// Flush spans to SQLite and Parquet atomically.
    /// Write order: Parquet first, then SQLite. If SQLite fails, delete parquet files.
    /// This ensures no orphan parquet files exist.
    async fn flush_spans(
        storage: &TraceStorageManager,
        spans: Vec<NormalizedSpan>,
        attribute_config: &crate::core::config::OtelAttributeConfig,
    ) -> OtelResult<()> {
        if spans.is_empty() {
            return Ok(());
        }

        let pool = storage.pool();
        let attr_cache = storage.attribute_cache();

        // Step 1: Write to parquet first (before SQLite transaction)
        let written_files = storage.parquet_pool().write_batch(&spans).await?;

        // Step 2: Single SQLite transaction for all operations
        // If this fails, we clean up parquet files
        let sqlite_result = async {
            let mut tx = pool.begin().await.map_err(|e| {
                OtelError::StorageError(format!("Failed to begin transaction: {}", e))
            })?;

            // Upsert sessions first (if spans have session_id)
            storage::sqlite::sessions::upsert_sessions_batch_with_tx(&mut tx, &spans).await?;

            // Upsert traces (spans have FK to traces)
            storage::sqlite::traces::upsert_traces_batch_with_tx(&mut tx, &spans).await?;

            // Batch insert all spans
            storage::sqlite::spans::insert_spans_batch_with_tx(&mut tx, &spans).await?;

            // Register parquet files and link spans (inside same transaction)
            for file in &written_files {
                let file_path_str = file.path.to_str().unwrap_or("");

                // Register parquet file metadata
                storage::sqlite::files::register_file_with_tx(
                    &mut tx,
                    file_path_str,
                    &file.date_partition,
                    file.span_count as i32,
                    file.file_size as i64,
                    file.min_start_time,
                    file.max_end_time,
                )
                .await?;

                // Update spans with their parquet file reference
                storage::sqlite::spans::update_spans_parquet_file_with_tx(
                    &mut tx,
                    file_path_str,
                    file.min_start_time,
                    file.max_end_time,
                )
                .await?;
            }

            // Extract and store EAV attributes (inside same transaction)
            Self::extract_and_store_attributes_with_tx(
                &mut tx,
                attr_cache,
                &spans,
                attribute_config,
            )
            .await?;

            tx.commit().await.map_err(|e| {
                OtelError::StorageError(format!("Failed to commit transaction: {}", e))
            })?;

            Ok::<(), OtelError>(())
        }
        .await;

        // Step 3: If SQLite failed, delete the parquet files we just wrote
        if let Err(e) = sqlite_result {
            tracing::error!("SQLite transaction failed, cleaning up parquet files: {}", e);
            for file in &written_files {
                if let Err(del_err) = tokio::fs::remove_file(&file.path).await {
                    tracing::warn!("Failed to cleanup parquet file {:?}: {}", file.path, del_err);
                }
            }
            return Err(e);
        }

        Ok(())
    }

    /// Extract attributes from spans and store them in EAV tables (within a transaction)
    async fn extract_and_store_attributes_with_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        attr_cache: &std::sync::Arc<storage::sqlite::AttributeKeyCache>,
        spans: &[NormalizedSpan],
        config: &crate::core::config::OtelAttributeConfig,
    ) -> OtelResult<()> {
        use std::collections::{HashMap, HashSet};
        use storage::sqlite::{
            AttributeValue, insert_span_attributes_batch_with_tx,
            insert_trace_attributes_batch_with_tx,
        };

        // Type alias for EAV attribute values: (key_id, string_value, numeric_value)
        type AttrValues = Vec<(i64, Option<String>, Option<f64>)>;

        let mut trace_attrs: HashMap<String, AttrValues> = HashMap::new();
        let mut span_attrs: Vec<(String, i64, Option<String>, Option<f64>)> = Vec::new();
        let mut processed_traces: HashSet<String> = HashSet::new();

        // Build set of configured attributes for faster lookup
        let trace_attr_set: HashSet<&str> =
            config.trace_attributes.iter().map(|s| s.as_str()).collect();
        let span_attr_set: HashSet<&str> =
            config.span_attributes.iter().map(|s| s.as_str()).collect();

        for span in spans {
            // Parse span attributes
            let span_attributes: serde_json::Value =
                serde_json::from_str(&span.attributes_json).unwrap_or(serde_json::Value::Null);

            // Parse resource attributes
            let resource_attributes: serde_json::Value = span
                .resource_attributes_json
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::Value::Null);

            // Extract trace-level attributes (only once per trace)
            if !processed_traces.contains(&span.trace_id) {
                processed_traces.insert(span.trace_id.clone());

                // Extract from resource attributes
                if let serde_json::Value::Object(ref map) = resource_attributes {
                    for (key, value) in map {
                        let should_index = trace_attr_set.contains(key.as_str())
                            || (config.auto_index_genai && key.starts_with("gen_ai."));

                        if should_index {
                            // Key creation uses separate connection (idempotent operation)
                            let key_id = attr_cache
                                .get_or_create_key_with_tx(tx, key, "string", "trace")
                                .await?;
                            let attr_value = AttributeValue::from_json(value);
                            let (str_val, num_val) = attr_value.to_eav_values();
                            trace_attrs
                                .entry(span.trace_id.clone())
                                .or_default()
                                .push((key_id, str_val, num_val));
                        }
                    }
                }

                // Extract from span attributes (for trace-level indexing)
                if let serde_json::Value::Object(ref map) = span_attributes {
                    for (key, value) in map {
                        if trace_attr_set.contains(key.as_str()) {
                            let key_id = attr_cache
                                .get_or_create_key_with_tx(tx, key, "string", "trace")
                                .await?;
                            let attr_value = AttributeValue::from_json(value);
                            let (str_val, num_val) = attr_value.to_eav_values();
                            trace_attrs
                                .entry(span.trace_id.clone())
                                .or_default()
                                .push((key_id, str_val, num_val));
                        }
                    }
                }
            }

            // Extract span-level attributes
            if let serde_json::Value::Object(ref map) = span_attributes {
                for (key, value) in map {
                    let should_index = span_attr_set.contains(key.as_str())
                        || (config.auto_index_genai && key.starts_with("gen_ai."));

                    if should_index {
                        let key_id =
                            attr_cache.get_or_create_key_with_tx(tx, key, "string", "span").await?;
                        let attr_value = AttributeValue::from_json(value);
                        let (str_val, num_val) = attr_value.to_eav_values();
                        span_attrs.push((span.span_id.clone(), key_id, str_val, num_val));
                    }
                }
            }
        }

        // Batch insert trace attributes
        let trace_attrs_flat: Vec<(String, i64, Option<String>, Option<f64>)> = trace_attrs
            .into_iter()
            .flat_map(|(trace_id, attrs)| {
                attrs.into_iter().map(move |(key_id, str_val, num_val)| {
                    (trace_id.clone(), key_id, str_val, num_val)
                })
            })
            .collect();

        if !trace_attrs_flat.is_empty() {
            insert_trace_attributes_batch_with_tx(tx, &trace_attrs_flat).await?;
        }

        // Batch insert span attributes
        if !span_attrs.is_empty() {
            insert_span_attributes_batch_with_tx(tx, &span_attrs).await?;
        }

        Ok(())
    }

    /// Get a sender for ingesting spans
    pub fn sender(&self) -> mpsc::Sender<NormalizedSpan> {
        self.sender.clone()
    }

    /// Graceful shutdown - flushes pending data and stops background tasks
    pub async fn shutdown(&self) -> OtelResult<()> {
        tracing::debug!("Shutting down OtelManager...");

        let _ = self.shutdown_tx.send(true);

        let spans = self.buffer.drain().await;
        if !spans.is_empty() {
            Self::flush_spans(&self.storage, spans, &self.config.attributes).await?;
        }

        let _ = self.storage.parquet_pool().flush_all().await;

        // WAL checkpoint on shutdown is handled by DatabaseManager

        tracing::debug!("OtelManager shutdown complete");
        Ok(())
    }

    /// Get health status for the OTel subsystem
    pub async fn health_status(&self) -> health::OtelHealthStatus {
        let disk_usage = self.disk_monitor.get_usage_percent().unwrap_or(0);

        let status = if self.disk_monitor.should_pause_ingestion() {
            health::HealthState::Unhealthy
        } else if disk_usage >= self.config.disk.warning_percent {
            health::HealthState::Degraded
        } else {
            health::HealthState::Healthy
        };

        health::OtelHealthStatus {
            enabled: self.config.enabled,
            status,
            components: health::OtelComponentStatus {
                http_collector: health::ComponentHealth {
                    status: health::HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                grpc_collector: health::ComponentHealth {
                    status: if self.config.grpc.enabled {
                        health::HealthState::Healthy
                    } else {
                        health::HealthState::Unhealthy
                    },
                    message: if self.config.grpc.enabled {
                        None
                    } else {
                        Some("gRPC disabled".to_string())
                    },
                    last_activity: None,
                },
                sqlite: health::ComponentHealth {
                    status: health::HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                parquet_writer: health::ComponentHealth {
                    status: health::HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                sse_manager: health::ComponentHealth {
                    status: health::HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                retention_manager: health::ComponentHealth {
                    status: health::HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
            },
            stats: {
                // Query storage stats from SQLite
                let storage_stats = storage::sqlite::get_storage_stats(self.storage.pool())
                    .await
                    .unwrap_or_default();
                health::OtelStats {
                    total_traces: storage_stats.total_traces,
                    total_spans: storage_stats.total_spans,
                    storage_bytes: storage_stats.total_parquet_bytes,
                    storage_files: storage_stats.total_parquet_files,
                    disk_usage_percent: disk_usage,
                    buffer_size: self.buffer.count(),
                    buffer_capacity: self.config.ingestion.buffer_max_spans,
                    sse_connections: self.sse.subscription_count().await as u64,
                    uptime_seconds: self.start_time.elapsed().as_secs(),
                }
            },
        }
    }
}
