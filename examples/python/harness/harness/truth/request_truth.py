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


class _Lineage:
    """Assigns each occurrence its lineage as the requests arrive."""

    def __init__(self, truth: dict[str, Any]) -> None:
        self.truth = truth
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
        key = _canonical([role, part])
        if key in self.request_only:
            return {"replay_of": self.request_only[key]}
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
    lineage = _Lineage(truth)
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
