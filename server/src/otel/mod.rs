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
    pub async fn init(storage_manager: &StorageManager, config: OtelConfig) -> OtelResult<Self> {
        let data_dir = storage_manager.data_subdir(DataSubdir::Traces);
        tokio::fs::create_dir_all(&data_dir).await?;

        let trace_storage = Arc::new(TraceStorageManager::init(data_dir.clone(), &config).await?);
        let normalizer = Arc::new(Normalizer::new());
        let buffer = Arc::new(WriteBuffer::new(config.buffer_max_spans, config.buffer_max_bytes));
        let query_engine = Arc::new(QueryEngine::new(trace_storage.sqlite().pool().clone()));

        let broadcaster = EventBroadcaster::new(1024);
        let sse = Arc::new(SseManager::new(
            broadcaster.clone(),
            config.sse_max_connections,
            config.sse_timeout_secs,
            config.sse_keepalive_secs,
        ));
        let broadcaster = Arc::new(broadcaster);

        let disk_monitor = Arc::new(DiskMonitor::new(
            data_dir.clone(),
            config.disk_warning_percent,
            config.disk_critical_percent,
        ));

        let (tx, rx) = mpsc::channel(config.channel_capacity);

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
        let flush_batch_size = self.config.flush_batch_size;
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
                            status_code: span.status_code,
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
                            if !spans.is_empty()
                                && let Err(e) = Self::flush_spans(&storage, spans).await {
                                    tracing::error!("Flush error: {}", e);
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
        let flush_interval = std::time::Duration::from_millis(self.config.flush_interval_ms);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(flush_interval);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if !buffer.is_empty() {
                            let spans = buffer.drain().await;
                            if !spans.is_empty()
                                && let Err(e) = Self::flush_spans(&storage, spans).await {
                                    tracing::error!("Periodic flush error: {}", e);
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

        // Disk usage monitoring
        let disk_monitor = Arc::clone(&self.disk_monitor);
        let shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move { disk_monitor.run(shutdown_rx).await });

        // Storage retention (FIFO cleanup when disk limits exceeded)
        let traces_dir = self.storage.traces_dir().clone();
        let retention_days = self.config.retention_days;
        let max_gb = self.config.retention_max_gb;
        let retention_check_interval = self.config.retention_check_interval_secs;
        let shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            let retention = Arc::new(RetentionManager::new(
                traces_dir,
                max_gb,
                retention_days,
                retention_check_interval,
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
    }

    async fn flush_spans(
        storage: &TraceStorageManager,
        spans: Vec<NormalizedSpan>,
    ) -> OtelResult<()> {
        if spans.is_empty() {
            return Ok(());
        }

        // Insert into SQLite first (uses references)
        let pool = storage.sqlite().pool();
        for span in &spans {
            // Upsert trace first (spans have FK to traces)
            storage::sqlite::traces::upsert_trace(pool, span).await?;
            // Note: parquet_file will be updated after write completes
            storage::sqlite::spans::insert_span(pool, span, None).await?;
        }

        // Write to parquet (takes ownership to avoid cloning)
        let written_files = storage.parquet_pool().write_batch(spans).await?;

        // Register written files
        for file in written_files {
            storage::sqlite::files::register_file(
                pool,
                file.path.to_str().unwrap_or(""),
                &file.date_partition,
                file.span_count as i32,
                file.file_size as i64,
                file.min_start_time,
                file.max_end_time,
            )
            .await?;
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
            Self::flush_spans(&self.storage, spans).await?;
        }

        let _ = self.storage.parquet_pool().flush_all().await;

        tracing::debug!("OtelManager shutdown complete");
        Ok(())
    }

    /// Get health status for the OTel subsystem
    pub async fn health_status(&self) -> health::OtelHealthStatus {
        let disk_usage = self.disk_monitor.get_usage_percent().unwrap_or(0);

        let status = if self.disk_monitor.should_pause_ingestion() {
            health::HealthState::Unhealthy
        } else if disk_usage >= self.config.disk_warning_percent {
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
                    status: if self.config.grpc_enabled {
                        health::HealthState::Healthy
                    } else {
                        health::HealthState::Unhealthy
                    },
                    message: if self.config.grpc_enabled {
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
                let storage_stats = self.storage.sqlite().get_stats().await.unwrap_or_default();
                health::OtelStats {
                    total_traces: storage_stats.total_traces,
                    total_spans: storage_stats.total_spans,
                    storage_bytes: storage_stats.total_parquet_bytes,
                    storage_files: storage_stats.total_parquet_files,
                    disk_usage_percent: disk_usage,
                    buffer_size: self.buffer.count(),
                    buffer_capacity: self.config.buffer_max_spans,
                    sse_connections: self.sse.subscription_count().await as u64,
                    uptime_seconds: self.start_time.elapsed().as_secs(),
                }
            },
        }
    }
}
