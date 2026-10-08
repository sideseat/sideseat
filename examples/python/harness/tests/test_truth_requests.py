"""The request side of the wire: what a framework sent a model, decoded strictly and recorded per fixture."""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path

import pytest

from harness import fakes, transcript
from harness.truth import derive
from harness.truth.requests import decode_request
from harness.truth.wire import DecodeError

PNG = b"\x89PNG\r\n\x1a\nfake"


def test_converse_keeps_system_boundaries_media_and_tool_pairing() -> None:
    body = {
        "system": [{"text": "Be brief."}, {"cachePoint": {"type": "default"}}],
        "messages": [
            {
                "role": "user",
                "content": [
                    {"text": "What is this?"},
                    {
                        "image": {
                            "format": "png",
                            "source": {"bytes": base64.b64encode(PNG).decode()},
                        }
                    },
                ],
            },
            {
                "role": "assistant",
                "content": [
                    {"toolUse": {"toolUseId": "t1", "name": "look", "input": {"x": 1}}}
                ],
            },
            {
                "role": "user",
                "content": [
                    {
                        "toolResult": {
                            "toolUseId": "t1",
                            "content": [{"json": {"ok": True}}],
                            "status": "error",
                        }
                    }
                ],
            },
        ],
        "toolConfig": {"tools": [{"toolSpec": {"name": "look"}}]},
    }
    request = decode_request("POST", "/model/m/converse", json.dumps(body).encode())
    assert request is not None
    assert request.system == [{"type": "text", "text": "Be brief."}]
    assert [m["role"] for m in request.messages] == ["user", "assistant", "user"]
    image = request.messages[0]["parts"][1]
    assert image["sha256"] == hashlib.sha256(PNG).hexdigest()
    assert image["size"] == len(PNG) and image["media_type"] == "image/png"
    assert request.messages[1]["parts"] == [
        {"type": "tool_call", "id": "t1", "name": "look", "arguments": {"x": 1}}
    ]
    result = request.messages[2]["parts"][0]
    assert result["id"] == "t1" and result["is_error"]
    assert request.tools == ["look"]


def test_chat_moves_system_out_of_the_conversation_and_parses_arguments() -> None:
    body = {
        "model": "gpt",
        "messages": [
            {"role": "system", "content": "Be brief."},
            {"role": "user", "content": [{"type": "text", "text": "Hi"}]},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "f", "arguments": '{"a": 2}'},
                    }
                ],
            },
            {"role": "tool", "tool_call_id": "c1", "content": "done"},
        ],
    }
    request = decode_request("POST", "/v1/chat/completions", json.dumps(body).encode())
    assert request is not None
    assert request.system == [{"type": "text", "text": "Be brief."}]
    assert request.messages[1]["parts"][0]["arguments"] == {"a": 2}
    assert request.messages[2]["parts"][0]["id"] == "c1"


def test_responses_and_gemini_requests_decode() -> None:
    responses = {
        "instructions": "Be brief.",
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "Hi"}]},
            {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": "ok"},
        ],
    }
    request = decode_request("POST", "/v1/responses", json.dumps(responses).encode())
    assert request is not None
    assert request.system[0]["text"] == "Be brief."
    assert [m["role"] for m in request.messages] == ["user", "assistant", "tool"]
    gemini = {
        "systemInstruction": {"parts": [{"text": "Be brief."}]},
        "contents": [
            {"role": "user", "parts": [{"text": "Hi"}]},
            {"role": "model", "parts": [{"functionCall": {"name": "f", "args": {}}}]},
        ],
        "tools": [{"functionDeclarations": [{"name": "f"}]}],
    }
    request = decode_request(
        "POST", "/v1beta/models/gemini-x:generateContent", json.dumps(gemini).encode()
    )
    assert request is not None
    assert request.tools == ["f"] and request.messages[1]["parts"][0]["id"] is None


def test_a_responses_attachment_without_its_bytes_is_a_reference() -> None:
    # An image by URL or by upload id, and a file by upload id or location: the request says where each
    # one is, and a file's kind is not stated.
    responses = {
        "input": [
            {
                "role": "user",
                "content": [
                    {"type": "input_image", "image_url": "https://x/y.jpg"},
                    {"type": "input_image", "file_id": "file-img"},
                    {"type": "input_file", "file_id": "file-doc"},
                    {"type": "input_file", "file_url": "https://x/z.pdf"},
                ],
            }
        ]
    }
    request = decode_request("POST", "/v1/responses", json.dumps(responses).encode())
    assert request is not None
    assert [
        (p["modality"], p["media_type"], p["source"], p["reference"])
        for p in request.messages[0]["parts"]
    ] == [
        ("image", None, "url", "https://x/y.jpg"),
        ("image", None, "file_id", "file-img"),
        ("file", None, "file_id", "file-doc"),
        ("file", None, "url", "https://x/z.pdf"),
    ]
    empty = {
        "input": [{"role": "user", "content": [{"type": "input_file", "file_id": ""}]}]
    }
    with pytest.raises(DecodeError):
        decode_request("POST", "/v1/responses", json.dumps(empty).encode())


def test_gemini_inline_data_names_the_modality_every_other_api_does() -> None:
    """A PDF is a document, as Converse, Messages and Chat Completions call it - not `application`."""
    pdf = b"%PDF-1.3 fake"
    gemini = {
        "contents": [
            {
                "role": "user",
                "parts": [
                    {
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": base64.b64encode(PNG).decode(),
                        }
                    },
                    {
                        "inlineData": {
                            "mime_type": "application/pdf",
                            "data": base64.b64encode(pdf).decode(),
                        }
                    },
                ],
            }
        ],
    }
    request = decode_request(
        "POST", "/v1beta/models/gemini-x:generateContent", json.dumps(gemini).encode()
    )
    assert request is not None
    image, document = request.messages[0]["parts"]
    assert image["modality"] == "image"
    assert document["modality"] == "document"
    assert document["media_type"] == "application/pdf"
    assert document["sha256"] == hashlib.sha256(pdf).hexdigest()


@pytest.mark.parametrize(
    "fixture",
    [
        "google-genai/native/files",
        "vertex-ai/native/files",
        "adk-go/native/files",
        "genkit-go/native/files",
    ],
)
def test_captured_gemini_media_decode_as_the_request_bytes_and_the_script_say(
    fixture: str,
) -> None:
    """Each inline attachment a captured request sent decodes to the MIME type its bytes were sent under,
    and to the modality and type of the script's asset with those bytes - the fact the conversation states.

    Google's Python client spells the member `mime_type`, the Go clients `mimeType`; the decoder read only
    the second, and named a PDF's modality after its MIME type's top level (`application`) where every other
    API's decoder, and the conversation's own fact, call it a document.
    """
    path = (
        derive.REPO / "server/tests/fixtures/messages" / fixture / "model-requests.json"
    )
    assets = {fact["sha256"]: fact for fact in derive.media_facts()}
    seen = 0
    for interaction in json.loads(path.read_text())["interactions"]:
        body = base64.b64decode(interaction["body"])
        sent = [
            part["inlineData"]
            for content in json.loads(body)["contents"]
            for part in content["parts"]
            if "inlineData" in part
        ]
        request = decode_request(interaction["method"], interaction["path"], body)
        assert request is not None
        decoded = [
            part
            for message in request.messages
            for part in message["parts"]
            if part["type"] == "media"
        ]
        assert len(decoded) == len(sent)
        for wire, part in zip(sent, decoded, strict=True):
            assert part["media_type"] == (wire.get("mimeType") or wire.get("mime_type"))
            asset = assets[part["sha256"]]
            assert (part["modality"], part["media_type"]) == (
                asset["modality"],
                asset["media_type"],
            )
            seen += 1
    assert seen >= 2


@pytest.mark.parametrize(
    ("path", "body"),
    [
        (
            "/model/m/converse",
            {"messages": [{"role": "user", "content": [{"weird": {}}]}]},
        ),
        (
            "/v1/messages",
            {"messages": [{"role": "user", "content": [{"type": "weird"}]}]},
        ),
        (
            "/v1/chat/completions",
            {"messages": [{"role": "user", "content": [{"type": "weird"}]}]},
        ),
        ("/v1/responses", {"input": [{"type": "weird"}]}),
        (
            "/v1/chat/completions",
            {
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "image_url",
                                "image_url": {"url": "https://x/y.png"},
                            }
                        ],
                    }
                ]
            },
        ),
    ],
)
def test_an_unknown_or_remote_part_is_an_error(path: str, body: dict) -> None:
    with pytest.raises(DecodeError):
        decode_request("POST", path, json.dumps(body).encode())


def test_a_request_to_no_model_is_none() -> None:
    assert decode_request("GET", "/health", b"") is None


def test_the_transcript_keeps_requests_in_order_scrubbed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    log = tmp_path / "log.jsonl"
    monkeypatch.setenv(transcript.ENV, str(log))
    monkeypatch.setattr("harness.scrub.account_names", lambda: [b"someone"])
    transcript.record(
        "POST", "/a", b'{"path": "/Users/someone/x"}', "application/json", answered_by=3
    )
    transcript.record("POST", "/b", b"{}", "application/json")
    document = transcript.finish(log)
    assert document["format"] == transcript.FORMAT
    first, second = document["interactions"]
    assert (first["path"], first["answered_by"]) == (
        "/a",
        3,
    ) and "answered_by" not in second
    assert b"someone" not in base64.b64decode(first["body"])
    assert first["request_sha256"] == transcript.digest(
        "POST", "/a", b'{"path": "/Users/someone/x"}'
    )
    out = tmp_path / transcript.FILENAME
    out.write_text(json.dumps(document))
    assert [e["path"] for e in transcript.load(out)] == ["/a", "/b"]


def test_a_fake_model_records_what_it_was_sent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import urllib.request

    log = tmp_path / "log.jsonl"
    monkeypatch.setenv(transcript.ENV, str(log))
    url = fakes.start("fake-openai")
    body = json.dumps(
        {"model": "fake", "messages": [{"role": "user", "content": "Hello"}]}
    ).encode()
    request = urllib.request.Request(
        url + "/v1/chat/completions",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        assert response.status == 200
    (entry,) = transcript.finish(log)["interactions"]
    assert base64.b64decode(entry["body"]) == body
    assert entry["path"] == "/v1/chat/completions"


def _converse(messages: list[dict], system: str = "Be brief.") -> bytes:
    return json.dumps({"system": [{"text": system}], "messages": messages}).encode()


def test_lineage_comes_from_the_requests_and_the_outputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from harness.truth import request_truth

    monkeypatch.setattr(request_truth, "FIXTURES", tmp_path)
    monkeypatch.setattr(request_truth, "REPO", tmp_path)
    fixture = tmp_path / "p" / "native" / "s"
    fixture.mkdir(parents=True)
    user = {"role": "user", "content": [{"text": "Hi"}]}
    call = {
        "role": "assistant",
        "content": [{"toolUse": {"toolUseId": "t1", "name": "f", "input": {}}}],
    }
    result = {
        "role": "user",
        "content": [{"toolResult": {"toolUseId": "t1", "content": [{"text": "ok"}]}}],
    }
    summary = {"role": "assistant", "content": [{"text": "A summary nobody said."}]}
    log = tmp_path / "log.jsonl"
    for index, messages in enumerate([[user], [user, call, result], [summary, user]]):
        transcript.record(
            "POST",
            "/model/m/converse",
            _converse(messages),
            "application/json",
            answered_by=index,
            log=str(log),
        )
    (fixture / transcript.FILENAME).write_text(json.dumps(transcript.finish(log)))
    truth = {
        "calls": [
            {"id": "call-001", "conversation": "c", "outputs": ["fact-002"]},
            {"id": "call-002", "conversation": "c", "outputs": []},
            {"id": "call-003", "conversation": "c", "outputs": []},
        ],
        "facts": [
            {
                "id": "fact-001",
                "kind": "user_text",
                "conversation": "c",
                "value": {"text": "Hi"},
            },
            {
                "id": "fact-002",
                "kind": "tool_call",
                "conversation": "c",
                "value": {"id": "t1", "name": "f", "arguments": {}},
            },
            {
                "id": "fact-003",
                "kind": "tool_result",
                "conversation": "c",
                "value": {"call_id": "t1", "name": "f", "value": "ok"},
            },
        ],
        "conversations": [
            {"id": "c", "sequence": ["fact-001", "fact-002", "fact-003"]}
        ],
        "edges": [],
    }
    recorded = request_truth.fixture_requests(
        truth, "p/native/s", {0: "call-001", 1: "call-002", 2: "call-003"}
    )
    assert recorded is not None and recorded["unpaired_requests"] == []
    calls = recorded["calls"]

    def lineage(call: str) -> list[dict]:
        return [
            {k: v for k, v in part.items() if k != "part"}
            for message in calls[call]["messages"]
            for part in message["parts"]
        ]

    # The system instruction only a request carries becomes a fact of this fixture, introduced before
    # anything the conversation had said, and asserted wherever the conversation is shown.
    assert calls["call-001"]["system"][0]["new_fact"] == "fact-004"
    minted = truth["facts"][-1]
    assert minted["id"] == "fact-004" and minted["fixtures"] == ["p/native/s"]
    assert minted["kind"] == "system" and minted["require"]["anchor"] == "conversation"
    assert truth["conversations"][0]["sequence"][0] == "fact-004"
    assert lineage("call-001") == [{"new_fact": "fact-001"}]
    assert lineage("call-002") == [
        {"replay_of": "fact-001"},
        {"replay_of": "fact-002"},
        {"new_fact": "fact-003"},
    ]
    assert calls["call-003"]["system"][0] == {
        "part": {"type": "text", "text": "Be brief."},
        "replay_of": "fact-004",
    }
    # The summary is model-side content no output holds: its lineage is unknown, never "new".
    assert "lineage_unknown" in lineage("call-003")[0]
    assert lineage("call-003")[1] == {"replay_of": "fact-001"}


def test_a_fixture_without_a_transcript_has_no_request_truth(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from harness.truth import request_truth

    monkeypatch.setattr(request_truth, "FIXTURES", tmp_path)
    assert (
        request_truth.fixture_requests({"calls": [], "facts": []}, "p/native/s", None)
        is None
    )


def test_request_only_copies_are_facts_each_and_history_is_a_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from harness.truth import request_truth

    monkeypatch.setattr(request_truth, "FIXTURES", tmp_path)
    monkeypatch.setattr(request_truth, "REPO", tmp_path)
    fixture = tmp_path / "p" / "native" / "s"
    fixture.mkdir(parents=True)
    note = {"text": "For context: another agent spoke before you."}
    first = {"role": "user", "content": [{"text": "Hi"}]}
    answer = {"role": "assistant", "content": [{"text": "Hello there."}]}
    second = {"role": "user", "content": [note, note, {"text": "And now?"}]}
    log = tmp_path / "log.jsonl"
    for index, messages in enumerate([[first], [first, answer, second]]):
        transcript.record(
            "POST",
            "/model/m/converse",
            _converse(messages),
            "application/json",
            answered_by=index,
            log=str(log),
        )
    (fixture / transcript.FILENAME).write_text(json.dumps(transcript.finish(log)))

    def fact(identifier: str, kind: str, conversation: str, text: str) -> dict:
        value = {"text": text}
        return {
            "id": identifier,
            "kind": kind,
            "conversation": conversation,
            "value": value,
        }

    truth = {
        "calls": [
            {"id": "call-001", "conversation": "c1", "outputs": ["fact-002"]},
            {"id": "call-002", "conversation": "c2", "outputs": []},
        ],
        "facts": [
            fact("fact-001", "user_text", "c1", "Hi"),
            fact("fact-002", "text", "c1", "Hello there."),
            fact("fact-003", "user_text", "c2", "And now?"),
        ],
        "conversations": [
            {"id": "c1", "sequence": ["fact-001", "fact-002"]},
            {"id": "c2", "sequence": ["fact-003"]},
        ],
        "edges": [],
    }
    recorded = request_truth.fixture_requests(
        truth, "p/native/s", {0: "call-001", 1: "call-002"}
    )
    assert recorded is not None
    parts = [
        {k: v for k, v in part.items() if k != "part"}
        for message in recorded["calls"]["call-002"]["messages"]
        for part in message["parts"]
    ]
    # An earlier conversation's turns, handed to the next one, are history: replays, not new facts.
    assert parts[0] == {"replay_of": "fact-001"}
    assert parts[1] == {"replay_of": "fact-002"}
    # One request sending the same note twice sent two parts, and each is a fact of its own.
    assert parts[2] == {"new_fact": "fact-006"} and parts[3] == {"new_fact": "fact-007"}
    assert parts[4] == {"new_fact": "fact-003"}
    # The instruction every request carries is each conversation's own, not one conversation's history.
    systems = [
        recorded["calls"][call]["system"][0]["new_fact"]
        for call in ("call-001", "call-002")
    ]
    assert systems == ["fact-004", "fact-005"]


def test_a_thread_is_named_by_its_instruction_whatever_order_requests_arrive_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from harness.truth import request_truth

    monkeypatch.setattr(request_truth, "FIXTURES", tmp_path)
    monkeypatch.setattr(request_truth, "REPO", tmp_path)
    note = {"role": "user", "content": [{"text": "Hand your report back when done."}]}
    lead, helper = "You lead the work.", "You help the lead."

    def record(fixture: str, systems: list[str]) -> None:
        directory = tmp_path / fixture
        directory.mkdir(parents=True)
        log = directory / "log.jsonl"
        for index, system in enumerate(systems):
            transcript.record(
                "POST",
                "/model/m/converse",
                _converse([note], system=system),
                "application/json",
                answered_by=index,
                log=str(log),
            )
        (directory / transcript.FILENAME).write_text(json.dumps(transcript.finish(log)))

    # Two concurrent subagents' first requests, which the two runs received in opposite orders.
    record("p/native/s", [lead, helper])
    record("p/sdk/s", [helper, lead])
    truth = {
        "calls": [
            {"id": "call-001", "conversation": "c", "outputs": []},
            {"id": "call-002", "conversation": "c", "outputs": []},
        ],
        "facts": [],
        "conversations": [{"id": "c", "sequence": []}],
        "edges": [],
    }
    calls = {0: "call-001", 1: "call-002"}
    native = request_truth.fixture_requests(truth, "p/native/s", calls)
    sdk = request_truth.fixture_requests(truth, "p/sdk/s", calls)
    assert native is not None and sdk is not None

    def note_of(recorded: dict, instruction: str) -> str:
        for request in recorded["calls"].values():
            if request["system"][0]["part"]["text"] == instruction:
                (part,) = request["messages"][0]["parts"]
                return part.get("new_fact") or part["replay_of"]
        raise AssertionError(instruction)

    # Each thread's note is one fact, the same in both runs, and the two threads' notes are two.
    assert note_of(native, lead) == note_of(sdk, lead)
    assert note_of(native, helper) == note_of(sdk, helper)
    assert note_of(native, lead) != note_of(native, helper)


def test_withheld_reasoning_replays_the_response_its_message_replays(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from harness.truth import request_truth

    monkeypatch.setattr(request_truth, "FIXTURES", tmp_path)
    monkeypatch.setattr(request_truth, "REPO", tmp_path)
    fixture = tmp_path / "p" / "native" / "s"
    fixture.mkdir(parents=True)

    def withheld(signature: str) -> dict:
        return {
            "reasoningContent": {"reasoningText": {"text": "", "signature": signature}}
        }

    def call(identifier: str) -> dict:
        return {"toolUse": {"toolUseId": identifier, "name": "f", "input": {}}}

    def result(identifier: str) -> dict:
        return {
            "role": "user",
            "content": [
                {"toolResult": {"toolUseId": identifier, "content": [{"text": "ok"}]}}
            ],
        }

    user = {"role": "user", "content": [{"text": "Hi"}]}
    first = {"role": "assistant", "content": [withheld("s1"), call("t1")]}
    second = {"role": "assistant", "content": [withheld("s2"), call("t2")]}
    # Reasoning re-sent alone: no other part says which response it was.
    alone = {"role": "assistant", "content": [withheld("s3")]}
    log = tmp_path / "log.jsonl"
    requests = [
        [user],
        [user, first, result("t1")],
        [user, first, result("t1"), second, result("t2"), alone, user],
    ]
    for index, messages in enumerate(requests):
        transcript.record(
            "POST",
            "/model/m/converse",
            _converse(messages),
            "application/json",
            answered_by=index,
            log=str(log),
        )
    (fixture / transcript.FILENAME).write_text(json.dumps(transcript.finish(log)))

    def fact(identifier: str, kind: str, value: dict) -> dict:
        return {"id": identifier, "kind": kind, "conversation": "c", "value": value}

    reasoning = {"text": "", "signed": True, "redacted": False}
    truth = {
        "calls": [
            {
                "id": "call-001",
                "conversation": "c",
                "outputs": ["fact-002", "fact-003"],
            },
            {
                "id": "call-002",
                "conversation": "c",
                "outputs": ["fact-005", "fact-006"],
            },
            {"id": "call-003", "conversation": "c", "outputs": []},
        ],
        "facts": [
            fact("fact-001", "user_text", {"text": "Hi"}),
            fact("fact-002", "reasoning", reasoning),
            fact("fact-003", "tool_call", {"id": "t1", "name": "f", "arguments": {}}),
            fact(
                "fact-004", "tool_result", {"call_id": "t1", "name": "f", "value": "ok"}
            ),
            fact("fact-005", "reasoning", reasoning),
            fact("fact-006", "tool_call", {"id": "t2", "name": "f", "arguments": {}}),
            fact(
                "fact-007", "tool_result", {"call_id": "t2", "name": "f", "value": "ok"}
            ),
        ],
        "conversations": [
            {"id": "c", "sequence": [f"fact-{n:03d}" for n in range(1, 8)]}
        ],
        "edges": [],
    }
    recorded = request_truth.fixture_requests(
        truth, "p/native/s", {0: "call-001", 1: "call-002", 2: "call-003"}
    )
    assert recorded is not None
    messages = recorded["calls"]["call-003"]["messages"]

    def lineage(message: int) -> list[dict]:
        return [
            {k: v for k, v in part.items() if k != "part"}
            for part in messages[message]["parts"]
        ]

    # Each withheld part is the reasoning of the response the rest of its message replays, whatever
    # its place among the conversation's other withheld reasoning.
    assert lineage(1) == [{"replay_of": "fact-002"}, {"replay_of": "fact-003"}]
    assert lineage(3) == [{"replay_of": "fact-005"}, {"replay_of": "fact-006"}]
    assert list(lineage(5)[0]) == ["lineage_unknown"]
    assert messages[1]["parts"][0]["part"] == {
        "type": "reasoning",
        "text": "",
        "signed": True,
    }
