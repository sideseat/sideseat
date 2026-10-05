import getpass
import threading
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path

import pytest

from harness.capture import _Recorder, anonymise, recorded_prefix


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
    agents: dict[bytes, bytes] = {}
    first = anonymise(b"agentId: a2e3a1f7805cb8cf3 (use SendMessage)", agents)
    second = anonymise(b'a message from \\"a2e3a1f7805cb8cf3\\"', agents)

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
    _Recorder.agents = {}
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


def test_the_javascript_harness_reads_what_this_harness_says() -> None:
    from harness.capture import JAVASCRIPT_CONTENT, javascript_content

    assert JAVASCRIPT_CONTENT.read_text() == javascript_content(), (
        "examples/javascript/harness/content.json is stale; run capture --export-content"
    )


def test_javascript_suites_are_discovered_beside_python_ones(
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
    monkeypatch.setattr(capture, "PYTHON_SUITES", tmp_path / "python")
    monkeypatch.setattr(capture, "JAVASCRIPT_EXAMPLES", tmp_path / "javascript")

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
