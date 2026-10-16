"""The truth's reading of tool results a framework sends inside an envelope of its own."""

from __future__ import annotations


def test_a_declared_result_envelope_is_the_result_fact_it_carries() -> None:
    from harness.truth.framework import Framework, unwrap_result_envelopes

    framework = Framework.of(
        {
            "result_envelopes": [
                {
                    "format": "genkit_googleai_tool_output",
                    "source": "s",
                    "scenarios": ["s1"],
                }
            ]
        }
    )
    facts = [
        {
            "id": "fact-1",
            "kind": "tool_result",
            "value": {"name": "w", "value": {"c": 1}},
        },
        {
            "id": "fact-2",
            "kind": "tool_result",
            "value": {"name": "w", "value": {"c": 2}},
        },
    ]
    envelope = lambda output: {  # noqa: E731
        "type": "tool_result",
        "id": None,
        "content": [{"type": "json", "value": {"name": "w", "content": output}}],
    }
    requests = {
        "f": {
            "calls": {
                "call-1": {
                    "messages": [
                        {
                            "role": "user",
                            "parts": [
                                {"part": envelope('{"c": 1}'), "new": "rq-001"},
                                {"part": envelope('{"c": 3}'), "new": "rq-002"},
                            ],
                        }
                    ]
                },
                "call-2": {
                    "messages": [
                        {
                            "role": "user",
                            "parts": [
                                {"part": envelope('{"c": 1}'), "replay_of": "rq-001"},
                            ],
                        }
                    ]
                },
            }
        }
    }
    unwrap_result_envelopes(framework, "s1", {"facts": facts}, requests)
    first, unknown = requests["f"]["calls"]["call-1"]["messages"][0]["parts"]
    (replay,) = requests["f"]["calls"]["call-2"]["messages"][0]["parts"]
    assert first["new_fact"] == "fact-1" and "new" not in first
    assert first["part"]["content"] == [{"type": "text", "text": '{"c": 1}'}]
    assert replay["replay_of"] == "fact-1"
    # An output no fact holds stays the request-only part it was.
    assert unknown["new"] == "rq-002"
    assert unknown["part"]["content"][0]["type"] == "json"
    # Outside the declared scenarios nothing changes.
    other = {
        "f": {
            "calls": {
                "c": {
                    "messages": [
                        {
                            "role": "user",
                            "parts": [{"part": envelope('{"c": 2}'), "new": "rq-9"}],
                        }
                    ]
                }
            }
        }
    }
    unwrap_result_envelopes(framework, "s2", {"facts": facts}, other)
    assert other["f"]["calls"]["c"]["messages"][0]["parts"][0]["new"] == "rq-9"


def test_a_result_envelope_is_unwrapped_only_where_every_occurrence_agrees() -> None:
    from harness.truth.framework import Framework, unwrap_result_envelopes

    framework = Framework.of(
        {
            "result_envelopes": [
                {
                    "format": "genkit_googleai_tool_output",
                    "source": "s",
                    "scenarios": ["s1"],
                }
            ]
        }
    )

    def envelope(output, name="w"):
        return {
            "type": "tool_result",
            "id": None,
            "content": [{"type": "json", "value": {"name": name, "content": output}}],
        }

    def run(facts, parts_by_fixture):
        requests = {
            fixture: {"calls": {"c": {"messages": [{"role": "user", "parts": parts}]}}}
            for fixture, parts in parts_by_fixture.items()
        }
        unwrap_result_envelopes(framework, "s1", {"facts": facts}, requests)
        return {f: r["calls"]["c"]["messages"][0]["parts"] for f, r in requests.items()}

    paris = {
        "id": "fact-p",
        "kind": "tool_result",
        "value": {"name": "w", "value": {"city": "Paris"}},
    }
    tokyo = {
        "id": "fact-t",
        "kind": "tool_result",
        "value": {"name": "w", "value": {"city": "Tokyo"}},
    }
    # Per fixture: the same request-only id naming Paris in one and Tokyo in the other is each its own.
    out = run(
        [paris, tokyo],
        {
            "native": [{"part": envelope('{"city": "Paris"}'), "new": "rq-001"}],
            "sdk": [{"part": envelope('{"city": "Tokyo"}'), "new": "rq-001"}],
        },
    )
    assert (
        out["native"][0]["new_fact"] == "fact-p"
        and out["sdk"][0]["new_fact"] == "fact-t"
    )
    # One lineage whose occurrences disagree is left alone.
    out = run(
        [paris, tokyo],
        {
            "f": [
                {"part": envelope('{"city": "Paris"}'), "new": "rq-001"},
                {"part": envelope('{"city": "Tokyo"}'), "replay_of": "rq-001"},
            ]
        },
    )
    assert out["f"][0]["new"] == "rq-001" and out["f"][1]["replay_of"] == "rq-001"
    # A map output the plugin sends as it is stays that map.
    mapped = {
        "id": "fact-m",
        "kind": "tool_result",
        "value": {"name": "w", "value": {"name": "w", "content": 7}},
    }
    seven = {"id": "fact-7", "kind": "tool_result", "value": {"name": "w", "value": 7}}
    out = run([mapped, seven], {"f": [{"part": envelope(7), "new": "rq-001"}]})
    assert out["f"][0]["new"] == "rq-001"
    # A part tied to a fact keeps only the fact its envelope names, unwrapped in every occurrence.
    out = run(
        [paris],
        {
            "f": [
                {"part": envelope('{"city": "Paris"}'), "new_fact": "fact-p"},
                {"part": envelope('{"city": "Paris"}'), "replay_of": "fact-p"},
            ]
        },
    )
    assert all(o["part"]["content"][0]["type"] == "text" for o in out["f"])
    # A boolean is not the number it equals in Python.
    flag = {
        "id": "fact-b",
        "kind": "tool_result",
        "value": {"name": "w", "value": {"c": 1}},
    }
    out = run([flag], {"f": [{"part": envelope({"c": True}), "new": "rq-001"}]})
    assert out["f"][0]["new"] == "rq-001"


def test_a_fact_two_lineages_carry_is_introduced_once() -> None:
    from harness.truth.framework import Framework, unwrap_result_envelopes

    framework = Framework.of(
        {
            "result_envelopes": [
                {
                    "format": "genkit_googleai_tool_output",
                    "source": "s",
                    "scenarios": ["s1"],
                }
            ]
        }
    )
    part = {
        "type": "tool_result",
        "id": None,
        "content": [{"type": "json", "value": {"name": "w", "content": "7"}}],
    }
    facts = [
        {"id": "fact-7", "kind": "tool_result", "value": {"name": "w", "value": 7}}
    ]
    requests = {
        "f": {
            "calls": {
                "call-1": {
                    "messages": [
                        {
                            "role": "user",
                            "parts": [{"part": dict(part), "new": "rq-001"}],
                        }
                    ]
                },
                "call-2": {
                    "messages": [
                        {
                            "role": "user",
                            "parts": [{"part": dict(part), "new": "rq-002"}],
                        }
                    ]
                },
            }
        }
    }
    unwrap_result_envelopes(framework, "s1", {"facts": facts}, requests)
    first = requests["f"]["calls"]["call-1"]["messages"][0]["parts"][0]
    second = requests["f"]["calls"]["call-2"]["messages"][0]["parts"][0]
    assert first.get("new_fact") == "fact-7"
    assert second.get("replay_of") == "fact-7" and "new_fact" not in second


def test_numbers_compare_by_value_so_a_map_output_stays_a_map() -> None:
    from harness.truth.framework import Framework, _same, unwrap_result_envelopes

    assert _same(7, 7.0) and not _same(True, 1) and not _same({"c": True}, {"c": 1})
    framework = Framework.of(
        {
            "result_envelopes": [
                {
                    "format": "genkit_googleai_tool_output",
                    "source": "s",
                    "scenarios": ["s1"],
                }
            ]
        }
    )
    facts = [
        {
            "id": "fact-m",
            "kind": "tool_result",
            "value": {"name": "w", "value": {"name": "w", "content": 7.0}},
        },
        {"id": "fact-7", "kind": "tool_result", "value": {"name": "w", "value": 7}},
    ]
    part = {
        "type": "tool_result",
        "id": None,
        "content": [{"type": "json", "value": {"name": "w", "content": 7}}],
    }
    requests = {
        "f": {
            "calls": {
                "c": {
                    "messages": [
                        {"role": "user", "parts": [{"part": part, "new": "rq-001"}]}
                    ]
                }
            }
        }
    }
    unwrap_result_envelopes(framework, "s1", {"facts": facts}, requests)
    assert requests["f"]["calls"]["c"]["messages"][0]["parts"][0]["new"] == "rq-001"
