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
from harness.fakes import script
from harness.truth import unexported
from harness.truth.wire import ModelCall

FORMAT = "sideseat.truth/3"

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
    "citations": [(content.CITATIONS,)],
    "file_references": [(content.FILE_REFERENCES,)],
    "multi_agent": [(content.MULTI_AGENT,)],
    "mcp_tools": [(content.MCP,)],
    "server_tools": [(content.SERVER_TOOLS,)],
    "trailing_tool": [(content.TRAILING_TOOL,)],
}

#: Attachments of each scenario's user turn, by scenario.
MEDIA = {
    "files": (
        ("image", "image/jpeg", "img.jpg"),
        ("document", "application/pdf", "task.pdf"),
    ),
    "citations": (("document", "application/pdf", "task.pdf"),),
}


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


#: Why an encrypted reasoning item with no text is not yet owed, naming the work that will owe it.
_UNTEXTED_PENDING = (
    "the provider returned an encrypted reasoning item with no text at all; how it is shown waits on "
    "SideML slice S1, OpenAI reasoning items (docs/engineering/sideml-provider-review.md)"
)


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


def media_facts(scenario: str = "files") -> list[dict[str, Any]]:
    values = []
    for modality, media_type, name in MEDIA.get(scenario, ()):
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


def reference_facts() -> list[dict[str, Any]]:
    """The ``file_references`` attachments: where each one is, since none carries its bytes.

    The request names the uploaded PDF only as a file, so the fact does not say it is a document.
    """
    return [
        {
            "modality": "image",
            "media_type": None,
            "source": "url",
            "reference": content.IMAGE_URL,
        },
        {
            "modality": "file",
            "media_type": None,
            "source": "file_id",
            "reference": script.file_id((ASSETS / "task.pdf").read_bytes()),
        },
    ]


def _usage(call: ModelCall) -> dict[str, Any] | None:
    return call.usage.to_json() if call.usage else None


@dataclass(frozen=True)
class Framework:
    """How a suite's framework turns the model's output into a conversation, as the suite declares it.

    Read from the suite manifest's ``truth`` table. Some frameworks do not let the model call tools
    directly: the model answers with one *plan* call whose member lists actions, and the framework
    executes each - under ids of its own, since the wire never had any - and reports each action and
    its result. Those actions are facts of the conversation: what the model asked for is on the wire,
    inside the plan's arguments.
    """

    #: Plan tool name -> the argument member listing its actions, each ``{action_name: arguments}``.
    action_plans: dict[str, str] = field(default_factory=dict)
    #: Actions that end the turn; the first is the turn's answer.
    turn_ending_actions: frozenset[str] = frozenset()
    #: Action -> the argument members whose value the framework reports as the action's result, the
    #: first present one deciding (a ``done`` action's result is its answer). Also read for a terminal
    #: tool the framework reports a result for (smolagents' ``final_answer``).
    action_results: dict[str, tuple[str, ...]] = field(default_factory=dict)
    #: The framework re-sends the task inside a state message of its own on every step of a turn.
    restates_prompt: bool = False
    #: A turn the model ends with text is ended by the framework through this tool, called with the text
    #: under this argument: ``{tool, argument}`` (smolagents runs ``final_answer(answer=...)`` itself).
    text_answer_action: dict[str, str] = field(default_factory=dict)
    #: Scenario -> actions the framework performs before the model's first call (opening a URL the task
    #: names), each ``{action_name: arguments}``.
    initial_actions: dict[str, list[dict[str, Any]]] = field(default_factory=dict)
    #: What the framework's telemetry leaves out, by a closed vocabulary, with the reason. Each becomes an
    #: absence gap the Rust rubric proves against every fixture (``message_truth::absence``):
    #: ``tool_call_ids`` - the model's tool-call ids (``id_not_exported``); ``tool_calling_rounds`` - the
    #: responses that only call tools, whose parts are recorded one execution at a time
    #: (``call_not_exported``); ``model``, ``response_model``, ``response_id``, ``finish`` - a call's
    #: metadata the producer states wrongly with the right value in no payload; ``reasoning_kind`` -
    #: visible reasoning the producer records as plain text (``kind_not_exported``);
    #: ``withheld_reasoning`` - signed reasoning with no text the producer exports no part for
    #: (``not_exported``); ``reasoning_signature`` - that reasoning's signature, the step itself exported
    #: (``signature_not_exported``); ``reasoning_output`` - that reasoning missing from the span that
    #: produced it, a later request re-sending it (``output_not_exported``); ``reasoning_span_signature``
    #: - that reasoning's signature missing from the span that produced it, another carrier holding it
    #: (``span_signature_not_exported``); ``tool_calling_round_output`` - the tool calls of a response that
    #: only calls tools, missing from the span that produced it, other carriers holding them
    #: (``output_not_exported``); ``parallel_tool_results`` - the results of every tool call but the
    #: first of one response, where the producer records only the first of the turn's tool messages
    #: (``not_exported``; it withdraws them); ``structured_tool_results`` - the results a tool returned as
    #: data rather than text, where the producer records only text (``not_exported``; it withdraws them);
    #: ``media`` - the
    #: attachments a user sent, whose bytes no payload holds (``not_exported``; ``modalities`` limits it to
    #: those modalities, and it withdraws the fact, so it holds for every capture or none)
    #: (``metadata_not_exported``; ``values`` limits it to calls whose truth has one of them). Each
    #: value is the reason, or ``{reason, scenarios, modes}`` when only
    #: some scenarios' - or, by scenario, some releases' (``modes = {streaming = ["native@1.0b1"]}``) -
    #: telemetry leaves it out.
    unexported: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def of(cls, table: dict[str, Any] | None) -> "Framework":
        table = table or {}
        return cls(
            action_plans=dict(table.get("action_plans", {})),
            turn_ending_actions=frozenset(table.get("turn_ending_actions", ())),
            action_results={
                name: tuple(members)
                for name, members in table.get("action_results", {}).items()
            },
            restates_prompt=bool(table.get("restates_prompt", False)),
            initial_actions={
                scenario: list(actions)
                for scenario, actions in table.get("initial_actions", {}).items()
            },
            unexported=dict(table.get("unexported", {})),
            text_answer_action=dict(table.get("text_answer_action", {})),
        )

    def actions(self, name: str, arguments: Any) -> list[tuple[str, Any]] | None:
        """The actions a plan call lists, or ``None`` when the call is not a plan.

        Fails closed: a plan without its declared list, an entry that is not one ``{action:
        arguments}`` pair, or an action after the turn's end is a derivation error rather than a plan
        with nothing to assert.
        """
        member = self.action_plans.get(name)
        if member is None:
            return None
        listed = arguments.get(member) if isinstance(arguments, dict) else None
        if not isinstance(listed, list) or not listed:
            raise ValueError(f"the {name} plan has no {member!r} list: {arguments!r}")
        actions = []
        for entry in listed:
            if not isinstance(entry, dict) or len(entry) != 1:
                raise ValueError(
                    f"a {name} action is not one {{name: arguments}} pair: {entry!r}"
                )
            ((action, value),) = entry.items()
            if actions and actions[-1][0] in self.turn_ending_actions:
                raise ValueError(
                    f"the {name} plan acts after ending the turn: {listed!r}"
                )
            actions.append((action, value))
        return actions

    def action_result(self, name: str, arguments: Any) -> ToolOutcome | None:
        """The result the framework reports for an action it carried out itself."""
        for member in self.action_results.get(name, ()):
            if isinstance(arguments, dict) and arguments.get(member) is not None:
                return ToolOutcome(arguments[member], is_error=False)
        return None


@dataclass
class Options:
    """What a source can vouch for, beyond the response content itself."""

    #: The response's model, id and usage were produced by a real provider and recorded.
    metadata_from_wire: bool = True
    #: An answer built from tool results quotes the framework's rendering of them (fake models).
    answers_follow_results: bool = False
    #: The suite's declarations about its framework.
    framework: Framework = field(default_factory=Framework)


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

    state: dict[str, Any] = {
        "conversation": None,
        "pending": None,
        "turn_calls": 0,
        "initial": True,
    }
    previous_call: str | None = None
    framework = options.framework

    def add_result(
        conversation: dict[str, Any],
        call_fact: str,
        outcome: ToolOutcome | None,
        *,
        matcher: str | None = None,
        evidence: str = "script",
    ) -> bool:
        """A tool call's result as a fact, or a gap when the script cannot compute it."""
        value = builder.facts[int(call_fact.split("-")[1]) - 1]["value"]
        if outcome is None:
            builder.gap(
                "tool_result",
                "tool_not_deterministic",
                f"{value['name']} is not one of the shared deterministic tools",
                subject=call_fact,
            )
            return False
        result = builder.fact(
            conversation,
            "tool_result",
            "tool",
            evidence,
            {
                "call_id": value["id"],
                "name": value["name"],
                "value": outcome.value,
                "is_error": outcome.is_error,
            },
            # A framework renders a raised error in its own words around the message.
            require=_conversation_requirement(
                matcher or ("error_message" if outcome.is_error else "semantic")
            ),
        )
        builder.edges.append({"kind": "result_of", "from": result, "to": call_fact})
        return True

    def add_action(
        conversation: dict[str, Any], name: str, arguments: Any, evidence: str
    ) -> str:
        """An action the framework executes: a call under an id the framework assigns, and its result."""
        fact = builder.fact(
            conversation,
            "tool_call",
            "assistant",
            evidence,
            {"id": None, "name": name, "arguments": arguments},
            # Executed and reported by the framework, not returned by the model call, so it belongs to
            # the conversation rather than to the call's span.
            require=_conversation_requirement("semantic"),
        )
        add_result(
            conversation,
            fact,
            framework.action_result(name, arguments) or run_tool(name, arguments),
        )
        return fact

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
        if scenario in MEDIA:
            for media in media_facts(scenario):
                builder.fact(
                    conversation,
                    "user_media",
                    "user",
                    "script",
                    media,
                    require=_conversation_requirement("digest"),
                )
        if scenario == "file_references":
            for media in reference_facts():
                builder.fact(
                    conversation,
                    "user_media",
                    "user",
                    "script",
                    media,
                    require=_conversation_requirement("reference"),
                )
        state["turn_calls"] = 0
        if state["initial"]:
            state["initial"] = False
            for entry in framework.initial_actions.get(scenario, ()):
                for name, arguments in entry.items():
                    add_action(conversation, name, arguments, "script")

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
        state["turn_calls"] += 1
        if framework.restates_prompt and state["turn_calls"] > 1:
            builder.gap(
                "user_text",
                "framework_restates_prompt",
                "the framework re-sends the task inside its own state message on every step; the "
                "request body is unrecorded, so the message is known only to contain the prompt",
                subject=identifier,
            )
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
        provider_calls: set[str] = set()
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
                    # The fake counts the answer's output tokens from its text, so a text the
                    # script does not know is a count it does not know either.
                    if record["usage"] is not None:
                        record["usage"]["output"] = None
            elif part["type"] == "reasoning":
                # Visible reasoning is owed by its text and a provider's redaction by its presence.
                # Reasoning the model signed and withheld the text of - an empty text, not an unknown one -
                # is owed by its signed mark, since there is no text to compare.
                match = (
                    "exact"
                    if part["text"]
                    else "presence"
                    if part["redacted"]
                    else "signed"
                    if unexported.withheld(part)
                    else None
                )
                fact = builder.fact(
                    conversation,
                    "reasoning",
                    "assistant",
                    evidence,
                    {key: part[key] for key in ("text", "signed", "redacted")},
                    call=identifier,
                    require=_model_call_requirement(match) if match else None,
                )
                # The signature's digest, outside the value: what tells two withheld turns apart when
                # their telemetry is searched, and no part of what a view must show.
                if part.get("seal"):
                    builder.facts[-1]["seal"] = part["seal"]
                if match is None:
                    builder.gap(
                        "reasoning",
                        "reasoning_text_omitted",
                        _UNTEXTED_PENDING,
                        subject=fact,
                    )
            elif part["type"] == "server_tool_call":
                # A tool the provider ran inside this response. An instrumentation re-shapes its
                # payload in its own words, so the call is owed by its name and what it searched for,
                # and its result by the sources the run found - never by an exact encoding.
                fact = builder.fact(
                    conversation,
                    "tool_call",
                    "assistant",
                    "wire",
                    {key: part[key] for key in ("id", "name", "arguments")},
                    call=identifier,
                    require=_model_call_requirement("contains"),
                )
                provider_calls.add(fact)
                if part["result"] is not None:
                    add_result(
                        conversation,
                        fact,
                        ToolOutcome(
                            part["result"], is_error=part.get("is_error", False)
                        ),
                        matcher="contains",
                        evidence="wire",
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
        turn_answer: str | None = None
        for fact_id in call_facts:
            fact = builder.facts[int(fact_id.split("-")[1]) - 1]
            # A provider's own tool brought its result with it.
            if fact["kind"] != "tool_call" or fact_id in provider_calls:
                continue
            value = fact["value"]
            if value["name"] in TERMINAL_TOOLS:
                # A terminal tool has no result, unless the framework reports one it declares.
                declared = framework.action_result(value["name"], value["arguments"])
                if declared is not None and add_result(conversation, fact_id, declared):
                    results_seen = True
                continue
            actions = framework.actions(value["name"], value["arguments"])
            if actions is not None:
                # A plan has no result of its own; each action it lists does.
                for name, arguments in actions:
                    action = add_action(conversation, name, arguments, evidence)
                    results_seen = True
                    if name in framework.turn_ending_actions and turn_answer is None:
                        turn_answer = action
                continue
            if add_result(
                conversation, fact_id, run_tool(value["name"], value["arguments"])
            ):
                results_seen = True
        # A turn the model ends in text that the framework then answers through a tool of its own.
        if (
            framework.text_answer_action
            and not call.tool_calls
            and (answer := _final_answer(builder, call_facts))
        ):
            text = builder.facts[int(answer.split("-")[1]) - 1]["value"]["text"]
            turn_answer = add_action(
                conversation,
                framework.text_answer_action["tool"],
                _text_action_arguments(text, framework.text_answer_action),
                evidence,
            )
        terminal = turn_answer is not None or all(
            part["name"] in TERMINAL_TOOLS for part in call.tool_calls
        )
        final = turn_answer or (
            _final_answer(builder, call_facts) if terminal else None
        )
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
    unexported.apply(builder, framework)
    return builder


def _text_action_arguments(text: str, action: dict[str, str]) -> dict[str, Any]:
    """The arguments a framework calls its answering tool with, for a turn the model ended in text.

    A model that writes the call as a JSON action in its text (``Action: {"name": ..., "arguments":
    ...}``, which smolagents parses) is answered with those arguments; any other text is the answer.
    """
    start, end = text.find("{"), text.rfind("}")
    if 0 <= start < end:
        try:
            parsed = json.loads(text[start : end + 1])
        except json.JSONDecodeError:
            parsed = None
        if (
            isinstance(parsed, dict)
            and parsed.get("name") == action["tool"]
            and isinstance(parsed.get("arguments"), dict)
        ):
            return parsed["arguments"]
    return {action["argument"]: text}


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
