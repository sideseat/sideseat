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


#: What a request part is, as a fact: its kind and how a reconstruction must match it. A tool result is not
#: here: one that answers no call of the conversation names its call only through an id the truth does not
#: know, so it stays an occurrence the per-call assignment checks and the views are not obliged to show.
_FACT_OF_PART = {
    ("text", "system"): ("system", "exact"),
    ("text", "user"): ("user_text", "exact"),
    ("text", "assistant"): ("text", "exact"),
    ("media", "user"): ("user_media", "digest"),
    ("media", "system"): ("user_media", "digest"),
}


def _fact_value(part: dict[str, Any], kind: str) -> dict[str, Any] | None:
    """A request part in the shape the fact of that kind states, or ``None`` when it has no shape."""
    if kind in ("system", "user_text", "text"):
        return {"text": part["text"]} if part.get("text") else None
    if kind == "user_media":
        return {
            "modality": part["modality"],
            "media_type": part["media_type"],
            "sha256": part["sha256"],
        }
    return None


def _collapsed(text: str) -> str:
    return " ".join(text.split())


#: How long a fact must be before a part that quotes it whole reads as a rendering of it. The whole fact is
#: quoted, never a fragment, so the bar only keeps a word or a city name from passing for a turn; a short
#: prompt a framework wraps in its own template - Browser Use's ``<user_request>`` - is still that prompt.
#: No share-of-the-part rule, because a framework's scaffolding around a quoted task - CrewAI's "Current
#: Task ... expected criteria" - is mostly scaffolding and still a rendering.
_RENDERS_MIN = 16


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
    Anything else only a request carries - a client's preamble, the environment block it appends - becomes a
    fact scoped to the fixtures whose requests carry it, so the views are obliged to show what the model was
    told.
    """

    def __init__(self, truth: dict[str, Any], fixture: str) -> None:
        self.truth = truth
        self.fixture = fixture
        self.established: set[str] = set()
        #: Content only a request carries, by its canonical form: the ids of its copies, in the order one
        #: request sends them. A request that sends the same part twice sent two parts, and a view must show
        #: both, so each copy is a fact of its own; a later request re-sending them replays them in order.
        self.request_only: dict[str, list[str]] = {}
        #: How many copies of each content the request being read has sent so far.
        self.in_request: dict[str, int] = {}
        #: Each distinct system instruction, in order of first appearance: a request's thread. Two agents of
        #: one conversation - a lead and the subagent it starts - are two model contexts, told apart by what
        #: they were instructed, and a message both are sent was sent twice, once into each.
        self.threads: list[str] = []
        self.thread = 0
        #: Whether the part being read is a message part rather than the system instruction.
        self.in_messages = False
        #: Per content, the (thread, copy) slots this fixture has used, in order of first appearance: a
        #: slot's index is which of the content's facts it is, the same index in every fixture.
        self.slots: dict[str, list[tuple[int, int]]] = {}

    def next_request(self, request: ModelRequest) -> None:
        """A new request begins: its copies are counted from the first again, in the thread it belongs to."""
        self.in_request = {}
        instruction = _canonical([part.get("text") for part in request.system])
        if instruction not in self.threads:
            self.threads.append(instruction)
        self.thread = self.threads.index(instruction)

    def _mint(
        self, part: dict[str, Any], role: str, conversation: str, call: str, copy: int
    ) -> dict[str, Any] | None:
        """A fact for content only a request carries, scoped to this fixture; ``None`` if it has no shape."""
        entry = _FACT_OF_PART.get((part["type"], role))
        if entry is None:
            return None
        kind, match = entry
        value = _fact_value(part, kind)
        if value is None:
            return None
        same = [
            existing
            for existing in self.truth["facts"]
            if "fixtures" in existing
            and existing["kind"] == kind
            and existing["role"] == role
            and existing["conversation"] == conversation
            and existing["value"] == value
        ]
        if copy < len(same):
            # One fact per copy, however many of a scenario's fixtures sent it.
            self._widen(same[copy])
            return same[copy]
        identifier = f"fact-{len(self.truth['facts']) + 1:03d}"
        fact = {
            "id": identifier,
            "kind": kind,
            "role": role,
            "conversation": conversation,
            "evidence": "wire",
            "value": value,
            "require": {
                "anchor": "conversation",
                "views": ["trace", "session", "feed"],
                "cardinality": "exactly_once",
                "match": match,
            },
            "fixtures": [self.fixture],
        }
        self.truth["facts"].append(fact)
        if kind == "user_text":
            # A prompt names the call it prompted, as every user turn does: the first that was sent it.
            self.truth["edges"].append(
                {"kind": "prompt_of", "from": identifier, "to": call}
            )
        for record in self.truth["conversations"]:
            if record["id"] == conversation:
                # Introduced where it was first sent, which is before everything the call produced.
                record["sequence"].insert(self._introduce_at(record), identifier)
                break
        return fact

    def _widen(self, fact: dict[str, Any]) -> None:
        """This fixture's request carries that fact too."""
        if self.fixture not in fact["fixtures"]:
            fact["fixtures"] = sorted([*fact["fixtures"], self.fixture])

    def _introduce_at(self, record: dict[str, Any]) -> int:
        """Where a minted fact belongs: right after the last fact this conversation has established.

        Not before the first fact it has not established: a turn a framework only ever rendered - CrewAI
        handing the researcher the task in its own words - is never named, and would pull every later
        introduction ahead of what the conversation had already said.
        """
        established = [
            at for at, fact in enumerate(record["sequence"]) if fact in self.established
        ]
        return established[-1] + 1 if established else 0

    def assign(
        self, part: dict[str, Any], role: str, conversation: str, call: str
    ) -> dict[str, Any]:
        facts = [f for f in self.truth["facts"] if f["conversation"] == conversation]
        # A minted fact is request-only content, found below by its copy: a request sending it twice sent
        # two facts, which a lookup by content would fold into one.
        candidates = _candidates(part, role, [f for f in facts if "fixtures" not in f])
        # History a session hands the next turn: a fact of an earlier conversation, already established,
        # sent again. A replay, never new content - the views show it once, where it was first told.
        # Request-only content is not history: a preamble every turn is sent is each conversation's own,
        # shown with each, so it is minted per conversation below.
        others = [
            f
            for f in self.truth["facts"]
            if f["conversation"] != conversation
            and f["id"] in self.established
            and "fixtures" not in f
        ]
        history = _candidates(part, role, others)
        if history and any(c not in self.established for c in candidates):
            # The same content is this conversation's next fact and an earlier one's history: which the
            # part is cannot be told from the request alone, so neither is assumed.
            return {
                "lineage_unknown": f"history of {', '.join(history)} or new {', '.join(candidates)}"
            }
        if not candidates:
            candidates = history
        if len(candidates) > 1:
            return {"lineage_unknown": f"could be any of {', '.join(candidates)}"}
        if candidates:
            (fact,) = candidates
            for record in self.truth["facts"]:
                if record["id"] == fact and "fixtures" in record:
                    self._widen(record)
            lineage = "replay_of" if fact in self.established else "new_fact"
            self.established.add(fact)
            return {lineage: fact}
        if role == "assistant" or part["type"] in ("tool_call", "reasoning"):
            # What the model said, which no output of this conversation is: a rewrite or a summary.
            return {"lineage_unknown": "model-side content no call's output matches"}
        # Any conversation of the scenario: a framework that hands one agent the other's turn - a crew's
        # task quoting its predecessor's answer - renders a fact of another conversation, and that is still
        # a rendering, never new content.
        rendered = _renders(
            part,
            [
                f
                for f in self.truth["facts"]
                if "fixtures" not in f or self.fixture in f["fixtures"]
            ],
        )
        # Copies are counted by what the minted fact states, so two parts that would be one fact - a
        # document sent twice under two file names - are two copies of it, not two first copies.
        key = _canonical([role, conversation, self._identity(part, role)])
        within = self.in_request.get(key, 0)
        self.in_request[key] = within + 1
        # A message is its thread's: the same note sent into two agents' contexts is two facts. The system
        # instruction is not keyed by thread - it is what tells the threads apart.
        slot = (self.thread if self.in_messages else 0, within)
        slots = self.slots.setdefault(key, [])
        if slot not in slots:
            slots.append(slot)
        copy = slots.index(slot)
        copies = self.request_only.setdefault(key, [])
        if copy < len(copies):
            lineage = {"replay_of": copies[copy]}
            return {**lineage, "renders": rendered} if rendered else lineage
        if rendered:
            # A rendering of facts the conversation already has: no fact of its own, so the two never
            # compete for one block. The assignment still requires the span sent it to show it.
            identifier = f"rq-{self._request_only_count() + 1:03d}"
            copies.append(identifier)
            return {"new": identifier, "renders": rendered}
        minted = self._mint(part, role, conversation, call, copy)
        if minted is None:
            identifier = f"rq-{self._request_only_count() + 1:03d}"
            copies.append(identifier)
            return {"new": identifier}
        copies.append(minted["id"])
        self.established.add(minted["id"])
        return {"new_fact": minted["id"]}

    @staticmethod
    def _identity(part: dict[str, Any], role: str) -> Any:
        """What makes two request-only parts the same content: the fact they would mint, else the part."""
        entry = _FACT_OF_PART.get((part["type"], role))
        value = _fact_value(part, entry[0]) if entry else None
        return [entry[0], value] if value is not None else part

    def _request_only_count(self) -> int:
        return sum(
            1
            for ids in self.request_only.values()
            for identifier in ids
            if identifier.startswith("rq-")
        )

    def outputs(self, call: dict[str, Any]) -> None:
        self.established.update(call["outputs"])


def _occurrences(
    request: ModelRequest, lineage: _Lineage, conversation: str, call: str
) -> dict[str, Any]:
    def occurrence(part: dict[str, Any], role: str) -> dict[str, Any]:
        return {"part": part, **lineage.assign(part, role, conversation, call)}

    lineage.in_messages = False
    system = [occurrence(part, "system") for part in request.system]
    lineage.in_messages = True
    return {
        "api": request.api,
        "system": system,
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
        lineage.next_request(request)
        out[call_id] = _occurrences(request, lineage, call["conversation"], call_id)
        lineage.outputs(call)
    return {
        "transcript": path.relative_to(REPO).as_posix(),
        "calls": out,
        "unpaired_requests": unanswered,
    }
