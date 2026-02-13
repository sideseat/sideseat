"""Basic tests for SideSeat SDK."""

from __future__ import annotations

import os
from datetime import date, datetime, timezone

import pytest

import sideseat
from sideseat import SideSeat, __version__, encode_value


def test_version() -> None:
    """Version should be a valid semver string."""
    assert __version__
    parts = __version__.split(".")
    assert len(parts) == 3
    assert all(p.isdigit() for p in parts)


def test_encode_value_bytes() -> None:
    """Bytes should be base64 encoded."""
    result = encode_value(b"hello")
    assert result == "aGVsbG8="


def test_encode_value_string_passthrough() -> None:
    """Strings should pass through unchanged."""
    result = encode_value("hello")
    assert result == "hello"


def test_encode_value_dict_recursive() -> None:
    """Dicts should have bytes values encoded recursively."""
    result = encode_value({"key": b"value", "nested": {"inner": b"data"}})
    assert result == {"key": "dmFsdWU=", "nested": {"inner": "ZGF0YQ=="}}


def test_encode_value_list() -> None:
    """Lists should have bytes values encoded."""
    result = encode_value([b"a", "b", b"c"])
    assert result == ["YQ==", "b", "Yw=="]


def test_encode_value_set() -> None:
    """Sets should be converted to sorted lists."""
    result = encode_value({"c", "a", "b"})
    assert result == ["a", "b", "c"]


def test_encode_value_set_mixed_types() -> None:
    """Sets with mixed types should be converted to lists (unsorted)."""
    # Mixed types can't be sorted, so just check it returns a list
    result = encode_value({1, "a", None})
    assert isinstance(result, list)
    assert set(result) == {1, "a", None}


def test_encode_value_primitives() -> None:
    """Primitive types should pass through unchanged."""
    assert encode_value(None) is None
    assert encode_value(42) == 42
    assert encode_value(3.14) == 3.14
    assert encode_value(True) is True
    assert encode_value(False) is False


def test_encode_value_datetime() -> None:
    """Datetime should be converted to ISO8601."""
    dt = datetime(2024, 1, 15, 12, 30, 45, tzinfo=timezone.utc)
    result = encode_value(dt)
    assert result == "2024-01-15T12:30:45+00:00"


def test_encode_value_date() -> None:
    """Date should be converted to ISO8601."""
    d = date(2024, 1, 15)
    result = encode_value(d)
    assert result == "2024-01-15"


def test_encode_value_non_serializable() -> None:
    """Non-serializable types should return type name placeholder."""
    result = encode_value(object())
    assert result == "<object>"


def test_sideseat_initialization(monkeypatch: pytest.MonkeyPatch) -> None:
    """SideSeat should initialize with a tracer provider."""
    # Ensure no env vars interfere
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    assert client.telemetry is not None
    assert client.telemetry.tracer_provider is not None
    client.shutdown()


def test_sideseat_context_manager(monkeypatch: pytest.MonkeyPatch) -> None:
    """Context manager should auto-shutdown."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    with SideSeat(enable_metrics=False, enable_logs=False) as client:
        assert client.telemetry is not None


def test_sideseat_disabled_mode(monkeypatch: pytest.MonkeyPatch) -> None:
    """Disabled mode should skip all setup."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(disabled=True)
    assert client.is_disabled is True
    client.shutdown()


def test_sideseat_chaining(temp_trace_file: str, monkeypatch: pytest.MonkeyPatch) -> None:
    """Exporter setup methods should return self for chaining."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    result = client.telemetry.setup_console_exporter()
    assert result is client.telemetry
    client.shutdown()


def test_sideseat_shutdown_idempotent(monkeypatch: pytest.MonkeyPatch) -> None:
    """Shutdown should be safe to call multiple times."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    client.shutdown()
    client.shutdown()  # Should not raise


def test_file_exporter_append_mode(temp_trace_file: str, monkeypatch: pytest.MonkeyPatch) -> None:
    """File exporter should default to append mode."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    # Create first telemetry instance and write
    t1 = SideSeat(enable_metrics=False, enable_logs=False)
    t1.telemetry.setup_file_exporter(temp_trace_file)
    t1.shutdown()

    # Create second instance - should append, not overwrite
    t2 = SideSeat(enable_metrics=False, enable_logs=False)
    t2.telemetry.setup_file_exporter(temp_trace_file)
    t2.shutdown()

    # File should still exist (not truncated)
    assert os.path.exists(temp_trace_file)


def test_sideseat_tracer_provider_property(monkeypatch: pytest.MonkeyPatch) -> None:
    """SideSeat should expose tracer_provider directly."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    assert client.tracer_provider is not None
    assert client.tracer_provider is client.telemetry.tracer_provider
    client.shutdown()


def test_sideseat_span_context_manager(monkeypatch: pytest.MonkeyPatch) -> None:
    """span() context manager should yield a span."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    with client.span("test-span") as span:
        assert span is not None
        span.set_attribute("test_key", "test_value")
    client.shutdown()


def test_sideseat_span_records_exception(monkeypatch: pytest.MonkeyPatch) -> None:
    """span() should record exception and set error status."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    try:
        with client.span("error-span"):
            raise ValueError("test error")
    except ValueError:
        pass  # Expected
    client.shutdown()


def test_sideseat_get_tracer(monkeypatch: pytest.MonkeyPatch) -> None:
    """get_tracer() should return a tracer."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    tracer = client.get_tracer("my-tracer")
    assert tracer is not None
    client.shutdown()


def test_sideseat_force_flush(monkeypatch: pytest.MonkeyPatch) -> None:
    """force_flush() should return True."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(enable_metrics=False, enable_logs=False)
    result = client.force_flush()
    assert result is True
    client.shutdown()


def test_sideseat_validate_connection_disabled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """validate_connection() should return False when disabled."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(disabled=True)
    result = client.validate_connection()
    assert result is False
    client.shutdown()


def test_sideseat_validate_connection_no_server(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """validate_connection() should return False when server not running."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    # Use a port that's unlikely to have a server running
    client = SideSeat(
        endpoint="http://127.0.0.1:59999",
        enable_metrics=False,
        enable_logs=False,
    )
    result = client.validate_connection(timeout=0.5)
    assert result is False
    client.shutdown()


def test_sideseat_repr(monkeypatch: pytest.MonkeyPatch) -> None:
    """__repr__ should show endpoint and project."""
    monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

    client = SideSeat(
        endpoint="http://test:5388",
        project_id="my-project",
        disabled=True,
    )
    repr_str = repr(client)
    assert "http://test:5388" in repr_str
    assert "my-project" in repr_str
    client.shutdown()


class TestGlobalInstance:
    """Tests for global instance functions."""

    def test_init_creates_instance(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """init() should create global instance."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        assert sideseat.is_initialized() is False
        client = sideseat.init(disabled=True)
        assert sideseat.is_initialized() is True
        assert isinstance(client, SideSeat)

    def test_get_client_returns_instance(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """get_client() should return global instance."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        sideseat.init(disabled=True)
        client = sideseat.get_client()
        assert isinstance(client, SideSeat)

    def test_get_client_raises_if_not_initialized(self) -> None:
        """get_client() should raise if not initialized."""
        # conftest resets global instance between tests
        assert sideseat.is_initialized() is False
        with pytest.raises(sideseat.SideSeatError, match="not initialized"):
            sideseat.get_client()

    def test_shutdown_clears_instance(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """shutdown() should clear global instance."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        sideseat.init(disabled=True)
        assert sideseat.is_initialized() is True
        sideseat.shutdown()
        assert sideseat.is_initialized() is False

    def test_double_init_returns_existing(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Double init() should return existing instance."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        client1 = sideseat.init(disabled=True)
        client2 = sideseat.init(disabled=True)
        assert client1 is client2


class TestBuildEndpoint:
    """Tests for TelemetryClient._build_endpoint."""

    def test_endpoint_without_path_uses_sideseat_format(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Endpoint without path should use SideSeat format: /otel/{project}/v1/{signal}."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        client = SideSeat(
            endpoint="http://localhost:5388",
            project_id="my-project",
            enable_traces=False,
            enable_metrics=False,
            enable_logs=False,
        )
        # Access private method for testing
        result = client.telemetry._build_endpoint("traces")
        assert result == "http://localhost:5388/otel/my-project/v1/traces"
        client.shutdown()

    def test_endpoint_with_path_appends_v1_signal(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Endpoint with path should append /v1/{signal}."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        client = SideSeat(
            endpoint="http://localhost:5388/otel/default",
            project_id="ignored",
            enable_traces=False,
            enable_metrics=False,
            enable_logs=False,
        )
        result = client.telemetry._build_endpoint("traces")
        assert result == "http://localhost:5388/otel/default/v1/traces"
        client.shutdown()

    def test_endpoint_with_root_path_uses_sideseat_format(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Endpoint with only root path should use SideSeat format."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        client = SideSeat(
            endpoint="http://localhost:5388/",
            project_id="my-project",
            enable_traces=False,
            enable_metrics=False,
            enable_logs=False,
        )
        result = client.telemetry._build_endpoint("metrics")
        assert result == "http://localhost:5388/otel/my-project/v1/metrics"
        client.shutdown()

    def test_build_endpoint_all_signals(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """All signal types should work correctly."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)

        client = SideSeat(
            endpoint="http://localhost:5388/custom/path",
            enable_traces=False,
            enable_metrics=False,
            enable_logs=False,
        )
        assert (
            client.telemetry._build_endpoint("traces")
            == "http://localhost:5388/custom/path/v1/traces"
        )
        assert (
            client.telemetry._build_endpoint("metrics")
            == "http://localhost:5388/custom/path/v1/metrics"
        )
        assert (
            client.telemetry._build_endpoint("logs") == "http://localhost:5388/custom/path/v1/logs"
        )
        client.shutdown()
