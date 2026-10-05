"""Assemble a scenario's ground truth from its recorded model responses and its script.

What the model said comes from the wire (:mod:`harness.truth.wire`). What was said *to* the model
cannot: cassettes keep only a digest of each request body. The user side is therefore derived from
the scenario script - every suite sends the shared prompts of :mod:`harness.content` - and each tool
result is recomputed from the decoded call's arguments, because the shared tools are deterministic.
Facts nothing here can know (a framework's system prompt, a browser's page state, a handoff tool's
result) are listed as gaps and asserted by nobody, so an unknowable fact never fails a fixture.

The document separates facts from where they must appear: each fact names the evidence it rests on
and a requirement - the anchor (the model call that produced it, or the conversation it belongs to),
the views it must be visible in, and how many times - so a Rust test can apply the same view
semantics to every producer without knowing any provider or framework.
"""

from __future__ import annotations

import ast
import hashlib
import json
import operator
import re
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from harness import content
from harness.truth.wire import ModelCall

FORMAT = "sideseat.truth/2"

REPO = Path(__file__).resolve().parents[5]
ASSETS = REPO / "examples" / "assets"

MODEL_CALL_VIEWS = ["span", "trace", "session", "feed"]
CONVERSATION_VIEWS = ["trace", "session", "feed"]

#: The user turns each catalog scenario sends, one tuple per conversation (trace).
PROMPTS: dict[str, list[tuple[str, ...]]] = {
    "chat": [(content.CHAT,)],
    "multi_turn": [content.MULTI_TURN],
    "tool_use": [(content.TOOL_USE,)],
    "session": [(question,) for question in content.SESSION],
    "error": [(content.ERROR,)],
    "streaming": [(content.STREAMING,)],
    "structured_output": [(content.STRUCTURED,)],
    "reasoning": [(content.REASONING,)],
    "files": [(content.FILES,)],
    "multi_agent": [(content.MULTI_AGENT,)],
    "mcp_tools": [(content.MCP,)],
}

#: Attachments of the ``files`` scenario's user turn.
MEDIA = (
    ("image", "image/jpeg", "img.jpg"),
    ("document", "application/pdf", "task.pdf"),
)


def topology(scenario: str) -> str:
    if scenario == "session":
        return "sessions"
    if scenario == "multi_agent":
        return "multi_agent"
    return "linear"


# --- Tool results -------------------------------------------------------------------------------


_BINARY = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
}
_UNARY = {ast.UAdd: operator.pos, ast.USub: operator.neg}


def calculate(expression: str) -> float | int:
    """The MCP calculator (``scripts/tools/mcp-calculator``): pure arithmetic, evaluated safely."""

    def evaluate(node: ast.AST) -> Any:
        if isinstance(node, ast.Expression):
            return evaluate(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, int | float):
            return node.value
        if isinstance(node, ast.BinOp) and type(node.op) in _BINARY:
            return _BINARY[type(node.op)](evaluate(node.left), evaluate(node.right))
        if isinstance(node, ast.UnaryOp) and type(node.op) in _UNARY:
            return _UNARY[type(node.op)](evaluate(node.operand))
        raise ValueError(f"not arithmetic: {ast.dump(node)}")

    return evaluate(ast.parse(expression, mode="eval"))


#: The deterministic tools, by the name the shared scripts give them.
TOOLS: dict[str, Callable[..., Any]] = {
    "get_weather": content.get_weather,
    "get_precipitation": content.get_precipitation,
    "book_flight": content.book_flight,
    "calculate": calculate,
}


#: Tools an agent calls to end its turn with an answer (smolagents' ``final_answer``): the call is
#: the answer, and no result follows it.
TERMINAL_TOOLS = frozenset({"final_answer"})


def resolve_tool(name: str) -> str | None:
    """The shared tool a framework's tool name refers to.

    Frameworks namespace tool names - ``travel-get_weather`` (Semantic Kernel),
    ``mcp__travel__get_weather`` (Claude Code) - so a known name after a separator counts.
    """
    for known in TOOLS:
        if re.search(rf"(?:^|[-.:/]|__){re.escape(known)}$", name):
            return known
    return None


@dataclass(frozen=True)
class ToolOutcome:
    value: Any
    is_error: bool


def run_tool(name: str, arguments: Any) -> ToolOutcome | None:
    """The result a shared tool returns for ``arguments``, or ``None`` if the tool is not shared."""
    known = resolve_tool(name)
    if known is None or not isinstance(arguments, dict):
        return None
    # A transport may add members of its own (the Strands TypeScript SDK sends ``_meta``).
    accepted = {
        key: value for key, value in arguments.items() if not key.startswith("_")
    }
    try:
        return ToolOutcome(TOOLS[known](**accepted), is_error=False)
    except content.BookingUnavailable as error:
        return ToolOutcome(str(error), is_error=True)
    except TypeError as error:
        # The model called a shared tool with arguments it does not take; the framework rejects
        # the call with a message of its own, which the script cannot predict.
        return ToolOutcome(str(error), is_error=True)


# --- The document -------------------------------------------------------------------------------


@dataclass
class Builder:
    """Collects facts, calls, edges and gaps with stable identifiers."""

    producer: str
    scenario: str
    facts: list[dict[str, Any]] = field(default_factory=list)
    calls: list[dict[str, Any]] = field(default_factory=list)
    edges: list[dict[str, Any]] = field(default_factory=list)
    gaps: list[dict[str, Any]] = field(default_factory=list)
    conversations: list[dict[str, Any]] = field(default_factory=list)

    def conversation(self) -> dict[str, Any]:
        value = {
            "id": f"conv-{len(self.conversations) + 1}",
            "sequence": [],
            "final_answers": [],
        }
        self.conversations.append(value)
        return value

    def fact(
        self,
        conversation: dict[str, Any],
        kind: str,
        role: str,
        evidence: str,
        value: dict[str, Any],
        *,
        call: str | None = None,
        require: dict[str, Any] | None,
    ) -> str:
        identifier = f"fact-{len(self.facts) + 1:03d}"
        fact: dict[str, Any] = {
            "id": identifier,
            "kind": kind,
            "role": role,
            "conversation": conversation["id"],
            "evidence": evidence,
            "value": value,
            "require": require,
        }
        if call is not None:
            fact["call"] = call
        self.facts.append(fact)
        conversation["sequence"].append(identifier)
        return identifier

    def gap(
        self, fact: str, reason: str, detail: str, subject: str | None = None
    ) -> None:
        entry = {"fact": fact, "reason": reason, "detail": detail}
        if subject is not None:
            entry["subject"] = subject
        if entry not in self.gaps:
            self.gaps.append(entry)


def _model_call_requirement(match: str = "exact") -> dict[str, Any]:
    return {
        "anchor": "model_call",
        "views": MODEL_CALL_VIEWS,
        "cardinality": "exactly_once",
        "match": match,
    }


def _conversation_requirement(match: str) -> dict[str, Any]:
    return {
        "anchor": "conversation",
        "views": CONVERSATION_VIEWS,
        "cardinality": "exactly_once",
        "match": match,
    }


def media_facts() -> list[dict[str, Any]]:
    values = []
    for modality, media_type, name in MEDIA:
        raw = (ASSETS / name).read_bytes()
        values.append(
            {
                "modality": modality,
                "media_type": media_type,
                "sha256": hashlib.sha256(raw).hexdigest(),
                "bytes": len(raw),
            }
        )
    return values


def _usage(call: ModelCall) -> dict[str, Any] | None:
    return call.usage.to_json() if call.usage else None


@dataclass
class Options:
    """What a source can vouch for, beyond the response content itself."""

    #: The response's model, id and usage were produced by a real provider and recorded.
    metadata_from_wire: bool = True
    #: An answer built from tool results quotes the framework's rendering of them (fake models).
    answers_follow_results: bool = False


def assemble(
    producer: str,
    scenario: str,
    calls: list[ModelCall],
    *,
    options: Options | None = None,
    prompts: list[tuple[str, ...]] | None = None,
) -> Builder:
    """Lays the decoded model calls out as the conversation(s) the scenario held."""
    options = options or Options()
    builder = Builder(producer, scenario)
    groups = [list(group) for group in (prompts or PROMPTS[scenario])]
    linear = topology(scenario) != "multi_agent"
    builder.gap(
        "system_prompt",
        "request_body_unrecorded",
        "cassettes keep a digest of each request, not its body; the system prompt is asserted "
        "only where a response echoes it",
    )
    if not linear:
        builder.gap(
            "routing",
            "multi_agent_routing",
            "which agent made each call, and what one agent passed another, is the framework's; "
            "only each call's own output and the shared tools' results are asserted",
        )

    state: dict[str, Any] = {"conversation": None, "pending": None}
    previous_call: str | None = None

    def open_turn(new_conversation: bool) -> None:
        nonlocal previous_call
        if state["conversation"] is None or new_conversation:
            state["conversation"] = builder.conversation()
            previous_call = None
        conversation = state["conversation"]
        prompt = groups[0].pop(0)
        if not groups[0]:
            groups.pop(0)
        state["pending"] = builder.fact(
            conversation,
            "user_text",
            "user",
            "script",
            {"text": prompt},
            # A framework may wrap the application's prompt in a template of its own.
            require=_conversation_requirement("contains"),
        )
        if scenario == "files":
            for media in media_facts():
                builder.fact(
                    conversation,
                    "user_media",
                    "user",
                    "script",
                    media,
                    require=_conversation_requirement("digest"),
                )

    def next_turn() -> None:
        # A new conversation starts when the scenario's next prompt belongs to another trace.
        sessions = topology(scenario) == "sessions"
        open_turn(new_conversation=sessions or state["conversation"] is None)

    next_turn()
    results_seen = False
    for call in calls:
        conversation = state["conversation"]
        identifier = f"call-{len(builder.calls) + 1:03d}"
        record: dict[str, Any] = {
            "id": identifier,
            "attempt": 1,
            "outcome": "success" if call.ok else "failed",
            "conversation": conversation["id"],
            "api": call.api,
            "streamed": call.streamed,
            "status": call.status,
            "model": call.model if options.metadata_from_wire else None,
            "response_model": call.response_model,
            "response_id": call.response_id if options.metadata_from_wire else None,
            "stop_reason": call.stop_reason,
            "finish": call.finish,
            "usage": _usage(call),
            "outputs": [],
        }
        if builder.calls and builder.calls[-1]["outcome"] == "failed":
            record["attempt"] = builder.calls[-1]["attempt"] + 1
        builder.calls.append(record)
        if not call.ok:
            record["error"] = call.error
            builder.gap(
                "failed_attempt",
                "no_output_obligation",
                f"{identifier} failed with HTTP {call.status}; the request was retried",
                subject=identifier,
            )
            continue
        if linear and previous_call is not None:
            builder.edges.append(
                {"kind": "call_order", "before": previous_call, "after": identifier}
            )
        if state["pending"] is not None:
            builder.edges.append(
                {"kind": "prompt_of", "from": state["pending"], "to": identifier}
            )
            state["pending"] = None
        previous_call = identifier
        if call.system_echo and not any(
            f["kind"] == "system" and f["conversation"] == conversation["id"]
            for f in builder.facts
        ):
            fact = builder.fact(
                conversation,
                "system",
                "system",
                "wire-echo",
                {"text": call.system_echo},
                require=_conversation_requirement("exact"),
            )
            # The echo is the request's, so it precedes the call's output in the conversation.
            sequence = conversation["sequence"]
            sequence.remove(fact)
            sequence.insert(0, fact)
        evidence = "wire" if options.metadata_from_wire else "script"
        # An answer a fake model composes from tool results quotes them as the framework rendered
        # them, which the script does not know.
        unknowable_text = options.answers_follow_results and results_seen
        call_facts: list[str] = []
        for part in _joined_text(call.parts):
            if part["type"] == "text":
                value = {"text": part["text"]}
                if len(part.get("segments", ())) > 1:
                    value["segments"] = part["segments"]
                fact = builder.fact(
                    conversation,
                    "text",
                    "assistant",
                    evidence,
                    value,
                    call=identifier,
                    require=None
                    if unknowable_text
                    else _model_call_requirement(_text_match(scenario, part["text"])),
                )
                if unknowable_text:
                    builder.gap(
                        "assistant_text",
                        "answer_quotes_framework_rendering",
                        "a fake model's answer quotes the tool results as the framework rendered "
                        "them, which the script does not know",
                        subject=fact,
                    )
            elif part["type"] == "reasoning":
                visible = bool(part["text"])
                fact = builder.fact(
                    conversation,
                    "reasoning",
                    "assistant",
                    evidence,
                    {key: part[key] for key in ("text", "signed", "redacted")},
                    call=identifier,
                    require=_model_call_requirement("exact")
                    if visible
                    else _model_call_requirement("presence")
                    if part["redacted"]
                    else None,
                )
                if not visible and not part["redacted"]:
                    builder.gap(
                        "reasoning",
                        "reasoning_text_omitted",
                        "the provider returned a signed reasoning block with no visible text; "
                        "whether a reconstruction shows a placeholder for it is a product "
                        "decision, not a fidelity fact",
                        subject=fact,
                    )
            else:
                fact = builder.fact(
                    conversation,
                    "tool_call",
                    "assistant",
                    evidence,
                    {key: part[key] for key in ("id", "name", "arguments")},
                    call=identifier,
                    require=_model_call_requirement("semantic"),
                )
            call_facts.append(fact)
        record["outputs"] = call_facts
        for fact_id in call_facts:
            fact = builder.facts[int(fact_id.split("-")[1]) - 1]
            if fact["kind"] != "tool_call":
                continue
            value = fact["value"]
            if value["name"] in TERMINAL_TOOLS:
                continue
            outcome = run_tool(value["name"], value["arguments"])
            if outcome is None:
                builder.gap(
                    "tool_result",
                    "tool_not_deterministic",
                    f"{value['name']} is not one of the shared deterministic tools",
                    subject=fact_id,
                )
                continue
            results_seen = True
            result = builder.fact(
                conversation,
                "tool_result",
                "tool",
                "script",
                {
                    "call_id": value["id"],
                    "name": value["name"],
                    "value": outcome.value,
                    "is_error": outcome.is_error,
                },
                # A framework renders a raised error in its own words around the message.
                require=_conversation_requirement(
                    "error_message" if outcome.is_error else "semantic"
                ),
            )
            builder.edges.append({"kind": "result_of", "from": result, "to": fact_id})
        terminal = all(part["name"] in TERMINAL_TOOLS for part in call.tool_calls)
        final = _final_answer(builder, call_facts) if terminal else None
        if final:
            # One per answered prompt: a multi-turn conversation has an answer for every question.
            conversation["final_answers"].append(final)
        if terminal and groups:
            results_seen = False
            next_turn()
    for group in groups:
        for prompt in group:
            builder.gap(
                "user_text",
                "prompt_without_model_call",
                f"the script sends {prompt!r}, but no recorded call answers it",
            )
    return builder


def _joined_text(parts: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Adjacent text blocks of one response joined into one text, keeping the blocks as segments.

    A provider may split one answer into several blocks (Anthropic does around citations); joining
    them is a faithful rendering, so the truth holds the joined text and the original segments.
    """
    joined: list[dict[str, Any]] = []
    for part in parts:
        if part["type"] == "text" and joined and joined[-1]["type"] == "text":
            joined[-1]["text"] += part["text"]
            joined[-1]["segments"].append(part["text"])
        elif part["type"] == "text":
            joined.append({**part, "segments": [part["text"]]})
        else:
            joined.append(part)
    return joined


def _text_match(scenario: str, text: str) -> str:
    """``json`` for a schema-constrained answer, which a reconstruction may hold as a JSON block."""
    if scenario != "structured_output":
        return "exact"
    try:
        return "json" if isinstance(json.loads(text), dict | list) else "exact"
    except json.JSONDecodeError:
        return "exact"


def _final_answer(builder: Builder, call_facts: list[str]) -> str | None:
    """The fact holding the call's answer: its last text, or a terminal ``final_answer`` tool call."""
    for fact_id in reversed(call_facts):
        fact = builder.facts[int(fact_id.split("-")[1]) - 1]
        if fact["kind"] == "text" and fact["value"]["text"].strip():
            return fact_id
        if fact["kind"] == "tool_call" and fact["value"]["name"] == "final_answer":
            return fact_id
    return None


def document(
    builder: Builder,
    *,
    fixtures: list[str],
    source: dict[str, Any],
    request_bodies: str,
) -> dict[str, Any]:
    successful = [call for call in builder.calls if call["outcome"] == "success"]
    return {
        "format": FORMAT,
        "producer": builder.producer,
        "scenario": builder.scenario,
        "fixtures": fixtures,
        "source": source,
        "generator": "examples/python/harness: python -m harness truth",
        "topology": topology(builder.scenario)
        if builder.scenario in PROMPTS
        else "linear",
        "request_bodies": request_bodies,
        "model_calls": len(successful),
        "calls": builder.calls,
        "conversations": builder.conversations,
        "facts": builder.facts,
        "edges": builder.edges,
        "gaps": builder.gaps,
    }
