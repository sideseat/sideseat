"""Tests for SideSeat configuration."""

import pytest

from sideseat.config import (
    Config,
    Frameworks,
    _detect_framework,
    _normalize_endpoint,
    _parse_bool_env,
)


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
        assert Frameworks.CrewAI == "crewai"
        assert Frameworks.AutoGen == "autogen"
        assert Frameworks.OpenAIAgents == "openai-agents"
        assert Frameworks.GoogleADK == "google-adk"
        assert Frameworks.PydanticAI == "pydantic-ai"
