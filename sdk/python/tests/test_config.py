"""Tests for SideSeat configuration."""

from unittest.mock import MagicMock

import pytest

from sideseat.config import (
    Config,
    Frameworks,
    _detect_framework,
    _normalize_endpoint,
    _parse_bool_env,
    _resolve_framework_input,
)
from sideseat.telemetry import _ContextSpanProcessor, _session_id_var, _user_id_var


class TestNormalizeEndpoint:
    """Tests for endpoint URL normalization."""

    def test_rejects_invalid_scheme(self) -> None:
        """Non-http(s) schemes should raise ValueError."""
        with pytest.raises(ValueError, match="Invalid endpoint"):
            _normalize_endpoint("ftp://localhost:5388")
        with pytest.raises(ValueError, match="Must start with http://"):
            _normalize_endpoint("localhost:5388")

    def test_accepts_http_and_https(self) -> None:
        """HTTP and HTTPS should be accepted."""
        assert _normalize_endpoint("http://localhost:5388") == "http://localhost:5388"
        assert _normalize_endpoint("https://api.sideseat.ai") == "https://api.sideseat.ai"

    def test_strips_whitespace(self) -> None:
        """Whitespace should be stripped."""
        assert _normalize_endpoint("  http://localhost:5388  ") == "http://localhost:5388"

    def test_strips_trailing_slash(self) -> None:
        """Trailing slashes should be stripped."""
        assert _normalize_endpoint("http://localhost:5388/") == "http://localhost:5388"
        assert _normalize_endpoint("https://api.example.com//") == "https://api.example.com"

    def test_preserves_path(self) -> None:
        """Path components should be preserved."""
        assert (
            _normalize_endpoint("http://localhost:5388/otel/default")
            == "http://localhost:5388/otel/default"
        )
        assert (
            _normalize_endpoint("http://localhost:4318/custom/path")
            == "http://localhost:4318/custom/path"
        )

    def test_preserves_port(self) -> None:
        """Port should be preserved."""
        assert _normalize_endpoint("http://localhost:4318") == "http://localhost:4318"
        assert _normalize_endpoint("http://localhost:4318/v1") == "http://localhost:4318/v1"


class TestParseBoolEnv:
    """Tests for boolean env var parsing."""

    def test_truthy_values(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Truthy values should return True."""
        for val in ("1", "true", "True", "TRUE", "yes", "Yes", "YES"):
            monkeypatch.setenv("TEST_BOOL", val)
            assert _parse_bool_env("TEST_BOOL", False) is True

    def test_falsy_values(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Falsy values should return False."""
        for val in ("0", "false", "False", "FALSE", "no", "No", "NO"):
            monkeypatch.setenv("TEST_BOOL", val)
            assert _parse_bool_env("TEST_BOOL", True) is False

    def test_missing_returns_default(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Missing env var should return default."""
        monkeypatch.delenv("TEST_BOOL", raising=False)
        assert _parse_bool_env("TEST_BOOL", True) is True
        assert _parse_bool_env("TEST_BOOL", False) is False

    def test_invalid_returns_default(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Invalid value should return default."""
        monkeypatch.setenv("TEST_BOOL", "maybe")
        assert _parse_bool_env("TEST_BOOL", True) is True
        assert _parse_bool_env("TEST_BOOL", False) is False


class TestDetectFramework:
    """Tests for framework auto-detection."""

    def test_returns_fallback_when_no_framework(self) -> None:
        """Should return sideseat fallback when no AI framework is installed."""
        key, pkg, ver = _detect_framework()
        # We can't guarantee which framework is installed in test env,
        # but we can verify the return type
        assert isinstance(key, str)
        assert isinstance(pkg, str)
        assert isinstance(ver, str)


class TestConfig:
    """Tests for Config dataclass."""

    def test_default_values(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Config should have sensible defaults."""
        # Clear all env vars
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.delenv("OTEL_EXPORTER_OTLP_ENDPOINT", raising=False)
        monkeypatch.delenv("SIDESEAT_API_KEY", raising=False)
        monkeypatch.delenv("SIDESEAT_PROJECT", raising=False)
        monkeypatch.delenv("SIDESEAT_DISABLED", raising=False)
        monkeypatch.delenv("SIDESEAT_DEBUG", raising=False)

        config = Config.create()

        assert config.disabled is False
        assert config.endpoint == "http://127.0.0.1:5388"
        assert config.api_key is None
        assert config.project_id == "default"
        assert config.auto_instrument is True
        assert config.enable_traces is True
        assert config.enable_metrics is True
        assert config.enable_logs is False
        assert config.encode_binary is True
        assert config.capture_content is True
        assert config.debug is False

    def test_env_var_priority_sideseat_over_otel(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """SIDESEAT_* env vars should take priority over OTEL_*."""
        monkeypatch.setenv("SIDESEAT_ENDPOINT", "http://sideseat:5388")
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel:4318")

        config = Config.create()

        assert config.endpoint == "http://sideseat:5388"

    def test_endpoint_preserves_path(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Endpoint paths should be preserved."""
        monkeypatch.delenv("SIDESEAT_ENDPOINT", raising=False)
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel:4318/otel/default")

        config = Config.create()

        assert config.endpoint == "http://otel:4318/otel/default"

    def test_constructor_overrides_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Constructor params should override env vars."""
        monkeypatch.setenv("SIDESEAT_ENDPOINT", "http://env:5388")
        monkeypatch.setenv("SIDESEAT_PROJECT", "env-project")

        config = Config.create(
            endpoint="http://constructor:5388",
            project_id="constructor-project",
        )

        assert config.endpoint == "http://constructor:5388"
        assert config.project_id == "constructor-project"

    def test_disabled_via_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """SIDESEAT_DISABLED should disable SDK."""
        monkeypatch.setenv("SIDESEAT_DISABLED", "true")

        config = Config.create()

        assert config.disabled is True

    def test_disabled_via_constructor_overrides_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Constructor disabled param should override env."""
        monkeypatch.setenv("SIDESEAT_DISABLED", "true")

        config = Config.create(disabled=False)

        assert config.disabled is False

    def test_debug_via_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """SIDESEAT_DEBUG should enable debug mode."""
        monkeypatch.setenv("SIDESEAT_DEBUG", "1")

        config = Config.create()

        assert config.debug is True

    def test_enable_logs_respects_otel_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """enable_logs should respect OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED."""
        monkeypatch.setenv("OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED", "true")

        config = Config.create()

        assert config.enable_logs is True

    def test_enable_logs_constructor_overrides_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Constructor enable_logs should override env var."""
        monkeypatch.setenv("OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED", "false")

        config = Config.create(enable_logs=True)

        assert config.enable_logs is True

    def test_explicit_framework(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Explicit framework should override auto-detection."""
        config = Config.create(framework=Frameworks.LangChain)

        assert config.framework == Frameworks.LangChain

    def test_immutable(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Config should be immutable (frozen dataclass)."""
        config = Config.create()

        with pytest.raises(AttributeError):
            config.endpoint = "http://new:5388"  # type: ignore[misc]

    def test_repr(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Config repr should be informative."""
        config = Config.create(project_id="test-project")

        repr_str = repr(config)
        assert "disabled=" in repr_str
        assert "endpoint=" in repr_str
        assert "project_id=" in repr_str
        assert "test-project" in repr_str


class TestFrameworks:
    """Tests for Frameworks constants."""

    def test_all_frameworks_are_strings(self) -> None:
        """All framework constants should be strings."""
        assert Frameworks.Strands == "strands"
        assert Frameworks.LangChain == "langchain"
        assert Frameworks.LangGraph == "langgraph"
        assert Frameworks.CrewAI == "crewai"
        assert Frameworks.AutoGen == "autogen"
        assert Frameworks.OpenAIAgents == "openai-agents"
        assert Frameworks.GoogleADK == "google-adk"
        assert Frameworks.PydanticAI == "pydantic-ai"
        assert Frameworks.ClaudeAgentSDK == "claude-agent-sdk"
        assert Frameworks.OpenAI == "openai"
        assert Frameworks.Anthropic == "anthropic"
        # Hyphenated like the rest of the set, the JS SDK and the docs. This assertion
        # previously pinned "google_genai", which is what let the mismatch survive.
        assert Frameworks.GoogleGenAI == "google-genai"

    def test_all_providers_are_strings(self) -> None:
        """All provider constants should be strings."""
        assert Frameworks.Bedrock == "bedrock"


class TestResolveFrameworkInput:
    """Tests for _resolve_framework_input partitioning."""

    def test_none_returns_none_and_empty(self) -> None:
        fw, provs = _resolve_framework_input(None)
        assert fw is None
        assert provs == ()

    def test_single_framework_string(self) -> None:
        fw, provs = _resolve_framework_input("strands")
        assert fw == "strands"
        assert provs == ()

    def test_single_provider_string(self) -> None:
        fw, provs = _resolve_framework_input("bedrock")
        assert fw is None
        assert provs == ("bedrock",)

    def test_mixed_list(self) -> None:
        fw, provs = _resolve_framework_input(["strands", "bedrock"])
        assert fw == "strands"
        assert provs == ("bedrock",)

    def test_provider_only_list(self) -> None:
        fw, provs = _resolve_framework_input(["bedrock", "custom_provider"])
        assert fw is None
        assert provs == ("bedrock", "custom_provider")

    def test_two_frameworks_raises(self) -> None:
        with pytest.raises(ValueError, match="At most one framework"):
            _resolve_framework_input(["strands", "crewai"])

    def test_config_create_with_list(self) -> None:
        """Config.create should accept list input."""
        config = Config.create(framework=["strands", "bedrock"])
        assert config.framework == "strands"
        assert config.providers == ("bedrock",)

    def test_config_create_with_provider_string(self) -> None:
        """Config.create with a single provider string."""
        config = Config.create(framework="anthropic")
        assert config.framework == "anthropic"
        assert config.providers == ()


class TestContextSpanProcessor:
    """Tests for _ContextSpanProcessor."""

    def test_noop_when_no_contextvar(self) -> None:
        proc = _ContextSpanProcessor()
        span = MagicMock()
        proc.on_start(span)
        span.set_attribute.assert_not_called()

    def test_sets_attributes_from_contextvar(self) -> None:
        proc = _ContextSpanProcessor()
        span = MagicMock()
        token_u = _user_id_var.set("ctx-user")
        token_s = _session_id_var.set("ctx-session")
        try:
            proc.on_start(span)
            span.set_attribute.assert_any_call("user.id", "ctx-user")
            span.set_attribute.assert_any_call("session.id", "ctx-session")
        finally:
            _user_id_var.reset(token_u)
            _session_id_var.reset(token_s)

    def test_partial_contextvar(self) -> None:
        proc = _ContextSpanProcessor()
        span = MagicMock()
        token_u = _user_id_var.set("ctx-user")
        try:
            proc.on_start(span)
            span.set_attribute.assert_any_call("user.id", "ctx-user")
            assert span.set_attribute.call_count == 1
        finally:
            _user_id_var.reset(token_u)

    def test_force_flush_returns_true(self) -> None:
        proc = _ContextSpanProcessor()
        assert proc.force_flush() is True


class TestSpanContextVars:
    """Tests for TelemetryClient.span() contextvar management."""

    def test_span_sets_and_resets_contextvars(self) -> None:
        from sideseat.telemetry import TelemetryClient

        config = Config.create(disabled=True)
        client = TelemetryClient(config)

        assert _user_id_var.get() is None
        assert _session_id_var.get() is None

        with client.span("test", user_id="u1", session_id="s1"):
            assert _user_id_var.get() == "u1"
            assert _session_id_var.get() == "s1"

        assert _user_id_var.get() is None
        assert _session_id_var.get() is None

    def test_nested_spans_restore_previous(self) -> None:
        from sideseat.telemetry import TelemetryClient

        config = Config.create(disabled=True)
        client = TelemetryClient(config)

        with client.span("outer", user_id="outer-u", session_id="outer-s"):
            assert _user_id_var.get() == "outer-u"
            with client.span("inner", user_id="inner-u", session_id="inner-s"):
                assert _user_id_var.get() == "inner-u"
                assert _session_id_var.get() == "inner-s"
            assert _user_id_var.get() == "outer-u"
            assert _session_id_var.get() == "outer-s"

        assert _user_id_var.get() is None

    def test_span_without_ids_no_contextvar_change(self) -> None:
        from sideseat.telemetry import TelemetryClient

        config = Config.create(disabled=True)
        client = TelemetryClient(config)

        assert _user_id_var.get() is None
        with client.span("test"):
            assert _user_id_var.get() is None
        assert _user_id_var.get() is None


def test_project_id_reads_sideseat_project_id(monkeypatch):
    """SIDESEAT_PROJECT_ID is the name the TS SDK, samples and docs all use."""
    from sideseat.config import Config

    monkeypatch.delenv("SIDESEAT_PROJECT", raising=False)
    monkeypatch.setenv("SIDESEAT_PROJECT_ID", "from-project-id")
    assert Config.create(framework="strands").project_id == "from-project-id"


def test_project_id_legacy_alias_still_works(monkeypatch):
    from sideseat.config import Config

    monkeypatch.delenv("SIDESEAT_PROJECT_ID", raising=False)
    monkeypatch.setenv("SIDESEAT_PROJECT", "from-legacy")
    assert Config.create(framework="strands").project_id == "from-legacy"


def test_project_id_prefers_canonical_over_legacy(monkeypatch):
    from sideseat.config import Config

    monkeypatch.setenv("SIDESEAT_PROJECT_ID", "canonical")
    monkeypatch.setenv("SIDESEAT_PROJECT", "legacy")
    assert Config.create(framework="strands").project_id == "canonical"


def test_google_genai_constant_is_hyphenated():
    """The docs and the MCP setup_guide publish "google-genai"; the constant must match."""
    from sideseat.config import Frameworks

    assert Frameworks.GoogleGenAI == "google-genai"


def test_google_genai_legacy_underscore_still_accepted():
    """The constant used to be "google_genai"; existing code must keep working."""
    from sideseat.config import Config

    assert Config.create(framework="google_genai").framework == "google-genai"


def test_all_framework_values_use_hyphens():
    """A stray underscore silently breaks the framework comparison for anyone passing the
    documented string, so keep the whole set on one spelling."""
    from sideseat.config import Frameworks

    values = [
        v for k, v in vars(Frameworks).items() if not k.startswith("_") and isinstance(v, str)
    ]
    assert values, "no framework constants found"
    offenders = [v for v in values if "_" in v]
    assert not offenders, f"framework values must use hyphens, got {offenders}"


def test_all_extra_is_complete():
    """`sideseat[all]` is a promise. When a framework extra was added without updating
    `all`, that framework silently went uninstrumented for everyone who installed `all`."""
    from pathlib import Path

    import tomllib

    pyproject = Path(__file__).resolve().parents[1] / "pyproject.toml"
    extras = tomllib.loads(pyproject.read_text())["project"]["optional-dependencies"]

    def names(reqs):
        return {r.split(">=")[0].split("==")[0].split("[")[0].strip().lower() for r in reqs}

    all_names = names(extras["all"])
    missing = {}
    for extra, reqs in extras.items():
        if extra in ("all", "dev"):  # dev is tooling, deliberately not in `all`
            continue
        gap = names(reqs) - all_names
        if gap:
            missing[extra] = sorted(gap)
    assert not missing, f"`all` is missing deps from these extras: {missing}"


def test_trace_starts_a_root_span_even_when_nested():
    """`trace()` promises a root span. It used to delegate to `span()` verbatim, so nesting
    it inside an active span produced a child and the "groups child spans into a single
    trace" contract silently did not hold."""
    from opentelemetry import trace as otel_trace
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import SimpleSpanProcessor
    from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter

    from sideseat import SideSeat

    exporter = InMemorySpanExporter()
    provider = TracerProvider()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    tracer = provider.get_tracer("test")

    client = SideSeat(framework="strands", disabled=True)
    # Drive the assertion off a real provider rather than the disabled client's no-op tracer.
    client._telemetry.get_tracer = lambda *_a, **_k: tracer  # type: ignore[method-assign]

    with tracer.start_as_current_span("outer"):
        outer_ctx = otel_trace.get_current_span().get_span_context()
        with client.trace("new-trace") as rooted:
            rooted_ctx = rooted.get_span_context()
        with client.span("nested") as child:
            child_ctx = child.get_span_context()

    assert rooted_ctx.trace_id != outer_ctx.trace_id, "trace() must start its own trace"
    assert child_ctx.trace_id == outer_ctx.trace_id, "span() must stay in the current trace"


def test_otel_service_name_is_honoured(monkeypatch):
    """Documented as "Override service name", and the TS SDK reads it. Python ignored it,
    and `Resource.create` gives the explicit value precedence, so it silently did nothing."""
    monkeypatch.setenv("OTEL_SERVICE_NAME", "from-otel-env")
    assert Config.create(framework="strands").service_name == "from-otel-env"


def test_explicit_service_name_beats_otel_env(monkeypatch):
    monkeypatch.setenv("OTEL_SERVICE_NAME", "from-otel-env")
    cfg = Config.create(framework="strands", service_name="explicit")
    assert cfg.service_name == "explicit"


def test_otel_service_name_absent_falls_back_to_framework_package(monkeypatch):
    monkeypatch.delenv("OTEL_SERVICE_NAME", raising=False)
    assert Config.create(framework="strands").service_name == "strands-agents"


class TestOtlpEnvVars:
    """The docs advertise these OTEL_* variables; each was silently ignored."""

    def test_timeout_env_is_honoured(self, monkeypatch):
        from sideseat.telemetry.setup import build_timeout

        monkeypatch.setenv("OTEL_EXPORTER_OTLP_TIMEOUT", "7")
        assert build_timeout() == 7

    def test_timeout_defaults_when_unset(self, monkeypatch):
        from sideseat.telemetry.setup import build_timeout

        monkeypatch.delenv("OTEL_EXPORTER_OTLP_TIMEOUT", raising=False)
        assert build_timeout() == 30

    @pytest.mark.parametrize("bad", ["abc", "0", "-5", ""])
    def test_timeout_falls_back_on_invalid(self, monkeypatch, bad):
        from sideseat.telemetry.setup import build_timeout

        monkeypatch.setenv("OTEL_EXPORTER_OTLP_TIMEOUT", bad)
        assert build_timeout() == 30

    def test_env_headers_survive_alongside_api_key(self, monkeypatch):
        """The exporter only reads the env var when `headers` is falsy, so returning just
        the auth header dropped the user's env headers entirely."""
        from sideseat.telemetry.setup import build_headers

        monkeypatch.setenv("OTEL_EXPORTER_OTLP_HEADERS", "x-tenant=acme,x-team=platform")
        headers = build_headers(Config.create(framework="strands", api_key="pk-123"))
        assert headers["x-tenant"] == "acme"
        assert headers["x-team"] == "platform"
        assert headers["Authorization"] == "Bearer pk-123"

    def test_signal_specific_headers_win(self, monkeypatch):
        from sideseat.telemetry.setup import build_headers

        monkeypatch.setenv("OTEL_EXPORTER_OTLP_HEADERS", "x-src=generic")
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_TRACES_HEADERS", "x-src=traces")
        assert build_headers(Config.create(framework="strands"))["x-src"] == "traces"

    def test_malformed_header_pairs_are_skipped(self, monkeypatch):
        from sideseat.telemetry.setup import build_headers

        monkeypatch.setenv("OTEL_EXPORTER_OTLP_HEADERS", "novalue,=noname,good=yes")
        assert build_headers(Config.create(framework="strands")) == {"good": "yes"}


def test_docs_auto_detection_order_matches_code():
    """The documented detection order drifted from FRAMEWORK_PACKAGES twice (LangGraph moved
    ahead of LangChain; seven frameworks were added without touching the docs). Parse the
    published list and compare, so the docs cannot silently go stale again."""
    import re
    from pathlib import Path

    from sideseat.config import _NO_AUTO_DETECT, FRAMEWORK_PACKAGES

    doc = (
        Path(__file__).resolve().parents[3]
        / "docs/src/content/docs/docs/sdks/python/configuration.mdx"
    )
    if not doc.exists():  # docs are not shipped in the sdist
        pytest.skip("docs tree not present")

    text = doc.read_text()
    section = text.split("auto-detects by looking for the first of these packages")[1]
    section = section.split("If none match")[0]
    documented = re.findall(r"^\d+\.\s+.*?\(`([^`]+)`\)", section, re.MULTILINE)

    expected = [pkg for key, pkg in FRAMEWORK_PACKAGES if key not in _NO_AUTO_DETECT]
    assert documented == expected, (
        f"docs list {documented}\ncode order {expected}\n"
        "update configuration.mdx to match FRAMEWORK_PACKAGES"
    )


def test_readme_documents_every_framework_constant():
    """The README listed 8 of 22 constants. A user scanning it concluded the other 14
    frameworks were unsupported."""
    from pathlib import Path

    from sideseat.config import Frameworks

    readme = Path(__file__).resolve().parents[1] / "README.md"
    if not readme.exists():
        pytest.skip("README not present")
    text = readme.read_text()
    names = [k for k, v in vars(Frameworks).items() if not k.startswith("_") and isinstance(v, str)]
    missing = sorted(n for n in names if f"Frameworks.{n}" not in text)
    assert not missing, f"README does not mention: {missing}"


def test_readme_typescript_examples_pass_required_framework():
    """`framework` is required in the TypeScript SDK — a bare `init()` throws SideSeatError.
    The README shipped one, so the copy-pasted Vercel example failed immediately."""
    import re
    from pathlib import Path

    readme = Path(__file__).resolve().parents[1] / "README.md"
    if not readme.exists():
        pytest.skip("README not present")
    offenders = re.findall(r"^\s*init\(\s*\)\s*;?\s*$", readme.read_text(), re.MULTILINE)
    assert not offenders, f"bare init() calls in README: {offenders}"
