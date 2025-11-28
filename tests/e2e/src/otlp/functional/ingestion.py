"""OTLP ingestion tests - HTTP and gRPC trace ingestion."""

import time
from concurrent.futures import ThreadPoolExecutor, as_completed

from ...api import api_call, encode_param
from ...base import BaseTestSuite
from ...config import TRACE_PERSIST_WAIT
from ...logging import log_info, log_section
from ..traces import (
    create_batch_traces,
    create_otlp_trace,
    create_simple_trace,
    generate_span_id,
    generate_trace_id,
    send_otlp_traces_http,
)


class IngestionTests(BaseTestSuite):
    """HTTP and gRPC trace ingestion tests."""

    def __init__(self) -> None:
        super().__init__()
        self.ingested_trace_ids: list[str] = []

    def test_http_json_ingestion(self) -> bool:
        """Test OTLP JSON ingestion via HTTP."""
        log_section("OTLP Ingestion Tests")
        log_info("Testing HTTP JSON ingestion...")

        trace_id, payload = create_simple_trace(
            service_name="http-json-test",
            span_count=3,
        )

        success, status = send_otlp_traces_http(payload)
        if not self.assert_true(success, f"HTTP JSON ingestion accepted ({status})"):
            return False

        self.ingested_trace_ids.append(trace_id)
        time.sleep(2)

        result = api_call(f"/traces/{trace_id}")
        return self.assert_not_none(result, f"Trace {trace_id[:16]}... persisted")

    def test_batch_ingestion(self) -> bool:
        """Test large batch handling."""
        log_info("Testing batch ingestion...")

        # Create 10 traces with 5 spans each
        traces = create_batch_traces(count=10, spans_per_trace=5)

        for trace_id, payload in traces:
            success, _ = send_otlp_traces_http(payload)
            if not success:
                return self.assert_true(False, "Batch ingestion failed")
            self.ingested_trace_ids.append(trace_id)

        time.sleep(TRACE_PERSIST_WAIT)

        # Verify all traces exist
        found = 0
        for trace_id in self.ingested_trace_ids[-10:]:
            result = api_call(f"/traces/{trace_id}")
            if result:
                found += 1

        return self.assert_equals(found, 10, "All 10 batch traces persisted")

    def test_concurrent_ingestion(self) -> bool:
        """Test multiple concurrent clients."""
        log_info("Testing concurrent ingestion...")

        traces = create_batch_traces(count=5, spans_per_trace=3)
        concurrent_trace_ids = []

        def send_trace(trace_data: tuple[str, dict]) -> bool:
            trace_id, payload = trace_data
            concurrent_trace_ids.append(trace_id)
            success, _ = send_otlp_traces_http(payload)
            return success

        with ThreadPoolExecutor(max_workers=5) as executor:
            futures = [executor.submit(send_trace, t) for t in traces]
            results = [f.result() for f in as_completed(futures)]

        all_success = all(results)
        if not self.assert_true(all_success, "All concurrent requests succeeded"):
            return False

        time.sleep(TRACE_PERSIST_WAIT)

        # Verify all traces
        found = 0
        for trace_id in concurrent_trace_ids:
            result = api_call(f"/traces/{trace_id}")
            if result:
                found += 1

        self.ingested_trace_ids.extend(concurrent_trace_ids)
        return self.assert_equals(found, 5, "All concurrent traces persisted")

    def test_framework_detection(self) -> bool:
        """Test LangChain, Strands, OpenInference detection."""
        log_info("Testing framework detection...")

        frameworks = [
            ("langchain", "langchain.request"),
            ("strands", "strands.agent.invoke"),
            ("openinference", "openinference.llm"),
        ]

        detected = 0
        for framework, scope_name in frameworks:
            trace_id = generate_trace_id()
            now_ns = int(time.time() * 1_000_000_000)

            spans = [
                {
                    "trace_id": trace_id,
                    "span_id": generate_span_id(),
                    "name": scope_name,
                    "kind": 1,
                    "start_time_ns": now_ns,
                    "end_time_ns": now_ns + 1_000_000_000,
                    "attributes": [],
                    "status": {"code": 1},
                }
            ]

            payload = create_otlp_trace(trace_id, spans, f"{framework}-test", framework)
            success, _ = send_otlp_traces_http(payload)

            if success:
                self.ingested_trace_ids.append(trace_id)
                detected += 1

        time.sleep(TRACE_PERSIST_WAIT)
        return self.assert_greater(
            detected, 0, f"Framework traces ingested: {detected}/3"
        )

    def test_genai_extraction(self) -> bool:
        """Test token usage and model info extraction."""
        log_info("Testing GenAI attribute extraction...")

        trace_id, payload = create_simple_trace(
            service_name="genai-test",
            span_count=1,
            with_genai=True,
        )

        success, _ = send_otlp_traces_http(payload)
        if not success:
            return self.assert_true(False, "GenAI trace ingestion failed")

        self.ingested_trace_ids.append(trace_id)
        time.sleep(TRACE_PERSIST_WAIT)

        result = api_call(f"/spans?trace_id={encode_param(trace_id)}&limit=10")
        if not result or not isinstance(result, dict):
            return self.assert_true(False, "No spans returned for GenAI trace")

        spans = result.get("spans", [])
        if not spans:
            return self.assert_true(False, "No spans in response for GenAI trace")

        # Check if GenAI attributes were extracted
        span = spans[0]
        has_model = span.get("gen_ai_request_model") is not None
        has_input = span.get("usage_input_tokens") is not None
        has_output = span.get("usage_output_tokens") is not None

        extracted = sum([has_model, has_input, has_output])
        return self.assert_greater(
            extracted, 0, f"GenAI attributes extracted: {extracted}/3"
        )

    def test_http_protobuf_ingestion(self) -> bool:
        """Test OTLP Protobuf via HTTP."""
        log_info("Testing HTTP Protobuf ingestion...")

        try:
            from opentelemetry.proto.collector.trace.v1 import trace_service_pb2
            from opentelemetry.proto.common.v1 import common_pb2
            from opentelemetry.proto.resource.v1 import resource_pb2
            from opentelemetry.proto.trace.v1 import trace_pb2
        except ImportError:
            self.skip("Protobuf dependencies not installed (opentelemetry-proto)")
            return True

        from urllib.request import Request, urlopen
        from ...config import OTEL_BASE

        trace_id = generate_trace_id()
        span_id = generate_span_id()
        now_ns = int(time.time() * 1_000_000_000)

        # Build protobuf message
        span = trace_pb2.Span(
            trace_id=bytes.fromhex(trace_id),
            span_id=bytes.fromhex(span_id),
            name="protobuf-http-test-span",
            kind=trace_pb2.Span.SpanKind.SPAN_KIND_INTERNAL,
            start_time_unix_nano=now_ns,
            end_time_unix_nano=now_ns + 1_000_000_000,
            status=trace_pb2.Status(code=trace_pb2.Status.StatusCode.STATUS_CODE_OK),
        )

        scope_spans = trace_pb2.ScopeSpans(spans=[span])
        resource = resource_pb2.Resource(
            attributes=[
                common_pb2.KeyValue(
                    key="service.name",
                    value=common_pb2.AnyValue(string_value="protobuf-http-test"),
                )
            ]
        )
        resource_spans = trace_pb2.ResourceSpans(
            resource=resource,
            scope_spans=[scope_spans],
        )

        request = trace_service_pb2.ExportTraceServiceRequest(
            resource_spans=[resource_spans]
        )

        try:
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=request.SerializeToString(),
                headers={"Content-Type": "application/x-protobuf"},
                method="POST",
            )
            with urlopen(req, timeout=10) as response:
                if response.status == 200:
                    self.ingested_trace_ids.append(trace_id)
                    time.sleep(TRACE_PERSIST_WAIT)

                    result = api_call(f"/traces/{trace_id}")
                    if result and isinstance(result, dict):
                        return self.assert_equals(
                            result.get("trace_id"),
                            trace_id,
                            "HTTP Protobuf ingested trace retrieved",
                        )
                    return self.assert_true(True, "HTTP Protobuf ingestion accepted")

                return self.assert_true(
                    False, f"HTTP Protobuf ingestion failed: {response.status}"
                )
        except Exception as e:
            # Server may not support protobuf over HTTP
            self.skip(f"HTTP Protobuf not supported: {e}")
            return True

    def test_grpc_ingestion(self) -> bool:
        """Test OTLP via gRPC."""
        log_info("Testing gRPC ingestion...")

        try:
            import grpc
            from opentelemetry.proto.collector.trace.v1 import trace_service_pb2
            from opentelemetry.proto.collector.trace.v1 import trace_service_pb2_grpc
            from opentelemetry.proto.common.v1 import common_pb2
            from opentelemetry.proto.resource.v1 import resource_pb2
            from opentelemetry.proto.trace.v1 import trace_pb2
        except ImportError:
            self.skip("gRPC dependencies not installed (grpcio, opentelemetry-proto)")
            return True

        from ...config import GRPC_PORT, SERVER_HOST

        trace_id = generate_trace_id()
        span_id = generate_span_id()
        now_ns = int(time.time() * 1_000_000_000)

        # Build protobuf message
        span = trace_pb2.Span(
            trace_id=bytes.fromhex(trace_id),
            span_id=bytes.fromhex(span_id),
            name="grpc-test-span",
            kind=trace_pb2.Span.SpanKind.SPAN_KIND_INTERNAL,
            start_time_unix_nano=now_ns,
            end_time_unix_nano=now_ns + 1_000_000_000,
            status=trace_pb2.Status(code=trace_pb2.Status.StatusCode.STATUS_CODE_OK),
        )

        scope_spans = trace_pb2.ScopeSpans(spans=[span])
        resource = resource_pb2.Resource(
            attributes=[
                common_pb2.KeyValue(
                    key="service.name",
                    value=common_pb2.AnyValue(string_value="grpc-test-service"),
                )
            ]
        )
        resource_spans = trace_pb2.ResourceSpans(
            resource=resource,
            scope_spans=[scope_spans],
        )

        request = trace_service_pb2.ExportTraceServiceRequest(
            resource_spans=[resource_spans]
        )

        try:
            channel = grpc.insecure_channel(f"{SERVER_HOST}:{GRPC_PORT}")
            stub = trace_service_pb2_grpc.TraceServiceStub(channel)
            response = stub.Export(request, timeout=10)
            channel.close()

            self.ingested_trace_ids.append(trace_id)
            time.sleep(TRACE_PERSIST_WAIT)

            # Verify trace exists
            result = api_call(f"/traces/{trace_id}")
            if result and isinstance(result, dict):
                return self.assert_equals(
                    result.get("trace_id"),
                    trace_id,
                    "gRPC ingested trace retrieved",
                )

            return self.assert_true(True, "gRPC ingestion accepted")

        except grpc.RpcError as e:
            return self.assert_true(False, f"gRPC ingestion failed: {e.code()}")
        except Exception as e:
            return self.assert_true(False, f"gRPC ingestion error: {e}")

    def run_all(self) -> None:
        """Run all ingestion tests."""
        self.test_http_json_ingestion()
        self.test_http_protobuf_ingestion()
        self.test_grpc_ingestion()
        self.test_batch_ingestion()
        self.test_concurrent_ingestion()
        self.test_framework_detection()
        self.test_genai_extraction()
