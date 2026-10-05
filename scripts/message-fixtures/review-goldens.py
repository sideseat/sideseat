#!/usr/bin/env python3
"""Print the recorded message expectations in a readable form, for review.

A golden is only worth having if someone read it. `git diff` on the JSON is unreviewable at
this size, so this renders the parts that matter — per-view message count, role sequence and
content — and flags patterns that usually mean a parsing defect.

Usage:
    scripts/message-fixtures/review-goldens.py                    # every fixture, summary only
    scripts/message-fixtures/review-goldens.py strands            # one suite, full detail
    scripts/message-fixtures/review-goldens.py strands/tool_use   # one sample, full detail
    scripts/message-fixtures/review-goldens.py --suspicious       # only fixtures with warnings
"""

from __future__ import annotations

import json
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

# Three levels up: message-fixtures/ -> scripts/ -> the repository root.
ROOT = Path(__file__).resolve().parents[2] / "server/tests/fixtures/messages"


@dataclass(frozen=True)
class Warning:
    code: str
    message: str


# Deliberate source semantics that resemble parser defects to the generic heuristics below.
# Counts are exact across trace and session views: changing either the telemetry or the parser
# makes the classification stale and returns the fixture to the unresolved review queue.
INTENTIONAL_WARNINGS: dict[str, dict[str, tuple[int, str]]] = {
    "haystack/native/files": {
        "json_block": (
            2,
            "Haystack's tracer writes a placeholder in place of file bytes, so the part is unreadable",
        ),
    },
    "haystack/sdk/files": {
        "json_block": (
            2,
            "Haystack's tracer writes a placeholder in place of file bytes, so the part is unreadable",
        ),
    },
    "ag2/native/structured_output": {
        "json_block": (
            2,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "ag2/sdk/structured_output": {
        "json_block": (
            2,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "langchain/native/structured_output": {
        "unbalanced_tools": (2, "TripPlan is a terminal schema pseudo-tool"),
    },
    "langchain/sdk/structured_output": {
        "unbalanced_tools": (2, "TripPlan is a terminal schema pseudo-tool"),
    },
    "_synthetic/text_split_by_parallel_calls": {
        "unbalanced_tools": (
            1,
            "synthetic response intentionally contains calls but no executions",
        ),
    },
    "adk/structured_output": {
        "raw_json_text": (
            2,
            "provider returns schema-constrained JSON as assistant text",
        ),
    },
    "adk-native/structured_output": {
        "raw_json_text": (
            2,
            "provider returns schema-constrained JSON as assistant text",
        ),
    },
    "adk-sdk/structured_output": {
        "raw_json_text": (
            2,
            "provider returns schema-constrained JSON as assistant text",
        ),
    },
    "adk-native/swarm": {
        "unbalanced_tools": (
            2,
            "ADK represents transfer completion as quoted handoff context in the next agent",
        ),
    },
    "adk-sdk/swarm": {
        "unbalanced_tools": (
            2,
            "ADK represents transfer completion as quoted handoff context in the next agent",
        ),
    },
    "agent-framework/native/structured_output": {
        "raw_json_text": (
            2,
            "the schema is prompted, so the model answers with JSON as text",
        ),
    },
    "agent-framework/sdk/structured_output": {
        "raw_json_text": (
            2,
            "the schema is prompted, so the model answers with JSON as text",
        ),
    },
    "agno/native/structured_output": {
        "json_block": (
            2,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "agno/sdk/structured_output": {
        "json_block": (
            2,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "bedrock/sdk/structured_output": {
        "unbalanced_tools": (2, "TripPlan is a terminal schema pseudo-tool"),
    },
    "claude-agent-sdk/structured_output": {
        "unbalanced_tools": (2, "StructuredOutput is a terminal schema pseudo-tool"),
    },
    "claude-agent-sdk-js/structured-output": {
        "unbalanced_tools": (2, "StructuredOutput is a terminal schema pseudo-tool"),
    },
    "crewai/structured_output": {
        "json_block": (
            2,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    **{
        f"{producer}/{mode}/{scenario}": warnings
        for producer in ("google-genai", "vertex-ai")
        for mode in ("native", "sdk")
        for scenario, warnings in {
            "structured_output": {
                "raw_json_text": (
                    2,
                    "provider returns schema-constrained JSON as assistant text",
                ),
            },
            "error": {
                "unbalanced_tools": (
                    2,
                    "the instrumentation records a failed tool only as error.type and the span "
                    "status, so the failure is an error message rather than a tool result",
                ),
            },
        }.items()
    },
    **{
        f"claude-agent-sdk/{mode}/{scenario}": warnings
        for mode in ("native", "sdk")
        for scenario, warnings in {
            "multi_agent": {
                "unbalanced_tools": (
                    2,
                    "the Claude Code CLI answers an Agent call through SubagentHandback, not a result",
                ),
            },
            "structured_output": {
                "unbalanced_tools": (
                    2,
                    "StructuredOutput is a terminal schema pseudo-tool",
                ),
            },
        }.items()
    },
    **{
        f"openinference/{mode}/tool_use": {
            "unbalanced_tools": (
                2,
                "the Bedrock instrumentor keeps only the last of a turn's parallel tool results",
            ),
        }
        for mode in ("native", "sdk")
    },
    **{
        f"logfire/{mode}/structured_output": {
            "raw_json_text": (
                2,
                "the Responses API returns schema-constrained JSON as assistant text",
            ),
        }
        for mode in ("native", "sdk")
    },
    **{
        f"openai/{mode}/structured_output": {
            "raw_json_text": (
                2,
                "Chat Completions returns schema-constrained JSON as assistant text",
            ),
        }
        for mode in ("native", "sdk")
    },
    **{
        f"azure-openai/{mode}/structured_output": {
            "json_block": (
                2,
                "structured assistant output is canonically represented as JSON",
            ),
        }
        for mode in ("native", "sdk")
    },
    "anthropic/sdk/structured_output": {
        "unbalanced_tools": (2, "trip_plan is a terminal schema pseudo-tool"),
    },
    "langgraph/native/structured_output": {
        "unbalanced_tools": (2, "TripPlan is a terminal schema pseudo-tool"),
    },
    "langgraph/sdk/structured_output": {
        "unbalanced_tools": (2, "TripPlan is a terminal schema pseudo-tool"),
    },
    "openai-agents/structured_output": {
        "json_block": (
            2,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "smolagents/native/chat": {
        "unbalanced_tools": (
            2,
            "the model wrote final_answer as text, which Smolagents parsed; only the result is a block",
        ),
    },
    "smolagents/native/error": {
        "unbalanced_tools": (
            2,
            "the two failed book_flight calls are reported as span errors, not tool results",
        ),
    },
    "smolagents/native/multi_agent": {
        "unbalanced_tools": (
            2,
            "the managed researcher returns through its own final_answer; its call has no result",
        ),
    },
    "smolagents/native/multi_turn": {
        "unbalanced_tools": (
            2,
            "one final_answer was written as text, which Smolagents parsed; only the result is a block",
        ),
    },
    "smolagents/sdk/chat": {
        "unbalanced_tools": (
            2,
            "the model wrote final_answer as text, which Smolagents parsed; only the result is a block",
        ),
    },
    "smolagents/sdk/error": {
        "unbalanced_tools": (
            2,
            "the two failed book_flight calls are reported as span errors, not tool results",
        ),
    },
    "smolagents/sdk/multi_agent": {
        "unbalanced_tools": (
            2,
            "the managed researcher returns through its own final_answer; its call has no result",
        ),
    },
    "smolagents/sdk/multi_turn": {
        "unbalanced_tools": (
            2,
            "one final_answer was written as text, which Smolagents parsed; only the result is a block",
        ),
    },
    "strands/structured_output": {
        "json_block": (
            1,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "strands-native/structured_output": {
        "json_block": (
            1,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "strands-sdk/structured_output": {
        "json_block": (
            1,
            "structured assistant output is canonically represented as JSON",
        ),
    },
    "vercel-ai-js/structured-output": {
        "json_block": (
            1,
            "structured assistant output is canonically represented as JSON",
        ),
    },
}


def load() -> list[tuple[str, dict]]:
    out = []
    for path in sorted(ROOT.rglob("expected.json")):
        label = str(path.parent.relative_to(ROOT))
        out.append((label, json.loads(path.read_text())))
    return out


def warnings_for(label: str, g: dict) -> list[Warning]:
    """Patterns that usually mean a parsing defect. Heuristics, not invariants: the test
    holds the hard guarantees, this is a reading aid that says where to look."""
    warns = []
    # Only meaningful when the fixture HAS sessions: many samples never set a session id, and
    # "no sessions" is not "empty sessions".
    if g["session_views"] and not any(
        v["message_count"] for v in g["session_views"].values()
    ):
        warns.append(
            Warning(
                "empty_sessions",
                "the fixture has sessions but every session view is empty",
            )
        )

    views = [(f"session {k}", v) for k, v in g["session_views"].items()]
    views += [(f"trace {k}", v) for k, v in g["trace_views"].items()]
    for name, view in views:
        roles = view["role_sequence"]
        if not roles:
            # A trace with no message-bearing spans is normal: instrumentation such as
            # botocore creates auxiliary traces that carry no conversation at all, and the
            # message query excludes rows with no messages. The harness asserts the real
            # property (a trace whose spans DO carry messages must not be empty).
            continue

        kinds = [m["entry_type"] for m in view["messages"]]

        # A conversation with no assistant output usually means the response was not parsed.
        if "assistant" not in roles:
            warns.append(
                Warning(
                    "missing_assistant",
                    f"{name}: no assistant message ({len(roles)} msgs) - output not parsed?",
                )
            )

        # A tool call with no result, or vice versa.
        n_use, n_res = kinds.count("tool_use"), kinds.count("tool_result")
        if n_use != n_res:
            warns.append(
                Warning(
                    "unbalanced_tools",
                    f"{name}: {n_use} tool_use vs {n_res} tool_result",
                )
            )

        # Raw JSON leaking into a text position: the extractor did not unwrap the payload.
        for m in view["messages"]:
            c = m["content"]
            if m["entry_type"] in ("text", "thinking") and c.startswith(('{"', '[{"')):
                warns.append(
                    Warning(
                        "raw_json_text",
                        f"{name}: {m['entry_type']} at {m['index']} holds raw JSON - not unwrapped?",
                    )
                )
                break

        # An entry_type of "json" in a conversation view is usually an unparsed message blob.
        if "json" in kinds:
            warns.append(
                Warning(
                    "json_block",
                    f"{name}: {kinds.count('json')} raw 'json' block(s) - message not parsed?",
                )
            )

    return warns


def classify_warnings(
    label: str, warns: list[Warning]
) -> tuple[list[tuple[Warning, str]], list[str]]:
    expected = INTENTIONAL_WARNINGS.get(label, {})
    counts = Counter(w.code for w in warns)
    accepted: list[tuple[Warning, str]] = []
    unresolved: list[str] = []

    for code in sorted(set(counts) | set(expected)):
        observed = counts.get(code, 0)
        declaration = expected.get(code)
        if declaration is None:
            unresolved.extend(w.message for w in warns if w.code == code)
            continue

        expected_count, reason = declaration
        if observed != expected_count:
            unresolved.append(
                f"{code}: intentional classification expected {expected_count}, observed {observed}"
            )
            unresolved.extend(w.message for w in warns if w.code == code)
            continue

        accepted.extend((w, reason) for w in warns if w.code == code)

    return accepted, unresolved


def render(label: str, g: dict, detail: bool) -> None:
    warns = warnings_for(label, g)
    accepted, unresolved = classify_warnings(label, warns)
    flag = "  [!]" if unresolved else ("  [i]" if accepted else "")
    print(
        f"\n{'=' * 78}\n{label}{flag}\n"
        f"  requests={g['request_count']} spans={g['span_count']} traces={g['trace_count']} "
        f"sessions={g.get('session_count', len(g['session_views']))}"
    )
    for w in unresolved:
        print(f"  [!] {w}")
    for warning, reason in accepted:
        print(f"  [i] {warning.message} — {reason}")

    if not detail:
        for key, view in g["trace_views"].items():
            print(
                f"  trace {key}: {view['message_count']:3d} msgs  {' -> '.join(view['role_sequence'])}"
            )
        return

    for key, view in g["trace_views"].items():
        print(f"\n  --- trace {key}: {view['message_count']} msgs ---")
        if view["tool_names"]:
            print(f"      tools: {view['tool_names']}")
        for m in view["messages"]:
            extra = f" finish={m['finish_reason']}" if m.get("finish_reason") else ""
            print(
                f"      [{m['index']:2d}] {m['role']:9} {m['entry_type']:12}"
                f" {m['content'][:88]}{extra}"
            )


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    only_suspicious = "--suspicious" in sys.argv
    fixtures = load()
    if not fixtures:
        print(f"no expectations under {ROOT}", file=sys.stderr)
        return 1

    target = args[0] if args else None
    detail = target is not None

    shown = 0
    unresolved_count = 0
    intentional_count = 0
    for label, g in fixtures:
        if target and not label.startswith(target):
            continue
        warns = warnings_for(label, g)
        accepted, unresolved = classify_warnings(label, warns)
        if unresolved:
            unresolved_count += 1
        elif accepted:
            intentional_count += 1
        if only_suspicious and not unresolved:
            continue
        render(label, g, detail)
        shown += 1

    print(
        f"\n{'=' * 78}\n{shown} fixture(s) shown, {unresolved_count} unresolved and "
        f"{intentional_count} intentional warning fixture(s) across {len(fixtures)} total"
    )
    return 1 if unresolved_count else 0


if __name__ == "__main__":
    raise SystemExit(main())
