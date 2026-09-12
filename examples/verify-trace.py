#!/usr/bin/env python3
"""Judge whether one sample run produced correct telemetry.

Reads its inputs from SS_* environment variables and appends one TSV row to SS_OUT.
Only traces that started after SS_MARK count: judging the newest trace overall would
silently re-validate the previous run's trace whenever a run exports nothing, turning a
real failure into a pass.
"""

import json
import os
import subprocess
import sys

LABEL = os.environ["SS_LABEL"]
EXPECT_ERROR = os.environ["SS_EXPECT"] == "yes"
API = os.environ["SS_API"]
COOKIES = os.environ["SS_COOKIES"]
OUT = os.environ["SS_OUT"]
MARK = os.environ["SS_MARK"]


def get(url: str):
    proc = subprocess.run(["curl", "-s", "-b", COOKIES, url], capture_output=True, text=True)
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None


def row(*fields) -> None:
    with open(OUT, "a", encoding="utf-8") as fh:
        fh.write("\t".join(str(f) for f in fields) + "\n")


def main() -> None:
    listing = get(f"{API}/traces?limit=40") or {}
    fresh = [t for t in (listing.get("data") or []) if (t.get("start_time") or "") > MARK]
    if not fresh:
        row(LABEL, "NO_TRACE", 0, 0, 0, 0, "", f"no trace started after {MARK}")
        return

    trace = max(fresh, key=lambda t: t["start_time"])
    detail = get(f"{API}/traces/{trace['trace_id']}") or {}
    messages = (get(f"{API}/traces/{trace['trace_id']}/messages") or {}).get("messages", [])
    frameworks = sorted({s.get("framework") for s in detail.get("spans", []) if s.get("framework")})
    roles = sorted({m.get("role") for m in messages if m.get("role")})

    problems = []
    if trace.get("span_count", 0) < 1:
        problems.append("no spans")
    if not frameworks:
        problems.append("no framework classified")
    if EXPECT_ERROR:
        if not trace.get("has_error"):
            problems.append("expected an error, trace is clean")
    else:
        if trace.get("has_error"):
            problems.append("unexpected error in trace")
        if trace.get("total_tokens", 0) < 1:
            problems.append("zero tokens")
        if not messages:
            problems.append("no messages")

    row(
        LABEL,
        "OK" if not problems else "BAD",
        trace.get("span_count", 0),
        trace.get("total_tokens", 0),
        round(float(trace.get("total_cost") or 0), 5),
        len(messages),
        ",".join(frameworks) + ("|" + ",".join(roles) if roles else ""),
        "; ".join(problems),
    )


if __name__ == "__main__":
    sys.exit(main())
