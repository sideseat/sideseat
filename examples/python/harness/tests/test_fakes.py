"""The fake model servers hold the shared conversations, so offline captures are meaningful."""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

import pytest

from harness import capture, clients, content
from harness.fakes import script
from harness.tooling import execute, spec

WEATHER_TOOLS = {
    "get_weather": spec(content.get_weather).parameters,
    "get_precipitation": spec(content.get_precipitation).parameters,
}
FUNCTIONS = {
    "get_weather": content.get_weather,
    "get_precipitation": content.get_precipitation,
}


def post(url: str, body: dict[str, Any]) -> Any:
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return response.read().decode()


def test_spec_describes_the_shared_tool_from_its_signature_and_docstring() -> None:
    tool = spec(content.get_weather)
    assert tool.name == "get_weather"
    assert tool.description == "Get the weather forecast for a city."
    assert tool.parameters == {
        "type": "object",
        "properties": {
            "city": {"type": "string", "description": "The city name."},
            "days": {
                "type": "integer",
                "description": "How many days to forecast, from 1 to 7.",
            },
        },
        "required": ["city"],
    }


def test_a_raising_tool_becomes_an_error_outcome() -> None:
    outcome = execute(
        {"book_flight": content.book_flight},
        "book_flight",
        '{"origin": "London", "destination": "Oslo", "date": "2026-11-14"}',
    )
    assert outcome.is_error
    assert outcome.text.startswith("BookingUnavailable: No seats from London to Oslo")


def test_tool_use_calls_both_tools_for_both_cities_then_answers_from_the_results() -> (
    None
):
    turns = [script.Turn(role="user", text=content.TOOL_USE)]
    first = script.reply(script.Request(turns=list(turns), tools=WEATHER_TOOLS))
    assert [(c.name, c.arguments) for c in first.calls] == [
        ("get_weather", {"city": "Paris", "days": 2}),
        ("get_weather", {"city": "Tokyo", "days": 2}),
    ]

    def answered(reply: script.Reply) -> list[script.Turn]:
        results = [
            script.Result(c.id, c.name, execute(FUNCTIONS, c.name, c.arguments).text)
            for c in reply.calls
        ]
        return [
            script.Turn(role="assistant", calls=reply.calls),
            script.Turn("tool", results=results),
        ]

    turns += answered(first)
    second = script.reply(script.Request(turns=list(turns), tools=WEATHER_TOOLS))
    assert [c.name for c in second.calls] == ["get_precipitation", "get_precipitation"]
    turns += answered(second)
    final = script.reply(script.Request(turns=turns, tools=WEATHER_TOOLS))
    assert not final.calls
    assert "Pack an umbrella for Tokyo." in final.text
    assert "Tokyo: day 1 rain at 21°C" in final.text


def test_each_shared_question_without_tools_gets_its_own_answer() -> None:
    answers = {
        script.reply(script.Request(turns=[script.Turn("user", text=q)])).text
        for q in (content.CHAT, *content.MULTI_TURN, *content.SESSION)
    }
    assert len(answers) == 6
    assert script.FALLBACK not in answers


def test_a_schema_constrained_answer_matches_the_schema() -> None:
    schema = content.TripPlan.model_json_schema()
    reply = script.reply(
        script.Request(
            turns=[script.Turn("user", text=content.STRUCTURED)], schema=schema
        )
    )
    plan = content.TripPlan.model_validate_json(reply.text)
    assert plan.city == "Vienna"


def test_fake_gemini_calls_the_declared_tools_with_their_own_parameter_names() -> None:
    # The fake used to call `get_weather(location="Paris")` whatever the request declared.
    url = clients.fake_url("fake-gemini")
    body = {
        "contents": [{"role": "user", "parts": [{"text": content.STREAMING}]}],
        "tools": [
            {
                "functionDeclarations": [
                    {"name": "get_weather", "parameters": WEATHER_TOOLS["get_weather"]}
                ]
            }
        ],
    }
    answer = json.loads(
        post(f"{url}/v1beta/models/gemini-flash-latest:generateContent", body)
    )
    parts = answer["candidates"][0]["content"]["parts"]
    assert [p["functionCall"]["args"] for p in parts] == [{"city": "Rome", "days": 1}]


def test_fake_gemini_reads_members_in_the_spelling_the_client_sends() -> None:
    # google-genai sends `thinkingConfig: {include_thoughts: true}`; the fake read only camelCase and
    # answered the reasoning scenario without a thought.
    body = {
        "contents": [{"role": "user", "parts": [{"text": content.REASONING}]}],
        "generationConfig": {"thinkingConfig": {"include_thoughts": True}},
    }
    from harness.fakes import google_genai

    parts = google_genai.parts_of(script.reply(google_genai.request_of(body)))
    assert parts[0] == {"text": script.THOUGHTS[content.REASONING], "thought": True}
    assert parts[1] == {"text": script.ANSWERS[content.REASONING]}


def test_fake_gemini_streams_text_as_server_sent_events() -> None:
    url = clients.fake_url("fake-gemini")
    body = {"contents": [{"role": "user", "parts": [{"text": content.CHAT}]}]}
    stream = post(f"{url}/v1beta/models/m:streamGenerateContent?alt=sse", body)
    chunks = [
        json.loads(line[6:])
        for line in stream.splitlines()
        if line.startswith("data: ")
    ]
    text = "".join(c["candidates"][0]["content"]["parts"][0]["text"] for c in chunks)
    assert text == script.ANSWERS[content.CHAT]
    assert chunks[-1]["candidates"][0]["finishReason"] == "STOP"


@pytest.mark.parametrize("surface", ["fake-openai", "fake-anthropic", "fake-gemini"])
def test_a_fake_url_reaches_a_running_server_without_one_started_by_hand(
    surface: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    # clients pointed fake-gemini at port 5403 while the server listened on 5404, and nothing started
    # either; a fake now runs in-process unless FAKE_<SURFACE>_URL names one.
    monkeypatch.delenv(f"{surface.upper().replace('-', '_')}_URL", raising=False)
    base = clients.fake_url(surface)
    assert base.startswith("http://127.0.0.1:")
    with pytest.raises(urllib.error.HTTPError) as unsupported:
        post(f"{base}/unsupported", {})
    assert unsupported.value.code == 404


def test_fake_openai_serves_chat_completions_on_azure_deployment_routes() -> None:
    base = clients.fake_url("fake-openai").removesuffix("/v1")
    body = {"model": "m", "messages": [{"role": "user", "content": content.CHAT}]}
    reply = json.loads(post(f"{base}/openai/deployments/m/chat/completions", body))
    assert reply["choices"][0]["message"]["content"] == script.ANSWERS[content.CHAT]


def test_fake_openai_serves_responses_on_the_azure_client_route() -> None:
    # AzureOpenAI posts the Responses API to /openai/responses?api-version=..., which answered 404.
    base = clients.fake_url("fake-openai").removesuffix("/v1")
    body = {"model": "m", "input": content.CHAT}
    reply = json.loads(
        post(f"{base}/openai/responses?api-version=2025-04-01-preview", body)
    )
    assert reply["output"][0]["content"][0]["text"] == script.ANSWERS[content.CHAT]


def test_capture_needs_no_cassette_for_a_fake_model(tmp_path: Path) -> None:
    # A fake-model suite failed its SDK run with "no cassette to replay": the fake records nothing.
    (tmp_path / "pyproject.toml").write_text(
        '[tool.sideseat-example]\nproducer = "x"\ndefault-model = "fake-gemini"\n'
    )
    assert capture.uses_fake_model(tmp_path, None)
    assert not capture.uses_fake_model(tmp_path, "sonnet")


def test_results_wrapped_the_way_google_genai_sends_them_are_read() -> None:
    # The Gemini client sends `{"result": value}` and `{"error": message}`; the answer echoed the JSON.
    weather = json.dumps({"result": content.get_weather("Rome")})
    error = json.dumps(
        {"error": "Failed to invoke function book_flight because of error offline."}
    )
    turns = [
        script.Turn("user", text=content.STREAMING),
        script.Turn(
            "assistant", calls=[script.Call("1", "get_weather", {"city": "Rome"})]
        ),
        script.Turn("tool", results=[script.Result("1", "get_weather", weather)]),
    ]
    tools = {"get_weather": WEATHER_TOOLS["get_weather"]}
    answer = script.reply(script.Request(turns=turns, tools=tools)).text
    assert answer.startswith("Rome: day 1 sunny at 21°C.")
    turns = [
        script.Turn("user", text=content.ERROR),
        script.Turn("assistant", calls=[script.Call("1", "book_flight", {})]),
        script.Turn("tool", results=[script.Result("1", "book_flight", error)]),
    ]
    tools = {"book_flight": spec(content.book_flight).parameters}
    answer = script.reply(script.Request(turns=turns, tools=tools)).text
    assert answer == "I could not book that flight: offline. Please try again later."
