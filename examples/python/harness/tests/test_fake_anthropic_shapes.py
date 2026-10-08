"""What the fake Anthropic endpoint answers is what Anthropic sends: the SDK's own types validate it.

A scripted response is hand-written, so each shape the fake serves for a provider feature is proven
against the Anthropic SDK, whose types are generated from the API's specification. The client builds
its objects without validating them, so these tests take the fake's raw JSON and validate it against
the SDK's types: a required member missing, or a value of the wrong type, fails here.
"""

from __future__ import annotations

import json
import urllib.request
from typing import Any

import anthropic
import pytest
from anthropic.types import (
    CitationsWebSearchResultLocation,
    Message,
    RawMessageStreamEvent,
    ServerToolUseBlock,
    TextBlock,
    WebSearchToolResultBlock,
)
from pydantic import TypeAdapter, ValidationError

from harness import clients, content
from harness.fakes import anthropic as fake
from harness.fakes import script

EVENTS: TypeAdapter[Any] = TypeAdapter(RawMessageStreamEvent)
SEARCH = [{"type": "web_search_20250305", "name": "web_search"}]


def base() -> str:
    return clients.fake_url("fake-anthropic")


def post(body: dict[str, Any]) -> str:
    request = urllib.request.Request(
        f"{base()}/v1/messages",
        data=json.dumps({"model": "fake", "max_tokens": 1024, **body}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return str(response.read().decode())


def ask(question: str, **body: Any) -> Message:
    """The fake's whole answer, validated as the SDK's ``Message``."""
    messages = [{"role": "user", "content": question}]
    return Message.model_validate(json.loads(post({"messages": messages, **body})))


def stream(question: str, **body: Any) -> list[dict[str, Any]]:
    """The fake's streamed answer, every event validated as the SDK's stream event types."""
    messages = [{"role": "user", "content": question}]
    raw = post({"messages": messages, "stream": True, **body})
    events = [
        json.loads(line.removeprefix("data: "))
        for line in raw.splitlines()
        if line.startswith("data: ")
    ]
    for event in events:
        EVENTS.validate_python(event)
    return events


def test_a_web_search_is_a_provider_call_and_its_results_before_a_cited_answer() -> (
    None
):
    answer = ask(content.SERVER_TOOLS, tools=SEARCH)
    searched, found, text = answer.content
    query, sources = script.SEARCHES[content.SERVER_TOOLS]
    assert isinstance(searched, ServerToolUseBlock)
    assert (searched.name, searched.input) == ("web_search", {"query": query})
    assert isinstance(found, WebSearchToolResultBlock)
    assert found.tool_use_id == searched.id
    assert isinstance(found.content, list)
    assert [result.url for result in found.content] == sources
    assert isinstance(text, TextBlock)
    assert text.text == script.ANSWERS[content.SERVER_TOOLS]
    (citation,) = text.citations or []
    assert isinstance(citation, CitationsWebSearchResultLocation)
    assert citation.url == sources[0]
    assert len(citation.cited_text) <= 150
    assert answer.stop_reason == "end_turn"
    assert answer.usage.server_tool_use is not None
    assert answer.usage.server_tool_use.web_search_requests == 1


def test_the_validation_these_tests_rely_on_refuses_a_missing_member() -> None:
    # The proof is only as strict as the validation: a search result without its call's id is refused.
    search = script.ServerCall("ws_1", "web_search", "q", ["https://example.com"])
    _, found = fake.search_blocks(search)
    del found["tool_use_id"]
    with pytest.raises(ValidationError):
        WebSearchToolResultBlock.model_validate(found)


def test_a_streamed_search_opens_its_call_then_its_results_then_cites_the_answer() -> (
    None
):
    events = stream(content.SERVER_TOOLS, tools=SEARCH)

    def kind(event: dict[str, Any]) -> str:
        """What an event carries: the block it opens, the delta it adds, or the event itself."""
        if event["type"] == "content_block_start":
            return str(event["content_block"]["type"])
        if event["type"] == "content_block_delta":
            return str(event["delta"]["type"])
        return str(event["type"])

    seen = [kind(event) for event in events]
    assert (
        seen.index("server_tool_use")
        < seen.index("input_json_delta")
        < seen.index("web_search_tool_result")
        < seen.index("text_delta")
        < seen.index("citations_delta")
    )
    (final,) = [event for event in events if event["type"] == "message_delta"]
    assert final["usage"]["server_tool_use"]["web_search_requests"] == 1


def test_the_sdk_assembles_the_stream_into_the_whole_answer() -> None:
    # The client's own accumulator rebuilds every block, citations included, from the deltas.
    client = anthropic.Anthropic(base_url=base(), api_key="fake")
    messages = [{"role": "user", "content": content.SERVER_TOOLS}]
    with client.messages.stream(
        model="fake", max_tokens=1024, messages=messages, tools=SEARCH
    ) as streamed:
        assembled = streamed.get_final_message()
    whole = ask(content.SERVER_TOOLS, tools=SEARCH)
    assert [block.to_dict() for block in assembled.content] == [
        block.to_dict() for block in whole.content
    ]


@pytest.mark.parametrize(
    "spelling", ["web_search_20250305", "web_search_20260209", "web_search_20260318"]
)
def test_every_dated_release_enables_the_same_search(spelling: str) -> None:
    answer = ask(content.SERVER_TOOLS, tools=[{"type": spelling, "name": "web_search"}])
    assert isinstance(answer.content[0], ServerToolUseBlock)


def test_without_its_search_tool_the_question_is_answered_plainly() -> None:
    (text,) = ask(content.SERVER_TOOLS).content
    assert isinstance(text, TextBlock) and text.citations is None


def test_a_provider_tool_is_never_called_as_the_clients_own() -> None:
    # A question that needs no search is answered directly, with no call to the tool it declares.
    (text,) = ask(content.CHAT, tools=SEARCH).content
    assert text.text == script.ANSWERS[content.CHAT]


@pytest.mark.parametrize(
    "tool",
    [
        {"type": "bash_20250124", "name": "bash"},
        {"type": "text_editor_20250728", "name": "str_replace_based_edit_tool"},
        {"type": "memory_20250818", "name": "memory"},
    ],
)
def test_an_anthropic_defined_client_tool_is_still_the_clients_to_run(
    tool: dict[str, Any],
) -> None:
    # Typed, but executed by the application: the script calls it as it calls any tool it does not know.
    (call,) = ask(content.CHAT, tools=[tool]).content
    assert call.type == "tool_use" and call.name == tool["name"]


def test_a_toolset_or_an_advisor_is_never_called_as_the_clients_own() -> None:
    # A toolset names no tool to call, and the advisor is a tool the provider runs.
    for tool in (
        {"type": "mcp_toolset", "mcp_server_name": "travel"},
        {"type": "computer_toolset_20260801"},
        {"type": "advisor_20260301", "name": "advisor"},
    ):
        (text,) = ask(content.CHAT, tools=[tool]).content
        assert text.type == "text"


def test_no_other_answer_changes_shape() -> None:
    # Only a search cites and reports search usage: every other answer keeps the shape the suites
    # captured against this endpoint replay.
    raw = json.loads(post({"messages": [{"role": "user", "content": content.CHAT}]}))
    assert raw["content"] == [{"type": "text", "text": script.ANSWERS[content.CHAT]}]
    assert raw["usage"] == {"input_tokens": 40, "output_tokens": 12}
    (final,) = [
        event for event in stream(content.CHAT) if event["type"] == "message_delta"
    ]
    assert final["usage"] == {"output_tokens": 12}
