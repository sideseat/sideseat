"""Truth assembly lays decoded calls out as the scenario's conversation, and committed truths are current."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from harness import content
from harness.fakes import script
from harness.truth import derive, sources, wire
from harness.truth.wire import ModelCall


# --- Assembling a conversation ------------------------------------------------------------------


def model_call(
    *parts: dict[str, Any], finish: str = "stop", ok: bool = True
) -> ModelCall:
    return ModelCall(
        api="bedrock.converse",
        status=200 if ok else 500,
        parts=list(parts),
        finish=finish,
        error=None if ok else "server error",
    )


def facts_by_kind(builder: derive.Builder, kind: str) -> list[dict[str, Any]]:
    return [fact for fact in builder.facts if fact["kind"] == kind]


def test_tool_results_are_recomputed_and_paired_by_call_id() -> None:
    calls = [
        model_call(
            wire.tool_call_part(
                "a", "travel-get_weather", {"city": "Tokyo", "days": 1}
            ),
            wire.tool_call_part(
                "b", "mcp__travel__get_precipitation", {"city": "Paris"}
            ),
            finish="tool_use",
        ),
        model_call(wire.text_part("Pack an umbrella for Tokyo.")),
    ]
    builder = derive.assemble("p", "tool_use", calls)
    results = facts_by_kind(builder, "tool_result")
    assert [r["value"]["call_id"] for r in results] == ["a", "b"]
    assert results[0]["value"]["value"] == content.get_weather("Tokyo", 1)
    assert results[1]["value"]["value"] == "10% chance of rain in Paris tomorrow."
    pairs = {(e["from"], e["to"]) for e in builder.edges if e["kind"] == "result_of"}
    calls_by_id = {
        f["value"]["id"]: f["id"] for f in facts_by_kind(builder, "tool_call")
    }
    assert pairs == {
        (results[0]["id"], calls_by_id["a"]),
        (results[1]["id"], calls_by_id["b"]),
    }
    (conversation,) = builder.conversations
    assert conversation["final_answers"] == [facts_by_kind(builder, "text")[0]["id"]]


def test_a_raised_tool_error_is_an_error_result_matched_by_its_message() -> None:
    call = wire.tool_call_part(
        "e",
        "book_flight",
        {"origin": "London", "destination": "Oslo", "date": "2026-11-14"},
    )
    builder = derive.assemble(
        "p",
        "error",
        [model_call(call, finish="tool_use"), model_call(wire.text_part("Sorry."))],
    )
    (result,) = facts_by_kind(builder, "tool_result")
    assert result["value"]["is_error"] is True
    assert "booking system is offline" in result["value"]["value"]
    assert result["require"]["match"] == "error_message"


def test_sessions_hold_one_conversation_per_prompt_and_multi_turn_one_in_all() -> None:
    answers = [model_call(wire.text_part("One.")), model_call(wire.text_part("Two."))]
    sessions = derive.assemble("p", "session", answers)
    assert len(sessions.conversations) == 2
    assert [f["value"]["text"] for f in facts_by_kind(sessions, "user_text")] == list(
        content.SESSION
    )
    turns = derive.assemble(
        "p", "multi_turn", answers + [model_call(wire.text_part("Three."))]
    )
    (conversation,) = turns.conversations
    kinds = [
        turns.facts[int(i.split("-")[1]) - 1]["kind"] for i in conversation["sequence"]
    ]
    assert kinds == ["user_text", "text"] * 3


def test_a_failed_attempt_has_no_output_and_its_retry_is_the_next_attempt() -> None:
    builder = derive.assemble(
        "p", "chat", [model_call(ok=False), model_call(wire.text_part("Kyoto."))]
    )
    first, second = builder.calls
    assert (first["outcome"], first["outputs"]) == ("failed", [])
    assert (second["outcome"], second["attempt"]) == ("success", 2)
    assert any(gap["reason"] == "no_output_obligation" for gap in builder.gaps)


def test_adjacent_text_blocks_join_into_one_fact_keeping_their_segments() -> None:
    call = model_call(
        wire.text_part("It says "), wire.text_part('"hi"'), wire.text_part(".")
    )
    builder = derive.assemble("p", "files", [call])
    (text,) = facts_by_kind(builder, "text")
    assert text["value"] == {
        "text": 'It says "hi".',
        "segments": ["It says ", '"hi"', "."],
    }
    assert len(facts_by_kind(builder, "user_media")) == 2


def test_an_unknown_tool_has_no_result_but_a_gap() -> None:
    call = model_call(
        wire.tool_call_part("h", "transfer_to_writer", {}), finish="tool_use"
    )
    builder = derive.assemble(
        "p", "multi_agent", [call, model_call(wire.text_part("Packed."))]
    )
    assert facts_by_kind(builder, "tool_result") == []
    assert {"tool_not_deterministic", "multi_agent_routing"} <= {
        g["reason"] for g in builder.gaps
    }


def test_reasoning_without_visible_text_is_not_required() -> None:
    builder = derive.assemble(
        "p",
        "chat",
        [model_call(wire.reasoning_part("", signed=True), wire.text_part("Hi."))],
    )
    reasoning = facts_by_kind(builder, "reasoning")[0]
    assert reasoning["require"] is None
    assert any(gap["reason"] == "reasoning_text_omitted" for gap in builder.gaps)


def test_calculator_evaluates_arithmetic_only() -> None:
    assert derive.calculate("(17 * 23) + 4") == 395
    with pytest.raises(ValueError):
        derive.calculate("__import__('os')")


# --- Sources ------------------------------------------------------------------------------------


def test_fake_model_truth_keeps_exact_call_ids_but_not_answers_built_from_results() -> (
    None
):
    document = sources.fake(sources.Target("autogen", "tool_use"), "fake-openai", [])
    calls = [f for f in document["facts"] if f["kind"] == "tool_call"]
    question = content.TOOL_USE
    assert calls[0]["value"]["id"] == script._call_id("get_weather", question, 0, 0)
    texts = [f for f in document["facts"] if f["kind"] == "text"]
    assert texts and all(f["require"] is None for f in texts)


def test_conformance_truth_describes_the_canonical_conversation() -> None:
    document = sources.conformance("rust")
    assert [c["usage"]["input"] for c in document["calls"]] == [12, 24]
    (result,) = [f for f in document["facts"] if f["kind"] == "tool_result"]
    assert result["value"]["value"] == {"temperature_c": 18, "condition": "sunny"}


def test_an_echoed_account_name_is_anonymised_whoever_regenerates() -> None:
    assert (
        sources.anonymise("/Users/someone/x and /home/a.b/y")
        == "/Users/sideseat/x and /home/sideseat/y"
    )
    # Cut or padded to the name's length, as the fixture capture does to its protobuf payloads.
    assert sources.anonymise("directory_users_jdoe_aa4c") == "directory_users_side_aa4c"
    assert (
        sources.anonymise("directory_users_lovelace_aa4c")
        == "directory_users_sideseat_aa4c"
    )


def committed() -> list[Path]:
    return sorted(sources.TRUTH.glob("*/*.json"))


def test_every_committed_truth_is_what_its_sources_produce() -> None:
    stale = []
    for path in committed():
        target = sources.Target(path.parent.name, path.stem)
        if sources.render(sources.build(target)) != path.read_text():
            stale.append(f"{target.producer}/{target.scenario}")
    assert not stale, f"regenerate with `python -m harness truth --all`: {stale}"


def test_every_captured_fixture_has_a_truth_or_a_stated_reason() -> None:
    from harness.truth import cli

    covered = {(p.parent.name, p.stem) for p in committed()}
    unexplained = [
        label
        for label, reason in cli._fixtures_without_truth(covered)
        if reason == "no truth derived"
    ]
    assert unexplained == ["autogen/native/multi_agent", "autogen/sdk/multi_agent"]


def test_a_terminal_answer_tool_ends_the_turn_without_a_result() -> None:
    # smolagents answers through `final_answer`; the next prompt starts a new turn after it.
    answers = [
        model_call(
            wire.tool_call_part(f"f{index}", "final_answer", {"answer": f"A{index}."}),
            finish="tool_use",
        )
        for index in range(3)
    ]
    builder = derive.assemble("p", "multi_turn", answers)
    (conversation,) = builder.conversations
    assert len(conversation["final_answers"]) == 3
    assert facts_by_kind(builder, "tool_result") == []
    assert not any(gap["fact"] in ("user_text", "tool_result") for gap in builder.gaps)


def test_committed_truths_are_internally_consistent() -> None:
    for path in committed():
        document = json.loads(path.read_text())
        label = f"{path.parent.name}/{path.stem}"
        facts = {fact["id"]: fact for fact in document["facts"]}
        calls = {call["id"]: call for call in document["calls"]}
        assert len(facts) == len(document["facts"]) and len(calls) == len(
            document["calls"]
        ), label
        sequenced = [i for c in document["conversations"] for i in c["sequence"]]
        assert sorted(sequenced) == sorted(facts), (
            f"{label}: every fact sequenced exactly once"
        )
        for call in calls.values():
            for output in call["outputs"]:
                assert facts[output]["call"] == call["id"], label
                assert facts[output]["conversation"] == call["conversation"], label
        for edge in document["edges"]:
            ends = (
                (edge["from"], edge["to"])
                if "from" in edge
                else (edge["before"], edge["after"])
            )
            assert all(end in facts or end in calls for end in ends), f"{label}: {edge}"
        subjects = {gap.get("subject") for gap in document["gaps"]}
        for fact in facts.values():
            if fact["require"] is None:
                assert fact["id"] in subjects, (
                    f"{label}: {fact['id']} is unasserted without a gap"
                )
        for gap in document["gaps"]:
            if "subject" in gap:
                assert gap["subject"] in facts or gap["subject"] in calls, (
                    f"{label}: {gap}"
                )
        for fixture in document["fixtures"]:
            assert (sources.FIXTURES / fixture).is_dir(), f"{label}: {fixture}"
        assert document["model_calls"] == sum(
            c["outcome"] == "success" for c in calls.values()
        )
