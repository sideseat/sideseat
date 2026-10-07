"""What each model call of one fixture was sent, with the lineage of every part (rubric v3, slice 1).

A truth document describes a scenario's conversation, which every fixture of the scenario shares; what
the framework *sent* differs per fixture - native, SDK and old releases serialise the same conversation
differently - so the request side is recorded per fixture, from that fixture's transcript
(:mod:`harness.transcript`), never from the reconstruction it is used to check.

Each part of a request is an *occurrence*, and names its lineage:

- ``new_fact``: the first time the conversation fact it carries reaches a model;
- ``replay_of``: a fact already established - an earlier call's output, or something an earlier request
  already sent;
- ``new`` / ``replay_of`` an ``rq-*`` id: content no conversation fact holds (a framework-composed turn,
  the system instruction), by its own first appearance;
- ``lineage_unknown``: the part could be more than one fact, or is an assistant turn no output matches.

Lineage is derived from the recorded requests and the calls' outputs alone. "The previous request plus its
output" is not assumed: truncation, summaries, retries and branches come out as what they are.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from harness import transcript
from harness.truth.requests import ModelRequest, decode_request
from harness.truth.wire import decode

REPO = Path(__file__).resolve().parents[5]
FIXTURES = REPO / "server" / "tests" / "fixtures" / "messages"


def model_interactions(cassette: Path) -> list[int]:
    """The cassette indices of its model interactions, in order: call ``n`` answered index ``[n-1]``."""
    interactions = json.loads(cassette.read_text())["interactions"]
    return [
        index for index, item in enumerate(interactions) if decode(item) is not None
    ]


def _canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _candidates(
    part: dict[str, Any], role: str, facts: list[dict[str, Any]]
) -> list[str]:
    kind = part["type"]
    out = []
    for fact in facts:
        value = fact["value"]
        if kind == "text":
            wanted = {"user": "user_text", "system": "system"}.get(role, "text")
            if fact["kind"] == wanted and value.get("text") == part["text"]:
                out.append(fact["id"])
        elif kind == "tool_call" and fact["kind"] == "tool_call":
            if part["id"] and value.get("id"):
                if part["id"] == value["id"]:
                    out.append(fact["id"])
            elif value.get("name") == part["name"] and _canonical(
                value.get("arguments")
            ) == _canonical(part["arguments"]):
                out.append(fact["id"])
        elif kind == "tool_result" and fact["kind"] == "tool_result":
            if part["id"] and value.get("call_id") == part["id"]:
                out.append(fact["id"])
        elif kind == "reasoning" and fact["kind"] == "reasoning":
            if part.get("text") and value.get("text") == part["text"]:
                out.append(fact["id"])
        elif kind == "media" and fact["kind"] == "user_media":
            if value.get("sha256") == part["sha256"]:
                out.append(fact["id"])
    return out


def _collapsed(text: str) -> str:
    return " ".join(text.split())


#: How much of a fact must appear inside a part before the part reads as a rendering of it. Long enough that
#: two different turns cannot share it by accident; no share-of-the-part rule, because a framework's
#: scaffolding around a quoted task - CrewAI's "Current Task ... expected criteria" - is mostly scaffolding
#: and still a rendering.
_RENDERS_MIN = 40


def _renderings(fact: dict[str, Any]) -> list[str]:
    """The forms a fact's content can appear in inside another message's text.

    A framework quotes a turn verbatim, and a tool result either as JSON or as the language's own literal -
    smolagents writes ``Observation: {'city': 'Tokyo'}``, which is `repr`, not JSON. Both spellings are
    offered, so a rendering is recognised whichever the framework used.
    """
    value = fact["value"]
    kind = fact["kind"]
    if kind in ("system", "user_text", "text", "reasoning"):
        text = value.get("text") or ""
        return [text] if text else []
    if kind == "tool_result":
        held = value.get("value")
        return [json.dumps(held, ensure_ascii=False), repr(held), str(held)]
    if kind == "tool_call":
        arguments = value.get("arguments")
        return [json.dumps(arguments, ensure_ascii=False), repr(arguments)]
    return []


def _renders(part: dict[str, Any], facts: list[dict[str, Any]]) -> list[str]:
    """The conversation facts a request part re-renders: their content inside its own text.

    A framework that hands the model its own state message, quotes the task back, or writes a tool result as
    an observation is rendering facts the conversation already has. Such a part is not new content, and
    minting a fact for it would make two facts demand one block.
    """
    text = _collapsed(part.get("text") or "") if part["type"] == "text" else ""
    if not text:
        return []
    out = []
    for fact in facts:
        for rendering in _renderings(fact):
            quoted = _collapsed(rendering)
            if len(quoted) < _RENDERS_MIN or quoted == text or quoted not in text:
                continue
            out.append(fact["id"])
            break
    return out


class _Lineage:
    """Assigns each occurrence its lineage as the requests arrive.

    Content a conversation fact holds is a reference to that fact. Content that renders facts the
    conversation has - a state message quoting the task, a tool result written as an observation - is a
    rendering, linked to the facts it renders and no fact of its own, so two facts never demand one block.
    Anything else only a request carries keeps an ``rq-*`` identity of its own.
    """

    def __init__(self, truth: dict[str, Any], fixture: str) -> None:
        self.truth = truth
        self.fixture = fixture
        self.established: set[str] = set()
        self.request_only: dict[str, str] = {}

    def assign(
        self, part: dict[str, Any], role: str, conversation: str
    ) -> dict[str, Any]:
        facts = [f for f in self.truth["facts"] if f["conversation"] == conversation]
        candidates = _candidates(part, role, facts)
        if len(candidates) > 1:
            return {"lineage_unknown": f"could be any of {', '.join(candidates)}"}
        if candidates:
            (fact,) = candidates
            lineage = "replay_of" if fact in self.established else "new_fact"
            self.established.add(fact)
            return {lineage: fact}
        if role == "assistant" or part["type"] in ("tool_call", "reasoning"):
            # What the model said, which no output of this conversation is: a rewrite or a summary.
            return {"lineage_unknown": "model-side content no call's output matches"}
        rendered = _renders(part, facts)
        key = _canonical([role, part])
        if key in self.request_only:
            lineage = {"replay_of": self.request_only[key]}
            return {**lineage, "renders": rendered} if rendered else lineage
        if rendered:
            # A rendering of facts the conversation already has: no fact of its own, so the two never
            # compete for one block. The assignment still requires the span sent it to show it.
            identifier = f"rq-{len(self.request_only) + 1:03d}"
            self.request_only[key] = identifier
            return {"new": identifier, "renders": rendered}
        identifier = f"rq-{len(self.request_only) + 1:03d}"
        self.request_only[key] = identifier
        return {"new": identifier}

    def outputs(self, call: dict[str, Any]) -> None:
        self.established.update(call["outputs"])


def _occurrences(
    request: ModelRequest, lineage: _Lineage, conversation: str
) -> dict[str, Any]:
    def occurrence(part: dict[str, Any], role: str) -> dict[str, Any]:
        return {"part": part, **lineage.assign(part, role, conversation)}

    return {
        "api": request.api,
        "system": [occurrence(part, "system") for part in request.system],
        "messages": [
            {
                "role": message["role"],
                "parts": [
                    occurrence(part, message["role"]) for part in message["parts"]
                ],
            }
            for message in request.messages
        ],
        "tools": request.tools,
    }


def fixture_requests(
    truth: dict[str, Any], fixture: str, interaction_calls: dict[int, str] | None
) -> dict[str, Any] | None:
    """The request truth of one fixture, or ``None`` when the fixture has no transcript.

    ``interaction_calls`` maps a cassette interaction index to the call it answered; without a cassette
    (a fake model), model requests pair with the calls in arrival order.
    """
    path = FIXTURES / fixture / transcript.FILENAME
    if not path.exists():
        return None
    calls = {call["id"]: call for call in truth["calls"]}
    in_order = [call["id"] for call in truth["calls"]]
    lineage = _Lineage(truth, fixture)
    out: dict[str, Any] = {}
    unanswered = []
    arrival = 0
    for entry in transcript.load(path):
        request = decode_request(entry["method"], entry["path"], entry["body"])
        if request is None:
            continue
        if interaction_calls is None:
            call_id = in_order[arrival] if arrival < len(in_order) else None
        else:
            call_id = interaction_calls.get(entry.get("answered_by", -1))
        arrival += 1
        if call_id is None or call_id in out:
            unanswered.append(entry["request_sha256"])
            continue
        call = calls[call_id]
        out[call_id] = _occurrences(request, lineage, call["conversation"])
        lineage.outputs(call)
    return {
        "transcript": path.relative_to(REPO).as_posix(),
        "calls": out,
        "unpaired_requests": unanswered,
    }
