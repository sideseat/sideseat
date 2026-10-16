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
