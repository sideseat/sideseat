"""Texts a framework writes into the conversation itself (``framework_authored``): replies kept from the model and
structured results returned from a call's arguments."""

from __future__ import annotations

from typing import Any

import pytest

from harness.truth import derive, wire
from harness.truth.framework import Framework
from harness.truth.wire import ModelCall


def model_call(*parts: dict[str, Any], finish: str = "stop") -> ModelCall:
    return ModelCall(
        api="bedrock.converse", status=200, parts=list(parts), finish=finish, error=None
    )


def test_a_framework_reply_kept_from_the_model_is_marked_so() -> None:
    entry = {
        "text": "Waiting for the tool.",
        "source": "framework.py:1",
        "scenarios": ["tool_use"],
        "after": 1,
    }
    calls = [
        model_call(
            wire.tool_call_part("a", "get_weather", {"city": "Tokyo", "days": 1}),
            finish="tool_use",
        ),
        model_call(wire.text_part("Pack an umbrella.")),
    ]

    def authored(table_entry: dict[str, object]) -> dict[str, object]:
        options = derive.Options(
            framework=Framework.of({"framework_authored": [table_entry]})
        )
        builder = derive.assemble("p", "tool_use", calls, options=options)
        (fact,) = [f for f in builder.facts if f["evidence"] == "framework"]
        return fact

    assert "kept_from_model" not in authored(entry)
    assert "kept_from_model" not in authored({**entry, "reaches_model": True})
    assert authored({**entry, "reaches_model": False})["kept_from_model"] is True
    with pytest.raises(ValueError, match="needs a text, a source and scenarios"):
        Framework.of({"framework_authored": [{**entry, "reaches_model": "no"}]})


def test_a_returned_structured_result_is_the_call_s_arguments_as_compact_json() -> None:
    entry = {
        "returns_arguments_of": "TripPlan",
        "source": "framework.py:1",
        "scenarios": ["structured_output"],
        "reaches_model": False,
    }
    options = derive.Options(framework=Framework.of({"framework_authored": [entry]}))
    arguments = {"city": "Wien", "days": ["Café"], "budget_eur": 350}
    call = model_call(
        wire.tool_call_part("a", "TripPlan", arguments), finish="tool_use"
    )
    builder = derive.assemble("p", "structured_output", [call], options=options)
    (result,) = [f for f in builder.facts if f["evidence"] == "framework"]
    assert result["value"] == {
        "text": '{"city":"Wien","days":["Café"],"budget_eur":350}'
    }
    assert result["require"]["match"] == "json" and result["kept_from_model"] is True
    assert builder.conversations[0]["sequence"][-1] == result["id"]
    twice = model_call(
        wire.tool_call_part("a", "TripPlan", arguments),
        wire.tool_call_part("b", "TripPlan", arguments),
        finish="tool_use",
    )
    with pytest.raises(ValueError, match="one result is returned"):
        derive.assemble("p", "structured_output", [twice], options=options)
    with pytest.raises(ValueError, match="states no text and no place"):
        Framework.of({"framework_authored": [{**entry, "text": "x"}]})


def test_a_record_answer_is_the_model_s_json_as_the_java_record_prints_it() -> None:
    entry = {
        "renders_answer_as_record": {
            "name": "TripPlan",
            "components": ["city", "days", "budget_eur"],
        },
        "source": "Main.java:1",
        "scenarios": ["structured_output"],
        "reaches_model": False,
    }
    options = derive.Options(framework=Framework.of({"framework_authored": [entry]}))

    def rendered(answer: str) -> str:
        builder = derive.assemble(
            "p",
            "structured_output",
            [model_call(wire.text_part(answer))],
            options=options,
        )
        (fact,) = [f for f in builder.facts if f["evidence"] == "framework"]
        assert fact["kept_from_model"] is True
        assert builder.conversations[0]["sequence"][-1] == fact["id"]
        return fact["value"]["text"]

    # In the record's component order, whatever the JSON's; a string as itself, a list as Java prints one.
    assert (
        rendered(
            '{"budget_eur": 350, "days": ["Day 1: Café", "Day 2"], "city": "Wien"}'
        )
        == "TripPlan[city=Wien, days=[Day 1: Café, Day 2], budget_eur=350]"
    )
    assert rendered('{"city": "Wien", "days": [], "budget_eur": true}') == (
        "TripPlan[city=Wien, days=[], budget_eur=true]"
    )
    # A value Java would print some other way refuses the declaration rather than guessing.
    with pytest.raises(ValueError, match="no record printing"):
        rendered('{"city": "Wien", "days": [], "budget_eur": 350.5}')
    with pytest.raises(ValueError, match="no record printing"):
        rendered('{"city": {"name": "Wien"}, "days": [], "budget_eur": 1}')
    # An answer without exactly the record's components is no answer of that record.
    with pytest.raises(ValueError, match="no JSON answer with the components"):
        rendered('{"city": "Wien", "days": []}')
    with pytest.raises(ValueError, match="names its record and its components"):
        Framework.of(
            {
                "framework_authored": [
                    {**entry, "renders_answer_as_record": {"name": "TripPlan"}}
                ]
            }
        )
