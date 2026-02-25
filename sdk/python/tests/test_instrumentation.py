"""Tests for framework instrumentation."""

import threading

import pytest

from sideseat.config import Frameworks
from sideseat.instrumentation import (
    LOGFIRE_FRAMEWORKS,
    _instrumented,
    _lock,
    instrument,
    is_logfire_framework,
)


@pytest.fixture(autouse=True)
def reset_instrumentation_state() -> None:
    """Reset instrumentation state between tests."""
    with _lock:
        _instrumented.clear()


class TestIsLogfireFramework:
    """Tests for Logfire framework detection."""

    def test_openai_agents_is_logfire(self) -> None:
        """OpenAI Agents should use Logfire."""
        assert is_logfire_framework(Frameworks.OpenAIAgents) is True

    def test_pydantic_ai_is_logfire(self) -> None:
        """PydanticAI should use Logfire."""
        assert is_logfire_framework(Frameworks.PydanticAI) is True

    def test_openai_is_logfire(self) -> None:
        """OpenAI should use Logfire."""
        assert is_logfire_framework(Frameworks.OpenAI) is True

    def test_anthropic_is_logfire(self) -> None:
        """Anthropic should use Logfire."""
        assert is_logfire_framework(Frameworks.Anthropic) is True

    def test_other_frameworks_not_logfire(self) -> None:
        """Other frameworks should not use Logfire."""
        assert is_logfire_framework(Frameworks.Strands) is False
        assert is_logfire_framework(Frameworks.LangChain) is False
        assert is_logfire_framework(Frameworks.CrewAI) is False
        assert is_logfire_framework(Frameworks.AutoGen) is False
        assert is_logfire_framework(Frameworks.GoogleADK) is False

    def test_logfire_frameworks_frozenset(self) -> None:
        """LOGFIRE_FRAMEWORKS should be a frozenset."""
        assert isinstance(LOGFIRE_FRAMEWORKS, frozenset)
        assert Frameworks.OpenAIAgents in LOGFIRE_FRAMEWORKS
        assert Frameworks.PydanticAI in LOGFIRE_FRAMEWORKS
        assert Frameworks.OpenAI in LOGFIRE_FRAMEWORKS
        assert Frameworks.Anthropic in LOGFIRE_FRAMEWORKS


class TestInstrument:
    """Tests for the instrument() function."""

    def test_strands_no_op(self) -> None:
        """Strands instrumentation should be a no-op (uses global provider)."""
        result = instrument(Frameworks.Strands, None)
        assert result is True
        assert Frameworks.Strands in _instrumented

    def test_google_adk_no_op(self) -> None:
        """Google ADK instrumentation should be a no-op (uses global provider)."""
        result = instrument(Frameworks.GoogleADK, None)
        assert result is True
        assert Frameworks.GoogleADK in _instrumented

    def test_double_instrumentation_blocked(self) -> None:
        """Second instrumentation attempt should be skipped."""
        # First call
        result1 = instrument(Frameworks.Strands, None)
        assert result1 is True

        # Second call should be blocked
        result2 = instrument(Frameworks.Strands, None)
        assert result2 is False

    def test_unknown_framework(self) -> None:
        """Unknown framework should return False."""
        result = instrument("unknown-framework", None)
        assert result is False
        assert "unknown-framework" not in _instrumented

    def test_missing_deps_graceful(self) -> None:
        """Missing instrumentation deps should not crash."""
        # LangChain instrumentation requires openinference-instrumentation-langchain
        # which is likely not installed in test env
        result = instrument(Frameworks.LangChain, None)
        # Result depends on whether deps are installed
        assert isinstance(result, bool)

    def test_thread_safety(self) -> None:
        """Instrumentation should be thread-safe."""
        results: list[bool] = []
        errors: list[Exception] = []

        def try_instrument() -> None:
            try:
                result = instrument(Frameworks.Strands, None)
                results.append(result)
            except Exception as e:
                errors.append(e)

        threads = [threading.Thread(target=try_instrument) for _ in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        # No errors should occur
        assert len(errors) == 0
        # Only one thread should successfully instrument
        assert sum(results) == 1
        assert Frameworks.Strands in _instrumented
