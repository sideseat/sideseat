#!/usr/bin/env python3
"""Print the recorded message expectations in a readable form, for review.

A golden is only worth having if someone read it. `git diff` on the JSON is unreviewable at
this size, so this renders the parts that matter — per-view message count, role sequence and
content — and flags patterns that usually mean a parsing defect.

Usage:
    misc/review-message-goldens.py                    # every fixture, summary only
    misc/review-message-goldens.py strands            # one suite, full detail
    misc/review-message-goldens.py strands/tool_use   # one sample, full detail
    misc/review-message-goldens.py --suspicious       # only fixtures with warnings
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "server/tests/fixtures/messages"


def load() -> list[tuple[str, dict]]:
    out = []
    for path in sorted(ROOT.rglob("expected.json")):
        label = str(path.parent.relative_to(ROOT))
        out.append((label, json.loads(path.read_text())))
    return out


def warnings_for(label: str, g: dict) -> list[str]:
    """Patterns that usually mean a parsing defect. Heuristics, not invariants: the test
    holds the hard guarantees, this is a reading aid that says where to look."""
    warns = []
    # Only meaningful when the fixture HAS sessions: many samples never set a session id, and
    # "no sessions" is not "empty sessions".
    if g["session_views"] and not any(v["message_count"] for v in g["session_views"].values()):
        warns.append("the fixture has sessions but every session view is empty")

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
            warns.append(f"{name}: no assistant message ({len(roles)} msgs) - output not parsed?")

        # A tool call with no result, or vice versa.
        n_use, n_res = kinds.count("tool_use"), kinds.count("tool_result")
        if n_use != n_res:
            warns.append(f"{name}: {n_use} tool_use vs {n_res} tool_result")

        # Raw JSON leaking into a text position: the extractor did not unwrap the payload.
        for m in view["messages"]:
            c = m["content"]
            if m["entry_type"] in ("text", "thinking") and c.startswith(('{"', '[{"')):
                warns.append(
                    f"{name}: {m['entry_type']} at {m['index']} holds raw JSON - not unwrapped?"
                )
                break

        # An entry_type of "json" in a conversation view is usually an unparsed message blob.
        if "json" in kinds:
            warns.append(f"{name}: {kinds.count('json')} raw 'json' block(s) - message not parsed?")

    return warns


def render(label: str, g: dict, detail: bool) -> None:
    warns = warnings_for(label, g)
    flag = "  [!]" if warns else ""
    print(
        f"\n{'=' * 78}\n{label}{flag}\n"
        f"  requests={g['request_count']} spans={g['span_count']} traces={g['trace_count']} "
        f"sessions={g.get('session_count', len(g['session_views']))}"
    )
    for w in warns:
        print(f"  [!] {w}")

    if not detail:
        for key, view in g["trace_views"].items():
            print(f"  trace {key}: {view['message_count']:3d} msgs  {' -> '.join(view['role_sequence'])}")
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
    suspicious = 0
    for label, g in fixtures:
        if target and not label.startswith(target):
            continue
        warns = warnings_for(label, g)
        if warns:
            suspicious += 1
        if only_suspicious and not warns:
            continue
        render(label, g, detail)
        shown += 1

    print(
        f"\n{'=' * 78}\n{shown} fixture(s) shown, {suspicious}/{len(fixtures)} "
        f"carry warnings across {len(fixtures)} total"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
