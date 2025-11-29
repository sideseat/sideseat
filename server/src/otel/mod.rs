//! OpenTelemetry collector for AI agent observability
//!
//! This module provides OTLP ingestion (HTTP + gRPC), SQLite storage,
//! GenAI semantic conventions with extensible framework detection, and real-time SSE updates.
//!
//! ## Architecture
//!
//! - **OtelManager** - Central orchestrator for all OTel components
//! - **Ingestion** - HTTP and gRPC OTLP endpoints with validation
//! - **Normalize** - Framework detection and field extraction
//! - **Storage** - SQLite for all span data
//! - **Query** - SQLite queries and aggregations
//! - **Realtime** - SSE subscriptions with filtered events

pub mod error;
pub mod health;

pub mod ingest;
pub mod normalize;
pub mod query;
pub mod realtime;
pub mod storage;

pub use error::{OtelError, OtelResult};
pub use health::{OtelHealthStatus, OtelStats};

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, watch};

use crate::core::config::OtelConfig;
use sqlx::SqlitePool;

use self::normalize::{NormalizedSpan, Normalizer};
use self::query::QueryEngine;
use self::realtime::{
    EventBroadcaster, EventPayload, SpanEvent as RtSpanEvent, SseManager, TraceEvent,
};
use self::storage::sqlite::AttributeKeyCache;
use self::storage::{RetentionManager, WriteBuffer};

/// Central orchestrator for all OTel components
pub struct OtelManager {
    pub config: OtelConfig,
    pub pool: SqlitePool,
    attribute_cache: Arc<AttributeKeyCache>,
    pub buffer: Arc<WriteBuffer>,
    pub normalizer: Arc<Normalizer>,
    pub sender: mpsc::Sender<NormalizedSpan>,
    pub query_engine: Arc<QueryEngine>,
    pub sse: Arc<SseManager>,
    pub broadcaster: Arc<EventBroadcaster>,
    pub start_time: Instant,
    shutdown_tx: watch::Sender<bool>,
}

impl OtelManager {
    /// Initialize OtelManager with all components
    ///
    /// # Arguments
    /// * `config` - OTel configuration
    /// * `pool` - SQLite connection pool from DatabaseManager
    pub async fn init(config: OtelConfig, pool: SqlitePool) -> OtelResult<Self> {
        // Initialize and load attribute key cache
        let attribute_cache = Arc::new(AttributeKeyCache::new());
        attribute_cache.load_from_db(&pool).await?;
        tracing::debug!("Loaded attribute key cache from database");

        let normalizer = Arc::new(Normalizer::new());
        let buffer = Arc::new(WriteBuffer::new(
            config.ingestion.buffer_max_spans,
            config.ingestion.buffer_max_bytes,
        ));
        let query_engine = Arc::new(QueryEngine::new(pool.clone()));

        let broadcaster = EventBroadcaster::new(1024);
        let sse = Arc::new(SseManager::new(
            broadcaster.clone(),
            config.sse.max_connections,
            config.sse.timeout_secs,
            config.sse.keepalive_secs,
        ));
        let broadcaster = Arc::new(broadcaster);

        let (tx, rx) = mpsc::channel(config.ingestion.channel_capacity);

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);

        let manager = Self {
            config,
            pool,
            attribute_cache,
            buffer,
            normalizer,
            sender: tx,
            query_engine,
            sse,
            broadcaster,
            start_time: Instant::now(),
            shutdown_tx,
        };

        manager.spawn_background_tasks(rx);

        tracing::debug!("OtelManager initialized");
        Ok(manager)
    }

    fn spawn_background_tasks(&self, mut rx: mpsc::Receiver<NormalizedSpan>) {
        let buffer = Arc::clone(&self.buffer);
        let pool = self.pool.clone();
        let attr_cache = Arc::clone(&self.attribute_cache);
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
                                Self::flush_with_retry(&pool, &attr_cache, spans, &attribute_config).await;
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
        let pool = self.pool.clone();
        let attr_cache = Arc::clone(&self.attribute_cache);
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
                            Self::flush_with_retry(&pool, &attr_cache, spans, &attribute_config).await;
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

        // Storage retention (time-based cleanup)
        let retention_days = self.config.retention.days;
        let retention_check_interval = self.config.retention.check_interval_secs;
        let retention_pool = self.pool.clone();
        let shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            let retention = Arc::new(RetentionManager::new(
                retention_pool,
                retention_days,
                retention_check_interval,
            ));
            retention.run(shutdown_rx).await;
        });

        // SSE connection cleanup
        let sse = Arc::clone(&self.sse);
        sse.start_cleanup_task();

        // WAL checkpoints are managed by DatabaseManager at the server level
    }

    /// Flush spans with exponential backoff retry on failure
    /// Retries up to 3 times with delays of 100ms, 500ms, 2s
    async fn flush_with_retry(
        pool: &SqlitePool,
        attr_cache: &Arc<AttributeKeyCache>,
        spans: Vec<NormalizedSpan>,
        attribute_config: &crate::core::config::OtelAttributeConfig,
    ) {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let span_count = spans.len();
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            match Self::flush_spans(pool, attr_cache, spans.clone(), attribute_config).await {
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

    /// Flush spans to SQLite
    async fn flush_spans(
        pool: &SqlitePool,
        attr_cache: &Arc<AttributeKeyCache>,
        spans: Vec<NormalizedSpan>,
        attribute_config: &crate::core::config::OtelAttributeConfig,
    ) -> OtelResult<()> {
        if spans.is_empty() {
            return Ok(());
        }

        let mut tx = pool
            .begin()
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

        // Upsert sessions first (if spans have session_id)
        storage::sqlite::sessions::upsert_sessions_batch_with_tx(&mut tx, &spans).await?;

        // Upsert traces (spans have FK to traces)
        storage::sqlite::traces::upsert_traces_batch_with_tx(&mut tx, &spans).await?;

        // Batch insert all spans
        storage::sqlite::spans::insert_spans_batch_with_tx(&mut tx, &spans).await?;

        // Batch insert span events
        let all_events: Vec<_> = spans.iter().flat_map(|s| s.events.iter().cloned()).collect();
        if !all_events.is_empty() {
            storage::sqlite::insert_events_batch_with_tx(&mut tx, &all_events).await?;
        }

        // Extract and store EAV attributes (inside same transaction)
        Self::extract_and_store_attributes_with_tx(&mut tx, attr_cache, &spans, attribute_config)
            .await?;

        tx.commit()
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

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

    /// Get the attribute key cache
    pub fn attribute_cache(&self) -> &Arc<AttributeKeyCache> {
        &self.attribute_cache
    }

    /// Graceful shutdown - flushes pending data and stops background tasks
    pub async fn shutdown(&self) -> OtelResult<()> {
        tracing::debug!("Shutting down OtelManager...");

        let _ = self.shutdown_tx.send(true);

        let spans = self.buffer.drain().await;
        if !spans.is_empty() {
            Self::flush_spans(&self.pool, &self.attribute_cache, spans, &self.config.attributes)
                .await?;
        }

        // WAL checkpoint on shutdown is handled by DatabaseManager

        tracing::debug!("OtelManager shutdown complete");
        Ok(())
    }

    /// Get health status for the OTel subsystem
    pub async fn health_status(&self) -> health::OtelHealthStatus {
        health::OtelHealthStatus {
            enabled: self.config.enabled,
            status: health::HealthState::Healthy,
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
                let storage_stats =
                    storage::sqlite::get_storage_stats(&self.pool).await.unwrap_or_default();
                health::OtelStats {
                    total_traces: storage_stats.total_traces,
                    total_spans: storage_stats.total_spans,
                    buffer_size: self.buffer.count(),
                    buffer_capacity: self.config.ingestion.buffer_max_spans,
                    sse_connections: self.sse.subscription_count().await as u64,
                    uptime_seconds: self.start_time.elapsed().as_secs(),
                }
            },
        }
    }
}
