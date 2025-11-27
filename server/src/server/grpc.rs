//! gRPC OTLP server

use crate::otel::OtelManager;
use crate::otel::ingest::OtlpTraceService;
use crate::otel::normalize::Normalizer;
use crate::{Error, Result};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::TraceServiceServer;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tonic::transport::Server as TonicServer;

/// Start the gRPC OTLP server
pub async fn start_grpc_server(otel: Arc<OtelManager>, addr: &str) -> Result<JoinHandle<()>> {
    let addr = addr.parse().map_err(|e| Error::Config(format!("Invalid gRPC address: {}", e)))?;

    tracing::debug!("Starting gRPC server on {}", addr);

    let normalizer = Arc::new(Normalizer::new());
    let trace_service = OtlpTraceService::new(otel.sender(), normalizer);

    let handle = tokio::spawn(async move {
        if let Err(e) = TonicServer::builder()
            .add_service(TraceServiceServer::new(trace_service))
            .serve(addr)
            .await
        {
            tracing::error!("gRPC server error: {}", e);
        }
    });

    Ok(handle)
}
