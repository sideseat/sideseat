"""What the fake OpenAI endpoint answers is what OpenAI sends: the SDK's own types validate it.

A scripted response is hand-written, so each shape the fake serves for a provider feature is proven
against the OpenAI SDK, whose types are generated from the API's specification. The client itself
builds its objects without validating them, so these tests take the fake's raw JSON and validate it
against the SDK's types: a required member missing, or a value of the wrong type, fails here.

Each test validates the items and events the provider features add. The response envelope around them
is older than these features and predates the SDK's ``usage.input_tokens_details.cache_write_tokens``,
which the fake does not report; it is left as it is, because every captured fake suite replays it.
"""

from __future__ import annotations

import io
import json
import urllib.request
from typing import Any

import openai
import pytest
from openai.types import FileObject
from openai.types.responses import (
    ResponseFunctionWebSearch,
    ResponseOutputMessage,
    ResponseStreamEvent,
)
from openai.types.responses.response_function_web_search import ActionSearch
from openai.types.responses.response_output_text import AnnotationURLCitation
from pydantic import TypeAdapter, ValidationError

from harness import Run, clients, content
from harness.fakes import openai as fake
from harness.fakes import script

EVENTS: TypeAdapter[Any] = TypeAdapter(ResponseStreamEvent)


def base() -> str:
    return clients.fake_url("fake-openai")


def client() -> Any:
    return openai.OpenAI(base_url=base(), api_key="fake")


def post(body: dict[str, Any]) -> str:
    request = urllib.request.Request(
        f"{base()}/responses",
        data=json.dumps({"model": "fake", **body}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return str(response.read().decode())


#: How each output item the tests read is typed: the SDK's own models.
ITEMS: dict[str, Any] = {
    "web_search_call": ResponseFunctionWebSearch,
    "message": ResponseOutputMessage,
}


def items(output: list[dict[str, Any]]) -> list[Any]:
    """Each output item, validated as the SDK's model for its type."""
    return [ITEMS[item["type"]].model_validate(item) for item in output]


def respond(**body: Any) -> list[Any]:
    """The output of the fake's answer to a Responses request, each item validated."""
    return items(json.loads(post(body))["output"])


def search(**body: Any) -> list[Any]:
    return respond(input=content.SERVER_TOOLS, tools=[{"type": "web_search"}], **body)


#: The stream events the provider features add, validated as the SDK's own event types.
FEATURE_EVENTS = (
    "response.web_search_call.in_progress",
    "response.web_search_call.searching",
    "response.web_search_call.completed",
    "response.output_text.annotation.added",
)


def stream(**body: Any) -> list[dict[str, Any]]:
    """The fake's streamed answer, every event a provider feature adds validated."""
    raw = post({"stream": True, **body})
    events = [
        json.loads(line.removeprefix("data: "))
        for line in raw.splitlines()
        if line.startswith("data: ")
    ]
    for event in events:
        if event["type"] in FEATURE_EVENTS:
            EVENTS.validate_python(event)
    return events


def test_a_web_search_is_a_search_call_before_a_cited_answer() -> None:
    searched, message = search()
    assert isinstance(searched, ResponseFunctionWebSearch)
    assert searched.status == "completed"
    assert isinstance(searched.action, ActionSearch)
    query, sources = script.SEARCHES[content.SERVER_TOOLS]
    assert searched.action.query == query
    assert searched.action.queries == [query]
    assert [source.url for source in searched.action.sources or []] == sources
    assert isinstance(message, ResponseOutputMessage)
    (text,) = message.content
    assert text.text == script.ANSWERS[content.SERVER_TOOLS]
    (citation,) = text.annotations
    assert isinstance(citation, AnnotationURLCitation)
    assert (citation.start_index, citation.end_index) == (0, len(text.text))
    assert citation.url == sources[0]


def test_the_validation_these_tests_rely_on_refuses_a_missing_member() -> None:
    # The proof is only as strict as the validation: a search call without its id is refused.
    item = fake.web_search_call(script.ServerCall("ws_1", "web_search", "q", ["u"]))
    del item["id"]
    with pytest.raises(ValidationError):
        ResponseFunctionWebSearch.model_validate(item)


def test_a_streamed_web_search_reports_each_stage_and_each_citation() -> None:
    events = stream(input=content.SERVER_TOOLS, tools=[{"type": "web_search"}])
    kinds = [event["type"] for event in events]
    assert (
        kinds.index("response.web_search_call.in_progress")
        < kinds.index("response.web_search_call.searching")
        < kinds.index("response.web_search_call.completed")
        < kinds.index("response.output_text.annotation.added")
        < kinds.index("response.output_text.done")
    )
    final = events[-1]
    assert final["type"] == "response.completed"
    assert isinstance(items(final["response"]["output"])[0], ResponseFunctionWebSearch)
    assert [event["sequence_number"] for event in events] == list(range(len(events)))


def test_dated_and_preview_spellings_enable_the_same_search() -> None:
    for spelling in ("web_search_2025_08_26", "web_search_preview"):
        answer = respond(input=content.SERVER_TOOLS, tools=[{"type": spelling}])
        assert isinstance(answer[0], ResponseFunctionWebSearch)


def test_without_its_search_tool_the_question_is_answered_plainly() -> None:
    (message,) = respond(input=content.SERVER_TOOLS)
    assert isinstance(message, ResponseOutputMessage)
    assert message.content[0].annotations == []


def test_no_other_answer_changes_shape() -> None:
    # Only the search question cites: every other shared answer keeps an empty annotation list and
    # streams no annotation event, so the suites captured against this endpoint replay unchanged.
    (message,) = respond(input=content.CHAT)
    assert message.content[0].annotations == []
    assert message.content[0].text == script.ANSWERS[content.CHAT]
    kinds = [event["type"] for event in stream(input=content.CHAT)]
    assert "response.output_text.annotation.added" not in kinds


def upload(file: Any) -> FileObject:
    """An upload through the SDK, its answer validated as the SDK's ``FileObject``."""
    sent = client().files.with_raw_response.create(file=file, purpose="user_data")
    return FileObject.model_validate(json.loads(sent.http_response.text))


def test_an_upload_is_a_file_object_with_a_stable_id() -> None:
    pdf = Run.asset("task.pdf").read_bytes()
    first = upload(("task.pdf", pdf, "application/pdf"))
    again = upload(("task.pdf", pdf, "application/pdf"))
    assert first.id == again.id and first.id.startswith("file-")
    assert (first.bytes, first.filename, first.purpose) == (
        len(pdf),
        "task.pdf",
        "user_data",
    )


def test_an_upload_streamed_in_chunks_or_without_a_name_is_read_whole() -> None:
    # The client sends a stream it cannot seek in chunks, without a length; a tuple may name no file.
    pdf = Run.asset("task.pdf").read_bytes()
    named = upload(("task.pdf", pdf, "application/pdf"))

    class Unseekable(io.RawIOBase):
        def __init__(self, data: bytes) -> None:
            self.data = io.BytesIO(data)

        def readable(self) -> bool:
            return True

        def readinto(self, buffer: Any) -> int:
            chunk = self.data.read(len(buffer))
            buffer[: len(chunk)] = chunk
            return len(chunk)

    streamed = upload(("task.pdf", Unseekable(pdf), "application/pdf"))
    assert (streamed.id, streamed.bytes) == (named.id, len(pdf))
    unnamed = upload((None, b"\xff\xfe binary", "application/pdf"))
    assert unnamed.bytes == len(b"\xff\xfe binary")


def test_an_input_file_named_by_its_id_is_accepted() -> None:
    uploaded = upload(("task.pdf", b"%PDF-1.3 minimal", "application/pdf"))
    answer = respond(
        input=[
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": content.FILES},
                    {"type": "input_file", "file_id": uploaded.id},
                ],
            }
        ]
    )
    assert answer[0].content[0].text == script.ANSWERS[content.FILES]


def test_a_compaction_item_in_the_input_is_carried_and_ignored() -> None:
    # A compacted conversation is opaque state the provider reads back; the next turn is answered
    # from the turns that follow it.
    answer = respond(
        input=[
            {"type": "compaction", "id": "cmp_1", "encrypted_content": "opaque"},
            {"role": "user", "content": content.CHAT},
        ]
    )
    assert answer[0].content[0].text == script.ANSWERS[content.CHAT]
