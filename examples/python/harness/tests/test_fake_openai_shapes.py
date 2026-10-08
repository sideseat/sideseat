"""What the fake OpenAI endpoint answers is what OpenAI sends: the SDK's own types read it.

A scripted response is hand-written, so each shape the fake serves for a provider feature is proven
against the OpenAI SDK, whose types are generated from the API's specification: the client
deserialises the fake's answer into the typed objects, field by field, rather than into the
untyped fallback it uses for a shape it does not know.
"""

from __future__ import annotations

from typing import Any

import openai
from openai.types import FileObject
from openai.types.responses import (
    Response,
    ResponseFunctionWebSearch,
    ResponseOutputMessage,
    ResponseOutputText,
)
from openai.types.responses.response_function_web_search import (
    ActionSearch,
    ActionSearchSource,
)
from openai.types.responses.response_output_text import (
    AnnotationURLCitation,
)

from harness import Run, clients, content
from harness.fakes import script


def client() -> Any:
    return openai.OpenAI(base_url=clients.fake_url("fake-openai"), api_key="fake")


def web_search(**extra: Any) -> Any:
    return client().responses.create(
        model="fake",
        input=content.SERVER_TOOLS,
        tools=[{"type": "web_search"}],
        **extra,
    )


def test_a_web_search_is_a_typed_search_call_before_a_cited_answer() -> None:
    answer = web_search()
    assert isinstance(answer, Response)
    search, message = answer.output
    assert isinstance(search, ResponseFunctionWebSearch)
    assert search.status == "completed"
    assert isinstance(search.action, ActionSearch)
    query, sources = script.SEARCHES[content.SERVER_TOOLS]
    assert search.action.query == query
    assert search.action.queries == [query]
    assert search.action.sources == [
        ActionSearchSource(type="url", url=url) for url in sources
    ]
    assert isinstance(message, ResponseOutputMessage)
    (text,) = message.content
    assert isinstance(text, ResponseOutputText)
    assert text.text == script.ANSWERS[content.SERVER_TOOLS]
    (citation,) = text.annotations
    assert isinstance(citation, AnnotationURLCitation)
    assert (citation.start_index, citation.end_index) == (0, len(text.text))
    assert citation.url == sources[0]


def test_a_streamed_web_search_reports_each_stage_in_typed_events() -> None:
    events = list(web_search(stream=True))
    kinds = [event.type for event in events]
    assert (
        kinds.index("response.web_search_call.in_progress")
        < kinds.index("response.web_search_call.searching")
        < kinds.index("response.web_search_call.completed")
    )
    final = events[-1]
    assert final.type == "response.completed"
    assert isinstance(final.response.output[0], ResponseFunctionWebSearch)


def test_dated_and_preview_spellings_enable_the_same_search() -> None:
    for spelling in ("web_search_2025_08_26", "web_search_preview"):
        answer = client().responses.create(
            model="fake", input=content.SERVER_TOOLS, tools=[{"type": spelling}]
        )
        assert isinstance(answer.output[0], ResponseFunctionWebSearch)


def test_without_its_search_tool_the_question_is_answered_plainly() -> None:
    answer = client().responses.create(model="fake", input=content.SERVER_TOOLS)
    (message,) = answer.output
    assert isinstance(message, ResponseOutputMessage)
    assert message.content[0].annotations == []


def test_no_other_answer_changes_shape() -> None:
    # Only the search question cites: every other shared answer keeps an empty annotation list,
    # so the suites already captured against this endpoint replay unchanged.
    answer = client().responses.create(model="fake", input=content.CHAT)
    (message,) = answer.output
    assert message.content[0].annotations == []
    assert message.content[0].text == script.ANSWERS[content.CHAT]


def test_an_upload_is_a_typed_file_object_with_a_stable_id() -> None:
    pdf = Run.asset("task.pdf").read_bytes()
    first = client().files.create(
        file=("task.pdf", pdf, "application/pdf"), purpose="user_data"
    )
    again = client().files.create(
        file=("task.pdf", pdf, "application/pdf"), purpose="user_data"
    )
    assert isinstance(first, FileObject)
    assert first.id == again.id and first.id.startswith("file-")
    assert first.bytes == len(pdf)
    assert first.filename == "task.pdf"
    assert first.purpose == "user_data"


def test_an_input_file_named_by_its_id_is_accepted() -> None:
    upload = client().files.create(
        file=("task.pdf", b"%PDF-1.3 minimal", "application/pdf"), purpose="user_data"
    )
    answer = client().responses.create(
        model="fake",
        input=[
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": content.FILES},
                    {"type": "input_file", "file_id": upload.id},
                ],
            }
        ],
    )
    assert answer.output[0].content[0].text == script.ANSWERS[content.FILES]


def test_a_compaction_item_in_the_input_is_carried_and_ignored() -> None:
    # A compacted conversation is opaque state the provider reads back; the next turn is answered
    # from the turns that follow it.
    answer = client().responses.create(
        model="fake",
        input=[
            {"type": "compaction", "id": "cmp_1", "encrypted_content": "opaque"},
            {"role": "user", "content": content.CHAT},
        ],
    )
    assert answer.output[0].content[0].text == script.ANSWERS[content.CHAT]
