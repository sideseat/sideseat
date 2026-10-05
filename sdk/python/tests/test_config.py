from __future__ import annotations

import pytest

from sideseat import _config
from sideseat.errors import ConfigurationError


def settings(**overrides: object) -> _config.Settings:
    args: dict[str, object] = {
        "endpoint": None,
        "project": None,
        "api_key": None,
        "service_name": None,
        "service_version": None,
        "integrations": None,
        "capture_content": None,
        "disabled": None,
        "debug": None,
        "export": True,
        "metrics": True,
        "logs": True,
        "capture_python_logs": False,
        "resource_attributes": None,
        "span_processors": None,
    }
    args.update(overrides)
    return _config.resolve(**args)  # type: ignore[arg-type]


def test_defaults_point_at_the_local_server_and_default_project() -> None:
    s = settings()
    assert s.endpoint == "http://127.0.0.1:5388"
    assert s.signal_endpoint("traces") == "http://127.0.0.1:5388/otel/default/v1/traces"
    assert s.capture_content is True
    assert s.disabled is False
    assert s.integrations is None


def test_an_endpoint_with_a_path_is_already_an_otlp_base() -> None:
    s = settings(endpoint="https://collector.example.com/otel/team-a/", project="ignored")
    assert s.signal_endpoint("logs") == "https://collector.example.com/otel/team-a/v1/logs"


def test_arguments_win_over_environment_which_wins_over_defaults(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SIDESEAT_ENDPOINT", "http://env:1")
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel:2")
    monkeypatch.setenv("SIDESEAT_PROJECT_ID", "from-env")
    assert settings().endpoint == "http://env:1"
    assert settings().project == "from-env"
    assert settings(endpoint="http://arg:3", project="arg").signal_endpoint("traces") == (
        "http://arg:3/otel/arg/v1/traces"
    )
    monkeypatch.delenv("SIDESEAT_ENDPOINT")
    assert settings().endpoint == "http://otel:2"


@pytest.mark.parametrize("raw", ["ftp://host", "localhost:5388", "http://"])
def test_an_endpoint_that_is_not_http_is_rejected(raw: str) -> None:
    with pytest.raises(ConfigurationError):
        settings(endpoint=raw)


@pytest.mark.parametrize(
    ("raw", "value"), [("1", True), ("YES", True), ("false", False), ("0", False)]
)
def test_boolean_variables_accept_the_documented_spellings(
    monkeypatch: pytest.MonkeyPatch, raw: str, value: bool
) -> None:
    monkeypatch.setenv("SIDESEAT_DISABLED", raw)
    assert settings().disabled is value


def test_an_invalid_boolean_is_an_error_not_a_default(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIDESEAT_CAPTURE_CONTENT", "sometimes")
    with pytest.raises(ConfigurationError, match="SIDESEAT_CAPTURE_CONTENT"):
        settings()


def test_the_api_key_joins_and_overrides_otlp_headers(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_HEADERS", "x-team=a%20b,Authorization=old")
    assert settings(api_key="k").headers() == {"x-team": "a b", "Authorization": "Bearer k"}


def test_integrations_come_from_the_environment_as_a_list(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIDESEAT_INTEGRATIONS", "strands, bedrock,")
    assert settings().integrations == ("strands", "bedrock")
    assert settings(integrations="openai").integrations == ("openai",)


def test_the_api_key_replaces_an_authorization_header_of_any_spelling(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_HEADERS", "authorization=old,AUTHORIZATION=older,x-a=1")
    assert settings(api_key="k").headers() == {"x-a": "1", "Authorization": "Bearer k"}
    # Without a key the application's own credential is kept as written.
    assert settings().headers()["authorization"] == "old"


def test_blank_arguments_and_variables_count_as_unset(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIDESEAT_ENDPOINT", "  ")
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel:2")
    monkeypatch.setenv("SIDESEAT_PROJECT_ID", " ")
    monkeypatch.setenv("SIDESEAT_API_KEY", "\t")
    monkeypatch.setenv("OTEL_SERVICE_NAME", " ")
    monkeypatch.setenv("OTEL_SERVICE_VERSION", " ")
    monkeypatch.setenv("SIDESEAT_INTEGRATIONS", " ")
    s = settings(endpoint=" ", project=" ", api_key=" ", service_name=" ", integrations=" ")
    assert s.endpoint == "http://otel:2"
    assert s.project == "default"
    assert s.api_key is None
    assert s.service_name is None
    assert s.service_version is None
    assert s.integrations is None
    assert "Authorization" not in s.headers()


def test_a_blank_argument_falls_back_to_the_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIDESEAT_PROJECT_ID", " team-a ")
    monkeypatch.setenv("SIDESEAT_API_KEY", "env-key")
    monkeypatch.setenv("SIDESEAT_INTEGRATIONS", "strands")
    s = settings(project="  ", api_key="", integrations="")
    assert (s.project, s.api_key, s.integrations) == ("team-a", "env-key", ("strands",))
    # An empty sequence is not blank: it asks for no integrations at all.
    assert settings(integrations=[]).integrations == ()


def test_the_project_is_one_url_path_segment() -> None:
    s = settings(project="team a/b")
    assert s.signal_endpoint("traces") == "http://127.0.0.1:5388/otel/team%20a%2Fb/v1/traces"


def test_debug_and_span_processors_are_part_of_the_init_identity() -> None:
    processor = object()
    base = settings(span_processors=[processor])
    assert base.identity() == settings(span_processors=[processor]).identity()
    assert base.identity() != settings(span_processors=[object()]).identity()
    assert base.identity() != settings(span_processors=[processor], debug=True).identity()
