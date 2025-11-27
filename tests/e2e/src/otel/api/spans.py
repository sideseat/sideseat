"""Span listing and filtering tests."""

from typing import Any

from ...api import api_call, encode_param
from ...logging import log_info, log_section, log_success
from ..base import BaseTestSuite


class SpanTests(BaseTestSuite):
    """Span API test suite."""

    def test_span_listing_basic(self) -> bool:
        """Test basic span listing."""
        log_section("Span Listing Tests")
        log_info("Testing span listing...")

        result = api_call("/spans?limit=100")
        if not self.assert_not_none(result, "Span listing returns data"):
            return False

        if isinstance(result, list):
            self.spans = result
            self.all_spans = result.copy()
            self.assert_greater(len(self.spans), 0, "At least one span exists")
            return len(self.spans) > 0

        return False

    def test_span_structure(self) -> bool:
        """Validate span data structure."""
        log_info("Validating span structure...")

        if not self.spans:
            self.skip("No spans to validate")
            return True

        span = self.spans[0]

        # Required fields
        required_fields = [
            "span_id",
            "trace_id",
            "span_name",
            "service_name",
            "start_time_ns",
            "status_code",
        ]
        for field in required_fields:
            self.assert_true(field in span, f"Span has required field: {field}")

        # Expected fields for GenAI spans (informational)
        genai_fields = [
            "detected_framework",
            "detected_category",
            "gen_ai_request_model",
            "usage_input_tokens",
            "usage_output_tokens",
        ]
        found_genai = sum(1 for field in genai_fields if span.get(field) is not None)
        log_success(f"Span has {found_genai} GenAI fields populated")

        return True

    def test_spans_by_trace_id(self) -> bool:
        """Test getting spans for a specific trace."""
        log_info("Testing spans by trace ID...")

        if not self.traces:
            self.skip("No traces for spans-by-trace test")
            return True

        trace_id = self.traces[0]["trace_id"]
        expected_count = self.traces[0].get("span_count", 0)

        # Use /spans endpoint with trace_id filter and higher limit
        result = api_call(f"/spans?trace_id={encode_param(trace_id)}&limit=500")
        if not self.assert_not_none(result, f"Spans for trace {trace_id[:16]}..."):
            return False

        if isinstance(result, list):
            spans = result
            self.assert_greater(len(spans), 0, "Trace has spans")

            # Verify all spans belong to this trace
            all_match = all(s.get("trace_id") == trace_id for s in spans)
            self.assert_true(all_match, "All spans belong to correct trace")

            # Verify span count matches trace metadata
            if expected_count > 0:
                self.assert_equals(len(spans), expected_count, "Span count matches trace metadata")

        return True

    def test_span_limit_parameter(self) -> bool:
        """Test span limit parameter."""
        log_info("Testing span limit parameter...")

        for limit in [1, 5, 10]:
            result = api_call(f"/spans?limit={limit}")
            if result and isinstance(result, list):
                self.assert_true(
                    len(result) <= limit,
                    f"Limit {limit} respected: got {len(result)} spans",
                )

        return True

    def test_span_filter_by_service(self) -> bool:
        """Test filtering spans by service."""
        log_section("Span Filtering Tests")
        log_info("Testing span filter by service...")

        if not self.spans:
            self.skip("No spans for service filter")
            return True

        service = self.spans[0].get("service_name", "")
        if not service:
            self.skip("No service name in span")
            return True

        result = api_call(f"/spans?service={encode_param(service)}")
        if not self.assert_not_none(result, "Filter spans by service"):
            return False

        if isinstance(result, list):
            all_match = all(s.get("service_name") == service for s in result)
            self.assert_true(all_match, "All spans have correct service")

        return True

    def test_span_filter_by_category(self) -> bool:
        """Test filtering spans by category."""
        log_info("Testing span filter by category...")

        # Find a span with category
        category = self._find_field_value("detected_category")
        if not category:
            self.skip("No spans with category found")
            return True

        result = api_call(f"/spans?category={encode_param(category)}")
        if not self.assert_not_none(result, f"Filter by category '{category}'"):
            return False

        if isinstance(result, list):
            self.assert_greater(len(result), 0, "Filter returns spans")

        return True

    def test_span_filter_by_model(self) -> bool:
        """Test filtering spans by model."""
        log_info("Testing span filter by model...")

        # Find a span with model
        model = self._find_field_value("gen_ai_request_model")
        if not model:
            self.skip("No spans with model found")
            return True

        result = api_call(f"/spans?model={encode_param(model)}")
        if not self.assert_not_none(result, "Filter by model"):
            return False

        if isinstance(result, list):
            self.assert_greater(len(result), 0, "Filter returns spans for model")

        return True

    def test_span_filter_by_framework(self) -> bool:
        """Test filtering spans by framework."""
        log_info("Testing span filter by framework...")

        # Find framework
        framework = None
        for s in self.spans:
            fw = s.get("detected_framework")
            if fw and fw != "unknown":
                framework = fw
                break

        if not framework:
            self.skip("No spans with framework found")
            return True

        result = api_call(f"/spans?framework={encode_param(framework)}")
        if not self.assert_not_none(result, f"Filter by framework '{framework}'"):
            return False

        if isinstance(result, list):
            self.assert_greater(len(result), 0, "Filter returns spans")

        return True

    def _find_field_value(self, field: str) -> Any:
        """Find a non-None value for a field in spans."""
        for s in self.spans:
            value = s.get(field)
            if value:
                return value
        return None

    def run_all(self) -> None:
        """Run all span tests."""
        # Listing tests
        self.test_span_listing_basic()
        self.test_span_structure()
        self.test_spans_by_trace_id()
        self.test_span_limit_parameter()

        # Filtering tests
        self.test_span_filter_by_service()
        self.test_span_filter_by_category()
        self.test_span_filter_by_model()
        self.test_span_filter_by_framework()
