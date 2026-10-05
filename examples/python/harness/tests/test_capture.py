import getpass
import threading
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path

import pytest

from harness.capture import Pins, _Recorder, anonymise, recorded_prefix


def test_the_cli_attachment_directory_is_pinned_at_its_length() -> None:
    raw = (
        b"[Image: source: /private/tmp/claude-504/-private-var-T-suite/"
        b"a7ad154d-40ab-4873-8951-05668b1b8aa8/images/1.jpg]"
    )

    pinned = anonymise(raw)

    assert len(pinned) == len(raw)
    assert b"/00000000-0000-0000-0000-000000000000/images/1.jpg" in pinned
    assert anonymise(pinned) == pinned


def test_other_uuids_are_left_alone() -> None:
    raw = b'{"session.id": "a7ad154d-40ab-4873-8951-05668b1b8aa8"}'

    assert anonymise(raw) == raw


def test_a_subagent_keeps_one_pinned_id_across_payloads() -> None:
    pins = Pins()
    first = anonymise(b"agentId: a2e3a1f7805cb8cf3 (use SendMessage)", pins)
    second = anonymise(b'a message from \\"a2e3a1f7805cb8cf3\\"', pins)

    assert first == b"agentId: a0000000000000001 (use SendMessage)"
    assert b"a0000000000000001" in second


def test_a_subagent_duration_is_zeroed_at_its_length() -> None:
    assert (
        anonymise(b"tool_uses: 2\\nduration_ms: 73</usage>")
        == b"tool_uses: 2\\nduration_ms: 00</usage>"
    )


def test_traces_and_logs_are_recorded_and_metrics_are_not() -> None:
    assert recorded_prefix("/v1/traces") == "req"
    assert recorded_prefix("/v1/logs") == "logs"
    assert recorded_prefix("/v1/metrics") is None


def test_a_log_export_is_written_beside_the_requests_and_anonymised(
    tmp_path: Path,
) -> None:
    _Recorder.out, _Recorder.forward, _Recorder.count = tmp_path, None, 0
    _Recorder.counts = {}
    _Recorder.pins = Pins()
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    named = (
        b'{"resourceLogs": [], "path": "/Users/' + getpass.getuser().encode() + b'/x"}'
    )
    try:
        for path, body in [
            ("/v1/logs", named),
            ("/v1/traces", b'{"resourceSpans": []}'),
            ("/v1/logs", b'{"resourceLogs": []}'),
            ("/v1/metrics", b'{"resourceMetrics": []}'),
        ]:
            request = urllib.request.Request(
                f"http://127.0.0.1:{server.server_address[1]}{path}",
                data=body,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=10) as response:
                assert response.status == 200
    finally:
        server.shutdown()

    assert sorted(p.name for p in tmp_path.iterdir()) == [
        "logs-001.json",
        "logs-002.json",
        "req-001.json",
    ]
    assert (tmp_path / "logs-001.json").read_bytes() == anonymise(named)


def test_every_other_harness_reads_what_this_harness_says() -> None:
    from harness.capture import CONTENT_TARGETS, javascript_content

    present = [target for target in CONTENT_TARGETS if target.parent.is_dir()]
    assert present, "no other language harness was found"
    for target in present:
        assert target.read_text() == javascript_content(), (
            f"{target} is stale; run capture --export-content"
        )


def test_suites_in_every_language_are_discovered_beside_python_ones(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from harness import capture

    python = tmp_path / "python" / "strands"
    python.mkdir(parents=True)
    (python / "pyproject.toml").write_text(
        '[tool.sideseat-example]\nproducer = "strands"\n'
    )
    javascript = tmp_path / "javascript" / "strands"
    javascript.mkdir(parents=True)
    (javascript / "suite.json").write_text(
        '{"producer": "strands-js", "integrations": ["strands"]}'
    )
    for language, producer in (("go", "adk-go"), ("java", "spring-ai")):
        directory = tmp_path / language / producer
        directory.mkdir(parents=True)
        (directory / "suite.json").write_text(
            f'{{"producer": "{producer}", "integrations": []}}'
        )
    monkeypatch.setattr(capture, "PYTHON_SUITES", tmp_path / "python")
    monkeypatch.setattr(
        capture,
        "MANIFEST_SUITES",
        tuple(
            (language, tmp_path / language) for language in ("javascript", "go", "java")
        ),
    )

    found = capture.suites()

    assert found["strands"].language == "python"
    assert found["strands"].sample("tool_use") == [
        "uv",
        "run",
        "--locked",
        "sample",
        "tool_use",
    ]
    assert found["strands-js"].root == javascript
    assert found["strands-js"].sample("tool_use", "--sideseat") == [
        "npm",
        "run",
        "--silent",
        "sample",
        "--",
        "tool_use",
        "--sideseat",
    ]
    assert found["adk-go"].sample("tool_use") == ["go", "run", ".", "tool_use"]
    assert found["spring-ai"].sample("tool_use", "--sideseat") == [
        str(tmp_path / "java" / "gradlew"),
        "-q",
        "--console=plain",
        "run",
        "--args=tool_use --sideseat",
    ]


def test_a_payload_holding_an_aws_credential_is_recognised() -> None:
    from harness.capture import credential_in

    access_key = b"AKIA" + b"ABCDEFGHIJKLMNOP"
    assert credential_in(b'{"key": "' + access_key + b'"}') == "an AWS access key id"
    secret = b"x" * 40
    assert credential_in(b'{\\"aws_secret_access_key\\": \\"' + secret + b'\\"}') == (
        "an AWS secret value"
    )
    reference = b'"aws_secret_access_key": {"type": "env_var", "env_vars": ["AWS_SECRET_ACCESS_KEY"]}'
    assert credential_in(reference) is None
    assert credential_in(b'{"agent_key": "74c1467e0000000000000000000000ff"}') is None


def test_a_browser_tab_keeps_one_pinned_name_across_payloads() -> None:
    pins = Pins()
    first = anonymise(b"Available tabs:\\nTab 49B3: about:blank - \\nTab C0DE: x", pins)
    second = anonymise(b'{"switch":{"tab_id":"C0DE"}} Switched to tab #49B3', pins)

    assert first == b"Available tabs:\\nTab 0001: about:blank - \\nTab 0002: x"
    assert second == b'{"switch":{"tab_id":"0002"}} Switched to tab #0001'


def test_a_tab_name_needs_its_context() -> None:
    raw = b"Table 49B3 and Tab 49B3F2"

    assert anonymise(raw, Pins()) == raw


def test_a_laminar_span_id_keeps_one_pinned_value_across_payloads() -> None:
    pins = Pins()
    first = anonymise(
        b'["00000000-0000-0000-3094-7f0f22abcda2","00000000-0000-0000-67bf-4e57113db53c"]',
        pins,
    )
    second = anonymise(b'["00000000-0000-0000-67bf-4e57113db53c"]', pins)

    assert first == (
        b'["00000000-0000-0000-0000-000000000001","00000000-0000-0000-0000-000000000002"]'
    )
    assert second == b'["00000000-0000-0000-0000-000000000002"]'


def test_the_pinned_attachment_directory_is_not_taken_for_a_span_id() -> None:
    raw = b"/claude-504/-x/a7ad154d-40ab-4873-8951-05668b1b8aa8/images/1.jpg"

    assert b"/00000000-0000-0000-0000-000000000000/images/" in anonymise(raw, Pins())
