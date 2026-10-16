"""What a framework does to a conversation that no model said, as a suite declares it for the truth derivation.

A suite's ``[tool.sideseat-example.truth]`` table: the actions its plans list, the results it reports for actions
it runs itself, the texts and prompts it writes, the step budget past which a call is its fallback, and what its
telemetry leaves out.
"""

from __future__ import annotations

import json

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class ToolOutcome:
    value: Any
    is_error: bool


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
    #: metadata the producer states wrongly with the right value in no payload; ``usage_input``,
    #: ``usage_output``, ``usage_cache_read``, ``usage_cache_write``, ``usage_reasoning`` - a usage count the
    #: producer states wrongly with the right count in no payload (``metadata_not_exported`` on
    #: ``usage.<count>``); ``reasoning_kind`` -
    #: visible reasoning the producer records as plain text (``kind_not_exported``);
    #: ``withheld_reasoning`` - signed reasoning with no text the producer exports no part for
    #: (``not_exported``); ``reasoning_signature`` - that reasoning's signature, the step itself exported
    #: (``signature_not_exported``); ``reasoning_output`` - that reasoning missing from the span that
    #: produced it, a later request re-sending it (``output_not_exported``); ``visible_reasoning_output`` -
    #: visible reasoning missing from the span that produced it, another carrier of the capture holding it
    #: (``output_not_exported``); ``reasoning_span_signature``
    #: - that reasoning's signature missing from the span that produced it, another carrier holding it
    #: (``span_signature_not_exported``); ``tool_calling_round_output`` - the tool calls of a response that
    #: only calls tools, missing from the span that produced it, other carriers holding them
    #: (``output_not_exported``); ``parallel_tool_results`` - the results of every tool call but the
    #: first of one response, where the producer records only the first of the turn's tool messages
    #: (``not_exported``; it withdraws them); ``structured_tool_results`` - the results a tool returned as
    #: data rather than text, where the producer records only text (``not_exported``; it withdraws them);
    #: ``parallel_call_order`` - the relative order of one response's parallel tool calls, where no payload
    #: holds two of them together (``call_order_not_exported``; their presence stays asserted);
    #: ``tool_round_call_spans`` - the own span of a call that answers a tool round, where the producer
    #: records the whole run on one agent span and no model-call span (``call_span_not_exported``);
    #: ``tool_round_run_spans`` - the own span of a call that answers a tool round, where the producer
    #: records a whole run of calls on one model-call span, the run's first call's (``call_folded_into_run_span``);
    #: ``tool_round_traces`` - the trace context of such a call, which the producer starts a new trace for
    #: (``trace_not_propagated``); ``run_call_spans`` - the place of every model call in the agent run that
    #: made it, where the producer opens the run's span without making it current, so its calls are the run's
    #: siblings rather than its children (``call_not_nested_under_its_run``);
    #: ``media`` - the
    #: attachments a user sent, whose bytes no payload holds (``not_exported``; ``modalities`` limits it to
    #: those modalities, and it withdraws the fact, so it holds for every capture or none); ``media_bytes`` - only
    #: the bytes of those attachments, the producer exporting a stand-in in their place
    #: (``bytes_not_exported``; the attachment stays owed, shown withheld under the stand-in, declared as
    #: ``{reason, stand_in[, named]}`` with ``stand_in`` a template of ``{bytes}`` and ``{base64_length}``, and
    #: ``named`` the modalities the producer shows under the name of the file they were read from)
    #: (``metadata_not_exported``; ``values`` limits it to calls whose truth has one of them). Each
    #: value is the reason, or ``{reason, scenarios, modes}`` when only
    #: some scenarios' - or, by scenario, some releases' (``modes = {streaming = ["native@1.0b1"]}``) -
    #: telemetry leaves it out.
    unexported: dict[str, Any] = field(default_factory=dict)
    #: Text the framework writes into the conversation itself rather than the model: each ``{text, source,
    #: scenarios[, after, reaches_model]}``, ``source`` citing the line of the framework that writes it and
    #: ``after`` the successful call it is written after (the last by default) - or ``{returns_arguments_of,
    #: source, scenarios[, reaches_model]}``, a structured result the framework returns: the arguments of the
    #: model's one call of that tool as their compact JSON, written after that call and owed as JSON. It becomes a fact resting on
    #: ``framework`` evidence, which the rubric proves per capture (``message_truth::authored``): byte for byte in
    #: a span's payload, held by no other fact, sent by no request as user or system content, placed where the
    #: raw timing says, and carried by every later request - or, with ``reaches_model = false``, a reply the
    #: framework hands its caller and keeps out of the model's context, carried by none.
    framework_authored: tuple[dict[str, Any], ...] = ()
    #: Scenario -> how many model calls one turn's steps may make. A call past the limit is the framework's
    #: fallback rather than a step: it asks for the answer without the step's tools, so its text is the turn's
    #: answer as written, never routed through ``text_answer_action``, and the ``framework_prompts`` of the
    #: scenario are what it was sent.
    step_limits: dict[str, int] = field(default_factory=dict)
    #: Prompts the framework writes and sends the model itself on a fallback call: each ``{template, source,
    #: scenarios}``, ``template`` the framework's own text with ``{task}`` for the turn's prompt and ``source``
    #: citing where the framework defines it. It becomes a user fact resting on ``framework`` evidence, sent to
    #: the fallback call, which the rubric proves per capture byte for byte against that call's request
    #: (``message_truth::authored``).
    framework_prompts: tuple[dict[str, Any], ...] = ()
    #: Turns the framework's telemetry states it sent the model and its client then dropped: each ``{text,
    #: reason, source, scenarios, modes}``, ``text`` the framework's own literal, ``source`` citing where it
    #: composes the turn and where its client drops it, ``modes`` the releases (``{scenario: [mode]}``) that do
    #: both. The framework appends the turn to a request whose last message holds nothing but tool results,
    #: so that is the call it is declared for. Each becomes a ``producer_states_unsent`` gap the rubric proves
    #: per capture: the call's span states the text byte for byte and its recorded request does not carry it.
    states_unsent: tuple[dict[str, Any], ...] = ()
    #: Tool results the framework hands the model as its own rendering of what the tool returned: each
    #: ``{format, source, scenarios}``. ``format`` is a closed vocabulary - ``mcp_call_tool_result_str``, the
    #: ``str()`` of an MCP SDK ``CallToolResult`` model - and ``source`` cites where the framework renders it. Each
    #: result fact of those scenarios becomes the text the recorded requests sent, checked against the format
    #: and the script's value (``rendered_results``), so a rendering of anything else is refused.
    result_renderings: tuple[dict[str, Any], ...] = ()
    #: Tool results the framework sends inside an envelope of its own: each ``{format, source, scenarios}``.
    #: ``format`` is a closed vocabulary - ``genkit_googleai_tool_output``, Genkit's Google AI plugin sending a
    #: tool output that is not a map as the function response ``{"name": <tool>, "content": <output>}`` - and
    #: ``source`` cites where the framework builds it. In those scenarios a request part holding exactly that
    #: envelope is the result fact it carries: the tool of that name whose output, as the script computed it,
    #: is the envelope's content, and the part states that content (``unwrap_result_envelopes``).
    result_envelopes: tuple[dict[str, Any], ...] = ()
    #: Tool results some releases encode their own way: each ``{format, reason, source, scenarios, modes}``.
    #: ``format`` is a closed vocabulary - ``python_str``, the release sending the model ``str()`` of the value
    #: where the current release sends JSON - ``source`` cites the release's encoder, and ``modes`` names the
    #: releases per scenario. Each result fact of those releases gets a ``result_encoding`` gap: the rubric
    #: proves the release's payloads hold that encoding of the value, and release invariance compares those
    #: results by value (``encoding_gaps``).
    result_encodings: tuple[dict[str, Any], ...] = ()

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
            framework_authored=tuple(
                _authored(entry) for entry in table.get("framework_authored", ())
            ),
            step_limits={
                scenario: int(limit)
                for scenario, limit in table.get("step_limits", {}).items()
            },
            framework_prompts=tuple(
                _prompt_template(entry) for entry in table.get("framework_prompts", ())
            ),
            states_unsent=tuple(
                _states_unsent(entry) for entry in table.get("states_unsent", ())
            ),
            result_renderings=tuple(
                _result_rendering(entry) for entry in table.get("result_renderings", ())
            ),
            result_envelopes=tuple(
                _result_envelope(entry) for entry in table.get("result_envelopes", ())
            ),
            result_encodings=tuple(
                _result_encoding(entry) for entry in table.get("result_encodings", ())
            ),
        )

    def authored(self, scenario: str) -> list[dict[str, Any]]:
        """The texts the framework writes itself in this scenario."""
        return [
            entry for entry in self.framework_authored if scenario in entry["scenarios"]
        ]

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


def _authored(entry: dict[str, Any]) -> dict[str, Any]:
    """One declared framework text, refused unless it states its text, its source and its scenarios.

    ``after`` is the ordinal of the successful call the framework writes it after, from 1; without it, the
    scenario's last. The rubric checks the place against the raw payload's timing. ``reaches_model`` (true by
    default) says whether the framework sends the text to the model later, which the rubric also checks.
    ``returns_arguments_of`` names a tool instead of a text: the framework returns the arguments of the model's
    one call of it as the run's structured result, written as their compact JSON after that call.
    """
    after = entry.get("after")
    returned = entry.get("returns_arguments_of")
    if returned is not None and ("text" in entry or "after" in entry):
        raise ValueError(
            f"a framework_authored entry returning a call's arguments states no text and no place: {entry!r}"
        )
    if not (
        (
            (isinstance(entry.get("text"), str) and entry["text"])
            or (isinstance(returned, str) and returned)
        )
        and isinstance(entry.get("source"), str)
        and entry["source"]
        and isinstance(entry.get("scenarios"), list)
        and entry["scenarios"]
        and (after is None or (isinstance(after, int) and after >= 1))
        and isinstance(entry.get("reaches_model", True), bool)
    ):
        raise ValueError(
            f"a framework_authored entry needs a text, a source and scenarios: {entry!r}"
        )
    return entry


def _prompt_template(entry: dict[str, Any]) -> dict[str, Any]:
    """One declared framework prompt, refused unless it states its template, its source and its scenarios."""
    if not (
        isinstance(entry.get("template"), str)
        and "{task}" in entry["template"]
        and isinstance(entry.get("source"), str)
        and entry["source"]
        and isinstance(entry.get("scenarios"), list)
        and entry["scenarios"]
    ):
        raise ValueError(
            f"a framework_prompts entry needs a template naming {{task}}, a source and scenarios: {entry!r}"
        )
    return entry


def _states_unsent(entry: dict[str, Any]) -> dict[str, Any]:
    """One declared unsent turn, refused unless it states its text, reason, source, scenarios and, for each
    scenario, the releases it holds for: a turn every release drops would be a defect of the declaration."""
    modes = entry.get("modes")
    scenarios = entry.get("scenarios")
    if not (
        all(
            isinstance(entry.get(k), str) and entry[k]
            for k in ("text", "reason", "source")
        )
        and isinstance(scenarios, list)
        and scenarios
        and isinstance(modes, dict)
        and set(modes) == set(scenarios)
        and all(isinstance(m, list) and m for m in modes.values())
    ):
        raise ValueError(
            "a states_unsent entry needs a text, a reason, a source, scenarios and the modes of each: "
            f"{entry!r}"
        )
    return entry


ENCODINGS = frozenset({"python_str"})


def _result_encoding(entry: dict[str, Any]) -> dict[str, Any]:
    """One declared result encoding, refused unless its format is known and it states a reason, a source,
    scenarios and the releases of each: an encoding every release uses is the current one, not a release's."""
    modes = entry.get("modes")
    scenarios = entry.get("scenarios")
    if not (
        entry.get("format") in ENCODINGS
        and all(
            isinstance(entry.get(k), str) and entry[k] for k in ("reason", "source")
        )
        and isinstance(scenarios, list)
        and scenarios
        and isinstance(modes, dict)
        and set(modes) == set(scenarios)
        and all(isinstance(m, list) and m for m in modes.values())
    ):
        raise ValueError(
            "a result_encodings entry needs a known format, a reason, a source, scenarios and the modes of "
            f"each: {entry!r}"
        )
    return entry


def encoding_gaps(
    framework: Framework, scenario: str, document: dict[str, Any]
) -> list[dict[str, Any]]:
    """The ``result_encoding`` gaps of a scenario: every asserted result whose value is not a text, for the
    releases a declared encoding names - a text is the same under every encoding."""
    gaps = []
    for entry in framework.result_encodings:
        if scenario not in entry["scenarios"]:
            continue
        for fact in document["facts"]:
            value = fact["value"].get("value")
            if (
                fact["kind"] != "tool_result"
                or fact["require"] is None
                or isinstance(value, str)
            ):
                continue
            gaps.append(
                {
                    "fact": "tool_result",
                    "reason": "result_encoding",
                    "detail": f"{entry['reason']} ({entry['source']})",
                    "subject": fact["id"],
                    "modes": list(entry["modes"][scenario]),
                    "text": entry["format"],
                }
            )
    return gaps


def unsent_gaps(
    framework: Framework, scenario: str, requests: dict[str, Any]
) -> list[dict[str, Any]]:
    """The ``producer_states_unsent`` gaps of a scenario: for each declared turn and each fixture of a release
    it names, every call whose recorded request ends with a message of nothing but tool results - the request
    the framework appends the turn to."""
    gaps = []
    for entry in framework.states_unsent:
        if scenario not in entry["scenarios"]:
            continue
        for fixture, recorded in sorted(requests.items()):
            mode = fixture.split("/")[1]
            if mode not in entry["modes"][scenario]:
                continue
            for call, request in sorted(recorded["calls"].items()):
                last = request["messages"][-1] if request["messages"] else None
                if not (
                    last
                    and last["role"] == "user"
                    and last["parts"]
                    and all(
                        p["part"].get("type") == "tool_result" for p in last["parts"]
                    )
                ):
                    continue
                gap = {
                    "fact": "user_text",
                    "reason": "producer_states_unsent",
                    "detail": f"{entry['reason']} ({entry['source']})",
                    "subject": call,
                    "modes": [mode],
                    "text": entry["text"],
                }
                if gap not in gaps:
                    gaps.append(gap)
    return gaps


#: The renderings a framework may declare it hands the model instead of a tool's returned value.
RENDERINGS = frozenset({"mcp_call_tool_result_str"})
ENVELOPES = frozenset({"genkit_googleai_tool_output"})


def _result_envelope(entry: dict[str, Any]) -> dict[str, Any]:
    """One declared result envelope, refused unless its format is known and it states a source and scenarios."""
    if not (
        entry.get("format") in ENVELOPES
        and isinstance(entry.get("source"), str)
        and entry["source"]
        and isinstance(entry.get("scenarios"), list)
        and entry["scenarios"]
    ):
        raise ValueError(
            f"a result_envelopes entry needs a known format, a source and scenarios: {entry!r}"
        )
    return entry


def _enveloped(part: dict[str, Any]) -> tuple[str, Any] | None:
    """``(tool, output)`` of a tool-result part that holds exactly the Genkit envelope, else ``None``."""
    content = part.get("content")
    if not (
        part.get("type") == "tool_result"
        and isinstance(content, list)
        and len(content) == 1
        and content[0].get("type") == "json"
    ):
        return None
    value = content[0].get("value")
    if not (
        isinstance(value, dict)
        and set(value) == {"name", "content"}
        and isinstance(value["name"], str)
    ):
        return None
    return value["name"], value["content"]


def _same(a: Any, b: Any) -> bool:
    """JSON equality as the rubric's: numbers by value (`7` is `7.0`), a boolean never a number (`True == 1` in
    Python), objects by their members and lists in order."""
    if isinstance(a, bool) or isinstance(b, bool):
        return isinstance(a, bool) and isinstance(b, bool) and a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return a == b
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_same(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_same(x, y) for x, y in zip(a, b))
    return type(a) is type(b) and a == b


def _is_output(sent: Any, value: Any) -> bool:
    """Whether what the envelope carries is the script's output: the value itself, or its JSON text."""
    if _same(sent, value):
        return True
    if isinstance(sent, str):
        try:
            return _same(json.loads(sent), value)
        except ValueError:
            return False
    return False


def unwrap_result_envelopes(
    framework: Framework,
    scenario: str,
    document: dict[str, Any],
    requests: dict[str, Any],
) -> None:
    """In the scenarios a framework declares an envelope for, each request part holding exactly that envelope is
    the result fact it carries - the tool named in it whose script output is its content, the one such fact - and
    states the content the framework enveloped.

    Decided per fixture, and only where every occurrence agrees: each occurrence must hold the envelope and name
    the one fact; a request-only id (``rq-*``) is renamed to that fact only if every part holding it does, and a
    part already tied to a fact must be tied to that one. A part that is also exactly some result's whole value -
    a map output, which the plugin sends as it is - is that map and is left alone."""
    if not any(scenario in e["scenarios"] for e in framework.result_envelopes):
        return
    facts = [fact for fact in document["facts"] if fact["kind"] == "tool_result"]

    def answering(part: dict[str, Any]) -> str | None:
        enveloped = _enveloped(part)
        if enveloped is None:
            return None
        tool, output = enveloped
        whole = part["content"][0]["value"]
        if any(_same(fact["value"].get("value"), whole) for fact in facts):
            return None
        found = [
            fact["id"]
            for fact in facts
            if fact["value"].get("name") == tool
            and _is_output(output, fact["value"].get("value"))
        ]
        return found[0] if len(found) == 1 else None

    for recorded in requests.values():
        occurrences = [
            part
            for request in recorded["calls"].values()
            for message in request["messages"]
            for part in message["parts"]
        ]
        # Every lineage's verdict over all its occurrences in this fixture: one fact, or none at all.
        verdicts: dict[str, set[str | None]] = {}
        for occurrence in occurrences:
            lineage = (
                occurrence.get("new")
                or occurrence.get("new_fact")
                or occurrence.get("replay_of")
            )
            if isinstance(lineage, str) and _enveloped(occurrence["part"]) is not None:
                verdicts.setdefault(lineage, set()).add(answering(occurrence["part"]))
            elif isinstance(lineage, str):
                verdicts.setdefault(lineage, set()).add(None)
        agreed: dict[str, str] = {}
        for lineage, found in verdicts.items():
            if len(found) != 1 or None in found:
                continue
            (fact,) = found
            # A part already tied to a fact must be tied to the one its envelope names.
            if not lineage.startswith("rq-") and lineage != fact:
                continue
            agreed[lineage] = fact
        for occurrence in occurrences:
            lineage = (
                occurrence.get("new")
                or occurrence.get("new_fact")
                or occurrence.get("replay_of")
            )
            if lineage not in agreed:
                continue
            _, output = _enveloped(occurrence["part"])  # type: ignore[misc]
            occurrence["part"]["content"] = (
                [{"type": "text", "text": output}]
                if isinstance(output, str)
                else [{"type": "json", "value": output}]
            )
            if "new" in occurrence:
                del occurrence["new"]
                occurrence["new_fact"] = agreed[lineage]
            elif "replay_of" in occurrence:
                occurrence["replay_of"] = agreed[lineage]
        # A fact is introduced once: the first occurrence in the fixture's call order states it new, every later one
        # - of whichever lineage it was renamed from - replays it.
        established: set[str] = set()
        for occurrence in occurrences:
            if (fact := occurrence.get("new_fact")) is not None:
                if fact in established:
                    del occurrence["new_fact"]
                    occurrence["replay_of"] = fact
                established.add(fact)
            elif isinstance(occurrence.get("replay_of"), str):
                established.add(occurrence["replay_of"])


def _result_rendering(entry: dict[str, Any]) -> dict[str, Any]:
    """One declared result rendering, refused unless its format is known and it states a source and scenarios."""
    if not (
        entry.get("format") in RENDERINGS
        and isinstance(entry.get("source"), str)
        and entry["source"]
        and isinstance(entry.get("scenarios"), list)
        and entry["scenarios"]
    ):
        raise ValueError(
            f"a result_renderings entry needs a known format, a source and scenarios: {entry!r}"
        )
    return entry


def _renders_call_tool_result(text: str, value: Any, is_error: bool) -> bool:
    """Whether the text is the ``str()`` of an MCP ``CallToolResult`` holding exactly this value: pydantic prints
    its fields as ``name=repr`` separated by spaces, and the SDK's model has a one-text content list, the value
    as ``structured_content={'result': ...}`` and the error flag."""
    import ast
    import re

    shape = re.fullmatch(
        r"meta=(?P<meta>\{.*\}|None) content=\[TextContent\(type='text', text=(?P<text>'[^'\\]*'), "
        r"annotations=None, meta=None\)\] structured_content=(?P<structured>\{.*\}|None) "
        r"is_error=(?P<error>True|False)(?: result_type='[a-z_]+')?",
        text,
    )
    if shape is None or (shape["error"] == "True") != is_error:
        return False
    shown = ast.literal_eval(shape["text"])
    structured = ast.literal_eval(shape["structured"])
    if not (isinstance(structured, dict) and set(structured) == {"result"}):
        return False
    result = structured["result"]

    def number(x: Any) -> bool:
        return isinstance(x, (int, float)) and not isinstance(x, bool)

    if number(value):
        # Exact comparison: Python compares an int with a float by value, never through a lossy conversion, and
        # a string or a boolean is not the number the script returned.
        try:
            text_value = ast.literal_eval(shown)
        except (ValueError, SyntaxError):
            return False
        return (
            number(result)
            and result == value
            and number(text_value)
            and text_value == value
        )
    return result == value and shown == value


def rendered_results(
    framework: Framework,
    scenario: str,
    document: dict[str, Any],
    requests: dict[str, Any],
) -> None:
    """Each tool result of a scenario the framework declares it renders, as the text the model was sent: read from
    the recorded requests, which must all send one text, and refused unless that text is the declared rendering of
    the result's value as the script computed it."""
    entries = [e for e in framework.result_renderings if scenario in e["scenarios"]]
    for entry in entries:
        for fact in document["facts"]:
            if fact["kind"] != "tool_result":
                continue
            naming = [
                part["part"]
                for recorded in requests.values()
                for request in recorded["calls"].values()
                for message in request["messages"]
                for part in message["parts"]
                if fact["id"] in (part.get("new_fact"), part.get("replay_of"))
            ]
            if not naming:
                continue
            # Every occurrence, in every fixture: one sent in any other shape is not the declared rendering.
            sent = set()
            for part in naming:
                content = part.get("content") or []
                if not (
                    part.get("type") == "tool_result"
                    and len(content) == 1
                    and isinstance(content[0].get("text"), str)
                ):
                    raise ValueError(
                        f"{fact['id']} is sent as {part!r}, not as one text ({entry['source']})"
                    )
                sent.add(content[0]["text"])
            if len(sent) != 1:
                raise ValueError(f"{fact['id']} is sent as {len(sent)} different texts")
            (text,) = sent
            value = fact["value"]
            if not _renders_call_tool_result(
                text, value.get("value"), bool(value.get("is_error"))
            ):
                raise ValueError(
                    f"{fact['id']} is sent as {text!r}, which is not the {entry['format']} of "
                    f"{value.get('value')!r} ({entry['source']})"
                )
            value["value"] = text
