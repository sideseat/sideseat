import sys
import os
import base64
import hashlib
import json
import urllib.error
import urllib.request
from pathlib import Path

import pytest
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)
from opentelemetry.proto.common.v1.common_pb2 import AnyValue, KeyValue

from harness.matrix import census, go, gradle, npm
from harness.matrix.cli import check, matrices
from harness.matrix.shape import digest, shape, skeleton
from harness.matrix.spec import MatrixError, parse
from harness.proxy import ModelProxy

MINIMAL = """
[matrix]
package = "acme"
pin = "acme[otel]=={version}"
since = "2025-10-05"
resolved-before = "2026-10-05T00:00:00Z"
probes = ["tool_use"]
scenarios = ["tool_use", "chat"]

[matrix.profiles]
latest = { OTEL_SEMCONV_STABILITY_OPT_IN = "gen_ai_latest_experimental" }

[[variant]]
version = "1.2.0"
doc = "before the rename"

[[variant]]
version = "1.4.0"
profile = "latest"
scenarios = ["tool_use"]
doc = "the opt-in format"
"""


def test_a_variant_is_named_for_its_release_and_profile(tmp_path: Path) -> None:
    matrix = parse(MINIMAL, tmp_path)

    plain, opted = matrix.variants

    assert plain.mode("native") == "native@1.2.0"
    assert opted.mode("native") == "native@1.4.0+latest"
    assert matrix.requirements(plain) == ["acme[otel]==1.2.0"]
    assert plain.scenarios == ("tool_use", "chat")
    assert matrix.profiles["latest"] == {
        "OTEL_SEMCONV_STABILITY_OPT_IN": "gen_ai_latest_experimental"
    }


@pytest.mark.parametrize(
    ("edit", "reason"),
    [
        (
            lambda t: t.replace('package = "acme"', 'package = "acme"\nprobe = "x"'),
            "unknown",
        ),
        (lambda t: t.replace("=={version}", "==1"), "{version}"),
        (
            lambda t: t.replace('scenarios = ["tool_use"]\n', 'scenarios = ["chat"]\n'),
            "probes",
        ),
        (lambda t: t.replace('doc = "before the rename"', 'doc = " "'), "doc"),
        (lambda t: t.replace('profile = "latest"', 'profile = "other"'), "profile"),
        (lambda t: t + '\n[[variant]]\nversion = "1.2.0"\ndoc = "again"\n', "repeated"),
        (lambda t: t.replace('version = "1.2.0"', 'version = "1.2.0/x"'), "portable"),
    ],
)
def test_a_matrix_that_cannot_be_followed_is_refused(
    edit, reason: str, tmp_path: Path
) -> None:
    with pytest.raises(MatrixError, match=reason):
        parse(edit(MINIMAL), tmp_path)


def test_a_withheld_scenario_needs_a_reason_and_must_be_recorded(
    tmp_path: Path,
) -> None:
    withheld = MINIMAL.replace(
        'doc = "before the rename"',
        'doc = "before the rename"\nwithheld = { chat = "nothing reads it yet" }',
    )

    assert parse(withheld, tmp_path).variants[0].withheld == {
        "chat": "nothing reads it yet"
    }
    with pytest.raises(MatrixError, match="why"):
        parse(withheld.replace("nothing reads it yet", " "), tmp_path)
    with pytest.raises(MatrixError, match="does not record"):
        parse(withheld.replace("{ chat =", "{ multi_turn ="), tmp_path)


def test_a_recorded_release_replays_its_own_cassettes(tmp_path: Path) -> None:
    recorded = (
        MINIMAL
        + '\n[[recording]]\nversion = "1.3.0"\nreason = "calls an extra API first"\n'
    )

    matrix = parse(recorded, tmp_path)

    assert matrix.cassettes("1.3.0") == tmp_path / "cassettes@1.3.0"
    assert matrix.cassettes("1.2.0") == tmp_path / "cassettes"
    with pytest.raises(MatrixError, match="reason"):
        parse(recorded.replace("calls an extra API first", " "), tmp_path)
    exempt = '\n[[exempt]]\nversion = "1.3.0"\nreason = "x"\nrevisit = "2027-01-01"\n'
    with pytest.raises(MatrixError, match="both"):
        parse(recorded + exempt, tmp_path)


def test_a_typescript_suite_pins_npm_versions(tmp_path: Path) -> None:
    suite = tmp_path / "javascript" / "strands"
    suite.mkdir(parents=True)

    matrix = parse(MINIMAL.replace('pin = "acme[otel]=={version}"\n', ""), suite)

    assert matrix.language == "javascript"
    assert matrix.registry == "npm"
    assert matrix.pinned("1.2.0") == ["acme@1.2.0"]
    assert npm.split("@scope/name@1.2.3") == ("@scope/name", "1.2.3")
    assert npm.split("@scope/name") == ("@scope/name", None)
    assert npm.split("ai") == ("ai", None)


def test_go_and_jvm_suites_pin_modules_and_catalog_versions(tmp_path: Path) -> None:
    go_suite = tmp_path / "go" / "adk"
    java_suite = tmp_path / "java" / "adk"
    go_suite.mkdir(parents=True)
    java_suite.mkdir(parents=True)
    plain = MINIMAL.replace('pin = "acme[otel]=={version}"\n', "")

    on_go = parse(plain.replace('"acme"', '"example.com/acme"'), go_suite)
    assert (on_go.registry, on_go.pinned("1.2.0")) == ("go", ["example.com/acme@1.2.0"])
    assert go.split("example.com/acme@1.2.0") == ("example.com/acme", "1.2.0")

    with pytest.raises(MatrixError, match="catalog"):
        parse(plain, java_suite)
    on_jvm = parse(
        plain.replace(
            'package = "acme"', 'package = "com.acme:acme"\npin = "acme={version}"'
        ),
        java_suite,
    )
    assert (on_jvm.registry, on_jvm.pinned("1.2.0")) == ("maven", ["acme=1.2.0"])
    catalog = '[versions]\nacme = "1.4.0"\nacme-extra = "0.1"\n'
    assert (
        gradle.pin(catalog, "acme", "1.2.0")
        == '[versions]\nacme = "1.2.0"\nacme-extra = "0.1"\n'
    )
    with pytest.raises(ValueError, match="no version"):
        gradle.pin(catalog, "other", "1.0")


@pytest.mark.skipif(sys.platform == "win32", reason="process groups are POSIX")
def test_a_timed_out_scenario_stops_the_children_it_started(tmp_path: Path) -> None:
    import time

    from harness.matrix.run import _run

    # The scenario starts a grandchild (as npm starts the Claude Code CLI) that would outlive it.
    pid_file = tmp_path / "grandchild.pid"
    grandchild = f"import os, time; open({str(pid_file)!r}, 'w').write(str(os.getpid())); time.sleep(60)"
    command = [
        sys.executable,
        "-c",
        f"import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', {grandchild!r}]); time.sleep(60)",
    ]

    code, output = _run(command, tmp_path, dict(os.environ), timeout=2)

    assert code == -9 and "timed out" in output
    pid = int(pid_file.read_text())
    for _ in range(50):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            break
        time.sleep(0.1)
    else:
        pytest.fail("the scenario's grandchild outlived the timeout")


def test_the_census_window_ends_where_environments_resolve(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def upload(instant: str) -> list[dict[str, object]]:
        return [{"upload_time_iso_8601": instant, "yanked": False}]

    index = {
        "releases": {
            "1.0.0": upload("2025-10-01T00:00:00.000Z"),
            "1.1.0": upload("2026-10-04T23:59:59.000Z"),
            "1.2.0": upload("2026-10-05T00:00:01.000Z"),
        }
    }

    class Response:
        def __enter__(self) -> "Response":
            return self

        def __exit__(self, *exc: object) -> None:
            pass

        def read(self) -> bytes:
            return json.dumps(index).encode()

    monkeypatch.setattr(urllib.request, "urlopen", lambda *a, **k: Response())

    found = census.releases(
        "acme", "2025-10-05", prereleases=False, before="2026-10-05T00:00:00Z"
    )

    assert [r.version for r in found] == ["1.1.0"]


def test_a_skeleton_keeps_structure_and_drops_values() -> None:
    first = skeleton(
        {"role": "user", "parts": [{"text": "hi"}, {"text": "there", "n": 1}]}
    )
    second = skeleton(
        {"parts": [{"text": "other", "n": 2}, {"text": "x"}], "role": "assistant"}
    )

    assert first == second
    assert first == "{parts:[{n:num,text:str}|{text:str}],role:str}"


def _export(path: Path, spans: list[tuple[str, dict[str, str]]]) -> None:
    request = ExportTraceServiceRequest()
    scoped = request.resource_spans.add().scope_spans.add()
    scoped.scope.name = "acme.tracer"
    scoped.scope.version = "9.9.9"
    for name, attributes in spans:
        span = scoped.spans.add()
        span.name = name
        span.trace_id = bytes(16)
        for key, value in attributes.items():
            span.attributes.append(
                KeyValue(key=key, value=AnyValue(string_value=value))
            )
    path.write_bytes(request.SerializeToString())


def test_a_shape_ignores_values_ids_and_indices_but_keeps_discriminators(
    tmp_path: Path,
) -> None:
    one, two, three = (tmp_path / n for n in ("one", "two", "three"))
    for directory in (one, two, three):
        directory.mkdir()
    _export(
        one / "req-001.pb",
        [
            (
                "chat",
                {
                    "gen_ai.operation.name": "chat",
                    "llm.input_messages.0.content": "Hello",
                    "gen_ai.input.messages": '[{"role": "user", "content": "Hello"}]',
                },
            )
        ],
    )
    _export(
        two / "req-001.pb",
        [
            (
                "chat",
                {
                    "gen_ai.operation.name": "chat",
                    "llm.input_messages.3.content": "Other text",
                    "gen_ai.input.messages": '[{"role": "assistant", "content": "x"}]',
                },
            )
        ],
    )
    _export(
        three / "req-001.pb",
        [
            (
                "chat",
                {
                    "gen_ai.operation.name": "invoke_agent",
                    "llm.input_messages.0.content": "Hello",
                    "gen_ai.input.messages": '[{"role": "user", "content": "Hello"}]',
                },
            )
        ],
    )

    assert shape(one) == shape(two)
    assert digest(shape(one)) == digest(shape(two))
    assert shape(one) != shape(three), (
        "the operation name is a discriminator a predicate compares"
    )
    assert any("llm.input_messages.#.content=str" in line for line in shape(one))


def test_a_shape_ignores_a_uuid_in_a_span_name(tmp_path: Path) -> None:
    runs = []
    for name in (
        "Crew_3f2b9c1e-8d4a-4b6e-9f0a-1c2d3e4f5a6b.kickoff",
        "Crew_a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d.kickoff",
    ):
        directory = tmp_path / name
        directory.mkdir()
        _export(directory / "req-001.pb", [(name, {"gen_ai.operation.name": "chat"})])
        runs.append(shape(directory))

    assert runs[0] == runs[1]
    assert any("Crew_<uuid>.kickoff" in line for line in runs[0])


def _interaction(path: str, body: bytes, answer: str) -> dict:
    return {
        "method": "POST",
        "path": path,
        "status": 200,
        "headers": {"content-type": "application/json"},
        "body": base64.b64encode(answer.encode()).decode(),
        "request_sha256": hashlib.sha256(
            b"POST " + path.encode() + b"\n" + body
        ).hexdigest(),
    }


def _post(url: str, body: bytes) -> str:
    request = urllib.request.Request(url, data=body, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.read().decode()
    except urllib.error.HTTPError as error:
        return f"{error.code}"


def test_replay_counts_exact_and_ordered_matches_and_reports_unasked_answers(
    tmp_path: Path,
) -> None:
    cassette = tmp_path / "c.json"
    cassette.write_text(
        json.dumps(
            {
                "interactions": [
                    _interaction("/model/m/converse", b"first", "one"),
                    _interaction("/model/m/converse", b"second", "two"),
                    _interaction("/model/m/converse", b"third", "three"),
                ]
            }
        )
    )
    with ModelProxy(cassette, record=False) as proxy:
        assert _post(proxy.url + "/model/m/converse", b"second") == "two"
        # An older release serialises the request differently: answered by arrival order.
        assert _post(proxy.url + "/model/m/converse", b"FIRST, reworded") == "one"
        assert _post(proxy.url + "/model/other/converse", b"x") == "599"

        assert (proxy.exact, proxy.by_order) == (1, 1)
        assert proxy.misses == ["POST /model/other/converse"]
        assert proxy.unanswered() == ["POST /model/m/converse"]


def _census(entries: list[tuple[str, str, str | None]]) -> dict:
    return {
        "releases": [
            {
                "version": v,
                "date": "2026-01-01",
                "profile": p,
                "shape": s,
                "failure": None if s else "boom",
            }
            for v, p, s in entries
        ]
    }


def test_coverage_holds_every_classified_release_by_shape_across_profiles(
    tmp_path: Path,
) -> None:
    matrix = parse(MINIMAL, tmp_path)
    document = _census(
        [
            ("1.2.0", "default", "a"),
            (
                "1.2.0",
                "latest",
                "a",
            ),  # the opt-in is unknown to 1.2.0: its default shape
            ("1.3.0", "default", "a"),
            ("1.4.0", "default", "a"),
            ("1.4.0", "latest", "b"),
        ]
    )

    assert census.coverage(matrix, document) == []


def test_coverage_names_unheld_shapes_redundant_variants_and_unexempt_failures(
    tmp_path: Path,
) -> None:
    text = (
        MINIMAL
        + '\n[[exempt]]\nversion = "1.6.0"\nreason = "needs Python 3.15"\nrevisit = "2027-01-01"\n'
    )
    matrix = parse(text, tmp_path)
    document = _census(
        [
            ("1.2.0", "default", "a"),
            ("1.4.0", "latest", "a"),
            ("1.5.0", "default", "c"),
            ("1.6.0", "default", None),
            ("1.7.0", "default", None),
        ]
    )

    problems = census.coverage(matrix, document)

    assert any("variant 1.4.0+latest" in p and "already holds" in p for p in problems)
    assert any(
        p.startswith("1.5.0 (default): shape c has no variant") for p in problems
    )
    assert any(p.startswith("1.7.0 (default): not classified") for p in problems)
    assert not any(p.startswith("1.6.0") for p in problems)


def test_every_committed_matrix_covers_its_window_with_fresh_fixtures() -> None:
    found = matrices()
    assert found, "no suite has a versions.toml"
    problems = [
        p for producer, (_, matrix) in found.items() for p in check(producer, matrix)
    ]
    assert problems == []
