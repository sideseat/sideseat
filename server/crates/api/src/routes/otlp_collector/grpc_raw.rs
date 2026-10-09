//! A gRPC codec that keeps the bytes the exporter sent, and the server plumbing to use it.
//!
//! Raw telemetry is the authority every derived row is rebuilt from, so it has to be the producer's bytes.
//! Over HTTP that is the request body. Over gRPC it was not: tonic's generated service hands over a decoded
//! message, so the stored record was prost's re-encoding of it - the same bytes for the canonical encoders
//! exporters use, and *not* the same for a non-minimal varint, an unusual field order, or any field prost does
//! not know, which is exactly the telemetry a parsing defect would later need to be re-read from.
//!
//! So the decoder keeps the frame. [`RawCodec`] decodes into [`Received<T>`]: the message **and** the bytes it
//! was decoded from, which is the gRPC frame's payload after tonic has removed any content encoding - the same
//! point in the pipeline as an HTTP body after `Content-Encoding`. Nothing re-encodes.
//!
//! tonic's generated servers hardwire `ProstCodec`, so [`RawExport`] is the generated dispatch for a one-method
//! service with the codec swapped: the same `tonic::server::Grpc::new(codec).unary(...)` call, the same
//! compression and message-size configuration, routed by the service's own path.

use std::marker::PhantomData;
use std::sync::Arc;
use std::task::{Context, Poll};

// `prost::bytes` rather than a direct dependency: it is the same `bytes` crate tonic's buffers are built on,
// so the frame needs no copy to become one.
use prost::Message;
use prost::bytes::{Buf, Bytes};
use tonic::codec::{BufferSettings, Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::{BoxFuture, Service, StdError, empty_body, http};
use tonic::server::{Grpc, NamedService, UnaryService};
use tonic::{Request, Response, Status};

use futures::StreamExt;

use super::admission::IngestAdmission;

/// A decoded message together with the bytes it was decoded from.
#[derive(Debug, Clone)]
pub struct Received<T> {
    pub message: T,
    /// The frame's payload, after any content encoding: what the producer sent.
    pub raw: Bytes,
}

/// A [`Codec`] that encodes responses with prost and decodes requests into [`Received`].
#[derive(Debug)]
pub struct RawCodec<T, U> {
    _pd: PhantomData<(T, U)>,
}

impl<T, U> Default for RawCodec<T, U> {
    fn default() -> Self {
        Self { _pd: PhantomData }
    }
}

impl<T, U> Codec for RawCodec<T, U>
where
    T: Message + Send + 'static,
    U: Message + Default + Send + 'static,
{
    type Encode = T;
    type Decode = Received<U>;
    type Encoder = RawEncoder<T>;
    type Decoder = RawDecoder<U>;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder(PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder(PhantomData)
    }
}

/// Responses are encoded exactly as tonic's own codec encodes them.
#[derive(Debug)]
pub struct RawEncoder<T>(PhantomData<T>);

impl<T: Message> Encoder for RawEncoder<T> {
    type Item = T;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, buf: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        // The buffer grows as needed, so the only failure prost reports here cannot happen; reported rather
        // than asserted, because an encoder that panics takes the connection with it.
        item.encode(buf).map_err(|error| {
            Status::internal(format!("failed to encode the export response: {error}"))
        })
    }

    fn buffer_settings(&self) -> BufferSettings {
        BufferSettings::default()
    }
}

/// Keeps the frame, then decodes from the copy.
#[derive(Debug)]
pub struct RawDecoder<U>(PhantomData<U>);

impl<U: Message + Default> Decoder for RawDecoder<U> {
    type Item = Received<U>;
    type Error = Status;

    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        // tonic limits the buffer to one message, so this is that message's bytes and no more. `Bytes` is
        // reference-counted, so the copy here is the only one: the decode below reads it without copying again,
        // and staging stores it without copying again.
        let raw = buf.copy_to_bytes(buf.remaining());
        let message = U::decode(raw.clone())
            .map_err(|error| Status::internal(format!("failed to decode the export: {error}")))?;
        Ok(Some(Received { message, raw }))
    }

    fn buffer_settings(&self) -> BufferSettings {
        BufferSettings::default()
    }
}

/// One export call, as the service implements it.
pub trait RawExportHandler: Send + Sync + 'static {
    /// The export request message.
    type Request: Message + Default + Send + 'static;
    /// The export response message.
    type Response: Message + Send + 'static;
    /// The full gRPC path this handler answers, as the generated server spells it.
    const PATH: &'static str;
    /// The service name, for tonic's routing.
    const SERVICE: &'static str;

    fn export(
        self: Arc<Self>,
        request: Request<Received<Self::Request>>,
    ) -> BoxFuture<Response<Self::Response>, Status>;
}

/// A one-method gRPC service over [`RawCodec`].
///
/// What `tonic-build` generates for a unary method, with the codec replaced and the method list reduced to the
/// one OTLP service each handler answers. `max_*_message_size` is applied exactly as the generated server
/// applies it, so a payload over the limit is refused at the same point.
pub struct RawExport<H> {
    inner: Arc<H>,
    max_decoding_message_size: Option<usize>,
    max_encoding_message_size: Option<usize>,
    admission: Arc<IngestAdmission>,
}

impl<H> RawExport<H> {
    pub fn new(handler: H, max_message_size: usize, admission: Arc<IngestAdmission>) -> Self {
        Self {
            inner: Arc::new(handler),
            max_decoding_message_size: Some(max_message_size),
            max_encoding_message_size: Some(max_message_size),
            admission,
        }
    }
}

impl<H> Clone for RawExport<H> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            max_decoding_message_size: self.max_decoding_message_size,
            max_encoding_message_size: self.max_encoding_message_size,
            admission: Arc::clone(&self.admission),
        }
    }
}

/// The export's message length, from the first five bytes of its body - the gRPC frame's flag and length - read
/// before the message itself, with the body that remains, those bytes first.
///
/// The body that remains ends with that frame: an export is one message, and a second one is an error, not more
/// bytes to decode. tonic reads past a unary call's message while it waits for the end of the stream, so a stream
/// that began with an empty message could otherwise carry messages up to the decoding limit past the five bytes it
/// was admitted for.
async fn frame_length<B>(body: B) -> (usize, axum::body::Body)
where
    B: tonic::codegen::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    let mut data = axum::body::Body::new(body).into_data_stream();
    let mut prefix = prost::bytes::BytesMut::new();
    let mut failure = None;
    while prefix.len() < 5 {
        match data.next().await {
            Some(Ok(chunk)) => prefix.extend_from_slice(&chunk),
            Some(Err(error)) => {
                failure = Some(error);
                break;
            }
            None => break,
        }
    }
    let length = if prefix.len() >= 5 {
        u32::from_be_bytes([prefix[1], prefix[2], prefix[3], prefix[4]]) as usize + 5
    } else {
        prefix.len()
    };
    let read = futures::stream::once(async move { Ok(prefix.freeze()) })
        .chain(futures::stream::iter(failure.map(Err)))
        .chain(data);
    (
        length,
        axum::body::Body::from_stream(one_frame(read, length)),
    )
}

/// `stream`, cut at `length` bytes: anything after them is an error, and ends it.
fn one_frame<S, E>(
    stream: S,
    length: usize,
) -> impl futures::Stream<Item = Result<Bytes, StdError>> + Send
where
    S: futures::Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Into<StdError> + Send + 'static,
{
    stream.scan((0usize, false), move |(passed, ended), chunk| {
        let item = if *ended {
            None
        } else {
            match chunk {
                Err(error) => Some(Err(error.into())),
                Ok(chunk) if chunk.len() > length.saturating_sub(*passed) => {
                    // The rest of the stream is not read: the frame was all the export was admitted for.
                    *ended = true;
                    Some(Err(StdError::from(
                        "an OTLP export carries one message; the stream continued past it",
                    )))
                }
                Ok(chunk) => {
                    *passed += chunk.len();
                    Some(Ok(chunk))
                }
            }
        };
        futures::future::ready(item)
    })
}

impl<H: RawExportHandler> NamedService for RawExport<H> {
    const NAME: &'static str = H::SERVICE;
}

/// Adapts the handler to tonic's unary-call shape.
struct ExportSvc<H>(Arc<H>);

impl<H: RawExportHandler> UnaryService<Received<H::Request>> for ExportSvc<H> {
    type Response = H::Response;
    type Future = BoxFuture<Response<Self::Response>, Status>;

    fn call(&mut self, request: Request<Received<H::Request>>) -> Self::Future {
        Arc::clone(&self.0).export(request)
    }
}

impl<H, B> Service<http::Request<B>> for RawExport<H>
where
    H: RawExportHandler,
    B: tonic::codegen::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        if req.uri().path() != H::PATH {
            return Box::pin(async move {
                let mut response = http::Response::new(empty_body());
                let headers = response.headers_mut();
                headers.insert(
                    Status::GRPC_STATUS,
                    (tonic::Code::Unimplemented as i32).into(),
                );
                headers.insert(
                    http::header::CONTENT_TYPE,
                    tonic::metadata::GRPC_CONTENT_TYPE,
                );
                Ok(response)
            });
        }
        let inner = Arc::clone(&self.inner);
        let admission = Arc::clone(&self.admission);
        let (max_decoding, max_encoding) = (
            self.max_decoding_message_size,
            self.max_encoding_message_size,
        );
        Box::pin(async move {
            // Admitted on the frame's declared length, before its message is read (`admission`).
            let (parts, body) = req.into_parts();
            let (length, body) = frame_length(body).await;
            let Some(admitted) = admission.try_admit(length) else {
                return Ok(Status::unavailable(
                    "OTLP ingest is holding as many bytes as it may; retry",
                )
                .into_http());
            };
            let mut grpc = Grpc::new(RawCodec::<H::Response, H::Request>::default())
                .apply_max_message_size_config(max_decoding, max_encoding);
            // On a task of its own that holds the bytes, as HTTP's answer does (`admission::answered_holding`).
            let answer = async move {
                grpc.unary(ExportSvc(inner), http::Request::from_parts(parts, body))
                    .await
            };
            Ok(super::admission::answered_holding(admitted, answer, || {
                Status::internal("the export failed").into_http()
            })
            .await)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
    use std::sync::Mutex;

    /// A handler that records the frame it was handed.
    struct Spy(Mutex<Option<Received<ExportTraceServiceRequest>>>);

    impl RawExportHandler for Spy {
        type Request = ExportTraceServiceRequest;
        type Response =
            opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
        const PATH: &'static str = "/opentelemetry.proto.collector.trace.v1.TraceService/Export";
        const SERVICE: &'static str = "opentelemetry.proto.collector.trace.v1.TraceService";

        fn export(
            self: Arc<Self>,
            request: Request<Received<Self::Request>>,
        ) -> BoxFuture<Response<Self::Response>, Status> {
            *self.0.lock().expect("the spy is not poisoned") = Some(request.into_inner());
            Box::pin(async { Ok(Response::new(Default::default())) })
        }
    }

    fn export() -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![Span {
                        trace_id: vec![7; 16],
                        span_id: vec![9; 8],
                        name: "kept".into(),
                        start_time_unix_nano: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    /// One gRPC length-prefixed frame: an uncompressed message.
    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut framed = vec![0u8];
        framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        framed.extend_from_slice(payload);
        framed
    }

    /// The handler is given the bytes the exporter sent, not a re-encoding of the decoded message.
    ///
    /// Driven through the real dispatch and the real codec, with an unknown field in the frame - what a newer
    /// exporter sends and prost drops. Re-encoding would lose it, which is the whole reason the codec exists.
    #[tokio::test]
    async fn the_handler_receives_the_frame_the_exporter_sent() {
        let canonical = export().encode_to_vec();
        let mut on_the_wire = canonical.clone();
        on_the_wire.extend_from_slice(&[0xfa, 0x3f, 0x02, 0x68, 0x69]);
        assert_ne!(on_the_wire, canonical);

        let spy = Arc::new(Spy(Mutex::new(None)));
        let mut service = RawExport {
            inner: Arc::clone(&spy),
            max_decoding_message_size: Some(1 << 20),
            max_encoding_message_size: Some(1 << 20),
            admission: Arc::new(super::super::admission::IngestAdmission::new(1 << 20)),
        };
        let request = http::Request::builder()
            .uri(Spy::PATH)
            .header(http::header::CONTENT_TYPE, "application/grpc")
            .body(axum::body::Body::from(frame(&on_the_wire)))
            .expect("a well-formed request");
        let response = service.call(request).await.expect("the call is infallible");
        assert_eq!(response.status(), http::StatusCode::OK);

        let received = spy
            .0
            .lock()
            .expect("the spy is not poisoned")
            .take()
            .expect("the handler was called");
        assert_eq!(
            received.message,
            export(),
            "the message is the decoded export"
        );
        assert_eq!(
            received.raw.as_ref(),
            on_the_wire.as_slice(),
            "the handler must see the frame's bytes, unknown field included"
        );
    }

    /// An export the in-flight budget cannot hold is answered UNAVAILABLE on its frame's declared length,
    /// before its message is read or decoded, and the handler never sees it; once the budget frees, it is served.
    #[tokio::test]
    async fn an_export_over_the_budget_is_unavailable_before_its_message_is_read() {
        let payload = export().encode_to_vec();
        let admission = Arc::new(super::super::admission::IngestAdmission::new(
            (payload.len() + 5) as u64,
        ));
        let spy = Arc::new(Spy(Mutex::new(None)));
        let mut service = RawExport {
            inner: Arc::clone(&spy),
            max_decoding_message_size: Some(1 << 20),
            max_encoding_message_size: Some(1 << 20),
            admission: Arc::clone(&admission),
        };
        let request = || {
            http::Request::builder()
                .uri(Spy::PATH)
                .header(http::header::CONTENT_TYPE, "application/grpc")
                .body(axum::body::Body::from(frame(&payload)))
                .expect("a well-formed request")
        };
        let held = admission.try_admit(1).expect("another export in flight");
        let response = service
            .call(request())
            .await
            .expect("the call is infallible");
        assert_eq!(
            response
                .headers()
                .get(Status::GRPC_STATUS)
                .and_then(|value| value.to_str().ok()),
            Some("14"),
            "over the budget answers UNAVAILABLE"
        );
        assert!(spy.0.lock().expect("spy").is_none(), "the handler ran");

        drop(held);
        let response = service
            .call(request())
            .await
            .expect("the call is infallible");
        assert_eq!(response.status(), http::StatusCode::OK);
        assert!(spy.0.lock().expect("spy").is_some(), "served once it fits");
        assert_eq!(admission.in_flight(), 0, "released once answered");
    }

    /// An export is one message: a stream that continues past its first frame is refused before anything after
    /// that frame is read or decoded, so the bytes it was admitted for are the bytes it can make the server hold.
    #[tokio::test]
    async fn a_second_message_in_an_export_is_refused_not_decoded() {
        let payload = export().encode_to_vec();
        let admission = Arc::new(super::super::admission::IngestAdmission::new(1 << 20));
        let spy = Arc::new(Spy(Mutex::new(None)));
        let mut service = RawExport {
            inner: Arc::clone(&spy),
            max_decoding_message_size: Some(1 << 20),
            max_encoding_message_size: Some(1 << 20),
            admission: Arc::clone(&admission),
        };
        let mut body = frame(&[]);
        body.extend_from_slice(&frame(&payload));
        let response = service
            .call(
                http::Request::builder()
                    .uri(Spy::PATH)
                    .header(http::header::CONTENT_TYPE, "application/grpc")
                    .body(axum::body::Body::from_stream(futures::stream::iter([
                        Ok::<_, std::io::Error>(Bytes::from(body[..5].to_vec())),
                        Ok(Bytes::from(body[5..].to_vec())),
                    ])))
                    .expect("a well-formed request"),
            )
            .await
            .expect("the call is infallible");
        let status = response
            .headers()
            .get(Status::GRPC_STATUS)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert!(
            status.as_deref().is_some_and(|code| code != "0"),
            "a two-message export was answered {status:?}"
        );
        assert!(spy.0.lock().expect("spy").is_none(), "the handler ran");
        assert_eq!(admission.in_flight(), 0, "released once answered");
    }

    /// Another service's path is unimplemented, as the generated server answers it.
    #[tokio::test]
    async fn another_path_is_unimplemented() {
        let mut service = RawExport::new(
            Spy(Mutex::new(None)),
            1 << 20,
            Arc::new(super::super::admission::IngestAdmission::new(1 << 20)),
        );
        let request = http::Request::builder()
            .uri("/opentelemetry.proto.collector.logs.v1.LogsService/Export")
            .body(axum::body::Body::empty())
            .expect("a well-formed request");
        let response = service.call(request).await.expect("the call is infallible");
        assert_eq!(
            response
                .headers()
                .get(Status::GRPC_STATUS)
                .and_then(|value| value.to_str().ok()),
            Some("12"),
            "an unknown method answers UNIMPLEMENTED"
        );
    }
}
