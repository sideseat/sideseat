"""The deterministic model behind every fake server.

Each fake translates its provider's wire format into a :class:`Request`, asks :func:`reply` what the
model does next, and translates the :class:`Reply` back. The script knows the shared prompts and
tools of :mod:`harness.content`, so a scenario run against a fake holds the same conversation shape
as a live one: the tools the question needs are called with arguments taken from the question, the
answer is built from their results, and a schema-constrained request gets JSON matching its schema.

Tools outside the shared set are called once, with arguments derived from their schema, so a
framework that adds tools of its own still completes a round trip.
"""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass, field
from typing import Any

from harness import content

CITIES = (
    "Paris",
    "Tokyo",
    "Rome",
    "Barcelona",
    "Lisbon",
    "Madrid",
    "Vienna",
    "Kyoto",
    "London",
    "Oslo",
)

#: Answers to the shared questions that need no tool.
ANSWERS = {
    content.CHAT: "Kyoto is best known for its historic Buddhist temples, Shinto shrines, and "
    "traditional gardens.",
    content.MULTI_TURN[0]: "Stay in Alfama, Lisbon's oldest neighbourhood.",
    content.MULTI_TURN[1]: "Try pastel de nata, the custard tart Lisbon is famous for.",
    content.MULTI_TURN[2]: "Stay in Alfama and try a pastel de nata.",
    content.SESSION[0]: "Visit the Museo del Prado.",
    content.SESSION[1]: "Walk through the Retiro Park.",
    content.REASONING: "The fastest crossing takes 17 minutes: 1 and 2 cross (2), 1 returns (1), "
    "5 and 10 cross (10), 2 returns (2), and 1 and 2 cross again (2).",
    content.FILES: "The image is a photograph; the document is a one-page task description.",
    content.MCP: "The result is 395.",
    content.SERVER_TOOLS: "The Louvre opens at 9 am and closes at 6 pm every day except Tuesday, "
    "when it is closed.",
}

THOUGHTS = {
    content.REASONING: "The two slowest should cross together so the 10 absorbs the 5. Shuttling "
    "the torch with the two fastest costs 1 + 2 + 2, so the total is 2 + 1 + 10 + 2 + 2.",
}

FALLBACK = "This is a deterministic local response."


@dataclass
class Call:
    id: str
    name: str
    arguments: dict[str, Any]


@dataclass
class Result:
    call_id: str
    name: str
    text: str


@dataclass
class ServerCall:
    """A tool the provider runs itself (its web search), and what the run found."""

    id: str
    #: The provider's own name for the tool, as the request declares it.
    tool: str
    query: str
    sources: list[str]


@dataclass
class Turn:
    """One message: a user's text, an assistant's text and calls, or tool results."""

    role: str
    text: str = ""
    calls: list[Call] = field(default_factory=list)
    results: list[Result] = field(default_factory=list)


@dataclass
class Request:
    turns: list[Turn]
    #: Declared tools by name, with their JSON-schema parameters.
    tools: dict[str, dict[str, Any]] = field(default_factory=dict)
    #: The JSON schema the answer must match, when the request constrains it.
    schema: dict[str, Any] | None = None
    thinking: bool = False
    #: The provider-run tools the request enables, by the provider's own type name (``web_search``).
    hosted_tools: set[str] = field(default_factory=set)


@dataclass
class Reply:
    text: str = ""
    calls: list[Call] = field(default_factory=list)
    thought: str = ""
    #: Tools the provider ran while answering, held in the same reply as the answer.
    server_calls: list[ServerCall] = field(default_factory=list)


#: What the provider's web search returns for the one question that asks for it.
SEARCHES = {
    content.SERVER_TOOLS: (
        "Louvre opening hours",
        ["https://www.louvre.fr/en/visit/hours-admission"],
    ),
}


def reply(request: Request) -> Reply:
    question, since = _current_question(request.turns)
    rounds_done = sum(1 for turn in since if turn.calls)
    results = [result for turn in since for result in turn.results]
    plan = _plan(question, request.tools)
    thought = THOUGHTS.get(question, "") if request.thinking else ""
    if rounds_done < len(plan):
        calls = [
            Call(_call_id(name, question, rounds_done, index), name, arguments)
            for index, (name, arguments) in enumerate(plan[rounds_done])
        ]
        return Reply(calls=calls, thought=thought)
    if question in SEARCHES and "web_search" in request.hosted_tools:
        query, sources = SEARCHES[question]
        search = ServerCall(f"ws_{digest(question)}", "web_search", query, sources)
        return Reply(text=ANSWERS[question], server_calls=[search], thought=thought)
    if request.schema is not None:
        text = json.dumps(structured(request.schema), separators=(",", ":"))
    elif results:
        text = _answer_from(question, results)
    else:
        text = ANSWERS.get(question, FALLBACK)
    if "final_answer" in request.tools:
        # Agents that answer through a terminal tool (smolagents) end only when it is called.
        call_id = _call_id("final_answer", question, rounds_done, 0)
        return Reply(
            calls=[Call(call_id, "final_answer", {"answer": text})], thought=thought
        )
    return Reply(text=text, thought=thought)


def _current_question(turns: list[Turn]) -> tuple[str, list[Turn]]:
    for index in range(len(turns) - 1, -1, -1):
        turn = turns[index]
        if turn.role == "user" and turn.text and not turn.results:
            return _canonical(turn.text), turns[index + 1 :]
    return "", turns


def _canonical(text: str) -> str:
    """The shared prompt a user turn carries, when it carries one among other parts."""
    for prompt in (
        *ANSWERS,
        content.TOOL_USE,
        content.ERROR,
        content.STREAMING,
        content.STRUCTURED,
        content.MULTI_AGENT,
    ):
        if prompt in text:
            return prompt
    return text.strip()


def _plan(
    question: str, tools: dict[str, dict[str, Any]]
) -> list[list[tuple[str, dict[str, Any]]]]:
    """The rounds of tool calls the question needs, given the tools the request declares."""
    rounds: list[list[tuple[str, dict[str, Any]]]] = []
    lowered = question.lower()
    booking = re.search(r"from (\w+) to (\w+) on ([\d-]+)", question)
    if "book_flight" in tools and booking:
        origin, destination, date = booking.groups()
        rounds.append(
            [
                (
                    "book_flight",
                    {"origin": origin, "destination": destination, "date": date},
                )
            ]
        )
    cities = [city for city in CITIES if city in question]
    if "get_weather" in tools and cities and not booking:
        days = 2 if "two days" in lowered else 1
        rounds.append(
            [("get_weather", {"city": city, "days": days}) for city in cities]
        )
    if (
        "get_precipitation" in tools
        and cities
        and ("umbrella" in lowered or "rain" in lowered)
    ):
        rounds.append([("get_precipitation", {"city": city}) for city in cities])
    arithmetic = re.search(r"compute (.+?), then", question)
    if "calculate" in tools and arithmetic:
        rounds.append([("calculate", {"expression": arithmetic.group(1)})])
    if not rounds:
        unknown = [
            name for name in tools if name not in _SHARED and name != "final_answer"
        ]
        if unknown:
            name = unknown[0]
            rounds.append([(name, _arguments(tools[name], question))])
    return rounds


_SHARED = {"get_weather", "get_precipitation", "book_flight"}


def _arguments(schema: dict[str, Any], question: str) -> dict[str, Any]:
    properties = schema.get("properties") or {}
    names = schema.get("required") or list(properties)[:1]
    numbers = [int(n) for n in re.findall(r"\d+", question)]
    arguments = {}
    for name in names:
        kind = str((properties.get(name) or {}).get("type") or "").lower()
        if kind in ("integer", "number"):
            arguments[name] = numbers.pop(0) if numbers else 1
        elif name in ("expression", "query", "input", "text"):
            arguments[name] = question
        else:
            arguments[name] = structured(properties.get(name) or {}, name)
    return arguments


def _answer_from(question: str, results: list[Result]) -> str:
    sentences = []
    for result in results:
        value = _value(result.text)
        if (
            result.name == "get_weather"
            and isinstance(value, dict)
            and "forecast" in value
        ):
            days = ", ".join(
                f"day {day['day']} {day['condition']} at {day['high_c']}°C"
                for day in value["forecast"]
            )
            sentences.append(f"{value['city']}: {days}")
        elif result.name == "book_flight":
            # Some clients wrap the exception in a sentence of their own; keep its message.
            message = str(value).rpartition("because of error ")[2]
            sentences.append(f"I could not book that flight: {message.rstrip('.')}")
        elif result.name == "calculate":
            sentences.append(f"The result is {str(value).rstrip('.')}")
        else:
            sentences.append(str(value).rstrip("."))
    answer = "; ".join(sentence for sentence in sentences if sentence) + "."
    if "umbrella" in question.lower():
        wet = [
            c for c in CITIES if c in question and any(_rainy(r, c) for r in results)
        ]
        advice = (
            f" Pack an umbrella for {' and '.join(wet)}."
            if wet
            else " No umbrella needed."
        )
        answer += advice
    elif "wear" in question.lower():
        answer += " Wear light layers and sunglasses."
    elif results[-1].name == "book_flight":
        answer += " Please try again later."
    return answer


def _value(text: str) -> Any:
    """A tool result's value, unwrapped from the ``{"result"|"error": ...}`` some clients send."""
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        return text
    while (
        isinstance(value, dict)
        and len(value) == 1
        and ("error" in value or "result" in value)
    ):
        value = next(iter(value.values()))
    return value


def _rainy(result: Result, city: str) -> bool:
    return city in result.text and (
        "rain" in result.text and "10% chance" not in result.text
    )


def digest(value: Any) -> str:
    """A short id derived from a request, so identical requests get identical response ids."""
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()[:12]


def _call_id(name: str, question: str, round_index: int, index: int) -> str:
    digest = hashlib.sha256(f"{question}|{round_index}|{index}".encode()).hexdigest()[
        :12
    ]
    return f"call_{name}_{digest}"


#: Values for schema properties the shared scenarios name.
KNOWN = {
    "city": "Vienna",
    "days": [
        "Schönbrunn Palace and the Naschmarkt",
        "St. Stephen's Cathedral and the Prater",
    ],
    "budget_eur": 600,
    "location": "Paris",
}


def structured(
    schema: dict[str, Any], name: str = "value", root: dict[str, Any] | None = None
) -> Any:
    """A value matching ``schema``, using :data:`KNOWN` values for the shared property names."""
    root = schema if root is None else root
    reference = schema.get("$ref")
    if isinstance(reference, str) and reference.startswith("#/"):
        target: Any = root
        for part in reference[2:].split("/"):
            target = target.get(part, {}) if isinstance(target, dict) else {}
        schema = target if isinstance(target, dict) else {}
    if name in KNOWN:
        return KNOWN[name]
    if "const" in schema:
        return schema["const"]
    if schema.get("enum"):
        return schema["enum"][0]
    for key in ("anyOf", "oneOf"):
        variants = [v for v in schema.get(key) or [] if v.get("type") != "null"]
        if variants:
            return structured(variants[0], name, root)
    kind = schema.get("type")
    if isinstance(kind, list):
        kind = next((k for k in kind if k != "null"), "null")
    kind = kind.lower() if isinstance(kind, str) else kind
    if kind == "object" or "properties" in schema:
        return {
            key: structured(value, key, root)
            for key, value in (schema.get("properties") or {}).items()
        }
    if kind == "array":
        return [structured(schema.get("items") or {}, name, root)]
    if kind == "integer":
        return 1
    if kind == "number":
        return 1.0
    if kind == "boolean":
        return True
    return f"deterministic {name.replace('_', ' ')}"
