"""The scenarios every framework suite implements, and what each one proves.

Scenario names are shared across languages and frameworks so a fixture path says the same thing
everywhere: ``<producer>/<mode>/<scenario>``. A suite implements every core scenario its framework
can express; an optional scenario is implemented when the framework has the feature.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ScenarioSpec:
    name: str
    summary: str
    core: bool


CATALOG: dict[str, ScenarioSpec] = {
    spec.name: spec
    for spec in (
        ScenarioSpec("chat", "A system prompt, one question, one answer.", core=True),
        ScenarioSpec(
            "multi_turn",
            "Three questions in one conversation; each request re-sends the history.",
            core=True,
        ),
        ScenarioSpec(
            "tool_use",
            "Two tools, called more than once, with results the answer depends on.",
            core=True,
        ),
        ScenarioSpec(
            "session",
            "Two separate traces that belong to one session and one user.",
            core=True,
        ),
        ScenarioSpec(
            "error",
            "A tool that raises; the model sees the error and answers.",
            core=True,
        ),
        ScenarioSpec(
            "streaming", "A streamed answer that also calls a tool.", core=False
        ),
        ScenarioSpec(
            "structured_output", "An answer constrained to a JSON schema.", core=False
        ),
        ScenarioSpec("reasoning", "Adaptive thinking before the answer.", core=False),
        ScenarioSpec("files", "An image and a PDF in the user turn.", core=False),
        ScenarioSpec(
            "multi_agent",
            "Agents that hand work to each other: handoff, swarm, or graph.",
            core=False,
        ),
        ScenarioSpec("mcp_tools", "Tools served by an MCP server.", core=False),
        ScenarioSpec(
            "server_tools",
            "A tool the provider runs itself, such as web search, beside the answer.",
            core=False,
        ),
        ScenarioSpec(
            "trailing_tool",
            "A turn that ends on a tool result: the model calls a tool and the conversation stops there, "
            "so no later request re-sends what the tool returned.",
            core=False,
        ),
    )
}

#: Session and user identifiers are deterministic so a recapture produces comparable fixtures.
USER_ID = "example-user"


def session_id(framework: str, scenario: str) -> str:
    return f"{framework}-{scenario}"
