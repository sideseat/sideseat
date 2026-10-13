"""What a framework's telemetry leaves out, declared by its suite and derived into absence gaps.

Each entry of a suite's ``truth.unexported`` table becomes a gap the Rust rubric proves against every
fixture (``message_truth::absence``); the vocabulary and what each item withdraws or moves is documented on
``derive.Framework.unexported``.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from harness.truth.derive import Builder, Framework


def withheld(part: dict[str, Any]) -> bool:
    """Reasoning the model signed and withheld the text of: an empty text, not an unknown one."""
    return part["text"] == "" and bool(part["signed"]) and not part["redacted"]


#: Call metadata a producer may state wrongly with the right value in no payload.
METADATA = ("model", "response_model", "response_id", "finish")
#: Usage counts a producer may state wrongly with the right count in no payload: each declaration key and the
#: count it names, as the call's usage and the gap's fact (``usage.<count>``) spell it.
USAGE = {
    "usage_input": "input",
    "usage_output": "output",
    "usage_cache_read": "cache_read",
    "usage_cache_write": "cache_write",
    "usage_reasoning": "reasoning",
}
UNEXPORTED = frozenset(
    {
        "tool_call_ids",
        "tool_calling_rounds",
        "reasoning_kind",
        "withheld_reasoning",
        "visible_reasoning",
        "reasoning_signature",
        "reasoning_output",
        "visible_reasoning_output",
        "reasoning_span_signature",
        "tool_calling_round_output",
        "parallel_tool_results",
        "parallel_call_order",
        "structured_tool_results",
        "tool_round_call_spans",
        "tool_round_traces",
        "media",
        *METADATA,
        *USAGE,
    }
)


def apply(builder: Builder, framework: Framework) -> None:
    """The suite's declared telemetry omissions as absence gaps, which the rubric proves or refuses."""
    unknown = set(framework.unexported) - UNEXPORTED
    if unknown:
        raise ValueError(f"unknown unexported content {sorted(unknown)}")

    def declared(content: str) -> tuple[str, list[str]] | None:
        """The reason and the capture modes it holds for (all when none are named)."""
        entry = framework.unexported.get(content)
        if isinstance(entry, dict):
            if "scenarios" in entry and builder.scenario not in entry["scenarios"]:
                return None
            modes = entry.get("modes", {})
            if isinstance(modes, dict):
                modes = modes.get(builder.scenario, [])
            return entry["reason"], list(modes)
        return (entry, []) if entry else None

    def gap(
        fact: str, reason: str, detail: tuple[str, list[str]], subject: str
    ) -> None:
        entry: dict[str, Any] = {
            "fact": fact,
            "reason": reason,
            "detail": detail[0],
            "subject": subject,
        }
        if detail[1]:
            entry["modes"] = detail[1]
        if entry not in builder.gaps:
            builder.gaps.append(entry)

    facts = {fact["id"]: fact for fact in builder.facts}
    for field_name in METADATA:
        if not (detail := declared(field_name)):
            continue
        entry = framework.unexported[field_name]
        limited = entry.get("values") if isinstance(entry, dict) else None
        for record in builder.calls:
            value = record.get(field_name)
            if record["outcome"] != "success" or value is None:
                continue
            if limited is None or value in limited:
                gap(field_name, "metadata_not_exported", detail, record["id"])
    for key, count in USAGE.items():
        if not (detail := declared(key)):
            continue
        for record in builder.calls:
            if (
                record["outcome"] == "success"
                and (record.get("usage") or {}).get(count) is not None
            ):
                gap(f"usage.{count}", "metadata_not_exported", detail, record["id"])
    if detail := declared("reasoning_kind"):
        for fact in builder.facts:
            if (
                fact["kind"] == "reasoning"
                and fact["value"].get("text")
                and fact["require"] is not None
            ):
                gap("reasoning", "kind_not_exported", detail, fact["id"])
    if detail := declared("withheld_reasoning"):
        for fact in builder.facts:
            if fact["kind"] == "reasoning" and withheld(fact["value"]):
                fact["require"] = None
                gap("reasoning", "not_exported", detail, fact["id"])
    if detail := declared("visible_reasoning"):
        # Reasoning the model showed, which the producer's telemetry drops: no payload holds its text, which the
        # goldens prove per capture before the fact is withdrawn. Declared for some captures only, the fact stays
        # asserted in the document and each of those captures withdraws it (`Truth::for_fixture`).
        for fact in builder.facts:
            if (
                fact["kind"] == "reasoning"
                and fact["value"].get("text")
                and not withheld(fact["value"])
                and fact["require"] is not None
            ):
                if not detail[1]:
                    fact["require"] = None
                gap("reasoning", "not_exported", detail, fact["id"])
    if detail := declared("reasoning_output"):
        # Only a call a later call of its conversation follows: the claim is that the next request's
        # history re-sends what the call's own output left out, and nothing re-sends the last call's.
        followed = {
            record["id"]
            for record in builder.calls
            if any(
                later["conversation"] == record["conversation"]
                and builder.calls.index(later) > builder.calls.index(record)
                for later in builder.calls
            )
        }
        # The last call's has no next request to re-send it, so no payload holds it at all.
        unsent = (
            f"{detail[0]}; the last turn's is re-sent by no request, so no payload holds it",
            detail[1],
        )
        for fact in builder.facts:
            if not (
                fact["kind"] == "reasoning"
                and withheld(fact["value"])
                and fact["require"] is not None
            ):
                continue
            if fact.get("call") in followed:
                gap("reasoning", "output_not_exported", detail, fact["id"])
            else:
                fact["require"] = None
                gap("reasoning", "not_exported", unsent, fact["id"])
    if detail := declared("visible_reasoning_output"):
        # Reasoning the model showed, which the span of the call that produced it leaves out while another
        # carrier of the capture holds it: still owed by the conversation views, off that span. The rubric proves
        # per capture that the producing span carries none of it.
        for fact in builder.facts:
            if (
                fact["kind"] == "reasoning"
                and fact["value"].get("text")
                and not withheld(fact["value"])
                and fact["require"] is not None
            ):
                gap("reasoning", "output_not_exported", detail, fact["id"])
    if detail := declared("reasoning_span_signature"):
        for fact in builder.facts:
            if (
                fact["kind"] == "reasoning"
                and withheld(fact["value"])
                and fact["require"] is not None
            ):
                gap("reasoning", "span_signature_not_exported", detail, fact["id"])
    if detail := declared("reasoning_signature"):
        for fact in builder.facts:
            if (
                fact["kind"] == "reasoning"
                and withheld(fact["value"])
                and fact["require"] is not None
            ):
                gap("reasoning", "signature_not_exported", detail, fact["id"])
    if detail := declared("tool_calling_rounds"):
        for record in builder.calls:
            outputs = [facts[output] for output in record["outputs"]]
            if outputs and all(fact["kind"] == "tool_call" for fact in outputs):
                gap("response", "call_not_exported", detail, record["id"])
    if detail := declared("tool_calling_round_output"):
        for record in builder.calls:
            outputs = [facts[output] for output in record["outputs"]]
            if outputs and all(fact["kind"] == "tool_call" for fact in outputs):
                for fact in outputs:
                    if fact["require"] is not None:
                        gap("tool_call", "output_not_exported", detail, fact["id"])
    if detail := declared("parallel_tool_results"):
        # The first call's result is exported, so it stays owed; the absence search proves each later one
        # against every payload of the capture.
        for record in builder.calls:
            calls = [
                facts[output]["value"].get("id")
                for output in record["outputs"]
                if facts[output]["kind"] == "tool_call"
            ]
            later = {id for id in calls[1:] if isinstance(id, str)}
            for fact in builder.facts:
                if (
                    fact["kind"] == "tool_result"
                    and fact["value"].get("call_id") in later
                    and fact["require"] is not None
                ):
                    fact["require"] = None
                    gap("tool_result", "not_exported", detail, fact["id"])
    if detail := declared("parallel_call_order"):
        # The order of one response's parallel calls, where no payload holds two of them together: the rubric
        # proves per capture that no carrier holds any two of the response's calls, and only then stops
        # asserting their relative order - never their presence, nor their place against the rest.
        for record in builder.calls:
            calls = [
                output
                for output in record["outputs"]
                if facts[output]["kind"] == "tool_call"
            ]
            if len(calls) >= 2:
                gap("response", "call_order_not_exported", detail, record["id"])
    if detail := declared("tool_round_call_spans"):
        # The call that answers a tool round continues the run the previous call's span records; the rubric
        # proves per capture that no model-call span exists and that span shows this call's output.
        for record in _tool_round_answers(builder.calls, facts):
            gap("response", "call_span_not_exported", detail, record["id"])
    if detail := declared("tool_round_traces"):
        # The rubric proves per capture that the call's span has no parent and sits in another trace than
        # the previous call's.
        for record in _tool_round_answers(builder.calls, facts):
            gap("response", "trace_not_propagated", detail, record["id"])
    if detail := declared("structured_tool_results"):
        # A text result is exported and stays owed; one the tool returned as data is proven absent per fact.
        # Declared for some releases only, the fact stays asserted and the rubric withdraws it in those
        # releases' captures alone.
        for fact in builder.facts:
            if (
                fact["kind"] == "tool_result"
                and not isinstance(fact["value"].get("value"), str)
                and fact["require"] is not None
            ):
                if not detail[1]:
                    fact["require"] = None
                gap("tool_result", "not_exported", detail, fact["id"])
    if detail := declared("media"):
        entry = framework.unexported["media"]
        modalities = entry.get("modalities") if isinstance(entry, dict) else None
        for fact in builder.facts:
            if (
                fact["kind"] == "user_media"
                and fact["require"] is not None
                and (modalities is None or fact["value"].get("modality") in modalities)
            ):
                fact["require"] = None
                gap("user_media", "not_exported", detail, fact["id"])
    if detail := declared("tool_call_ids"):
        for fact in builder.facts:
            if fact["kind"] == "tool_call" and isinstance(fact["value"].get("id"), str):
                gap("tool_call", "id_not_exported", detail, fact["id"])


def _tool_round_answers(
    calls: list[dict[str, Any]], facts: dict[str, dict[str, Any]]
) -> list[dict[str, Any]]:
    """The successful calls whose conversation's previous successful call asked for a tool."""
    previous: dict[str, dict[str, Any]] = {}
    answers = []
    for record in calls:
        if record["outcome"] != "success":
            continue
        before = previous.get(record["conversation"])
        if before is not None and any(
            facts[output]["kind"] == "tool_call" for output in before["outputs"]
        ):
            answers.append(record)
        previous[record["conversation"]] = record
    return answers
