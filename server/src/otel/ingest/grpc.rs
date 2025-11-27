//! gRPC OTLP services

use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse, trace_service_server::TraceService,
};

use super::converter::convert_otlp_spans;
use crate::otel::normalize::{NormalizedSpan, Normalizer};

/// gRPC trace service implementation
pub struct OtlpTraceService {
    sender: mpsc::Sender<NormalizedSpan>,
    normalizer: Arc<Normalizer>,
}

impl OtlpTraceService {
    pub fn new(sender: mpsc::Sender<NormalizedSpan>, normalizer: Arc<Normalizer>) -> Self {
        Self { sender, normalizer }
    }
}

#[tonic::async_trait]
impl TraceService for OtlpTraceService {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let req = request.into_inner();

        // Convert OTLP spans to normalized spans
        let spans = convert_otlp_spans(req, self.normalizer.detector_registry());

        // Send to ingestion channel
        for span in spans {
            if self.sender.send(span).await.is_err() {
                return Err(Status::unavailable("Ingestion channel full"));
            }
        }

        Ok(Response::new(ExportTraceServiceResponse { partial_success: None }))
    }
}
