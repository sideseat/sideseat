"""SideSeat WS bridge: multi-agent (Strands Graph) presence + AG-UI invoke demo.

Builds a profiler -> analyst -> strategist Strands Graph over a DuckDB-backed
`run_sql` tool against misc/data/car_sales.csv. The graph and all three inner
agents are registered with SideSeat over the persistent WS:

- The graph appears in registrations as kind="graph" (presence/introspection).
- profiler / analyst / strategist appear as kind="agent" and are individually
  invokable via POST /api/v1/project/{project_id}/agents/{name}/runs.

Each agent runs with adaptive thinking enabled so the AG-UI stream surfaces
reasoning blocks alongside text. Pattern lifted from aws-engagement's
`solution/core/bedrock.py` — Bedrock's `additionalModelRequestFields.thinking`
is what makes Strands emit `reasoningText` deltas during `stream_async`, which
the SDK's `StrandsEventTranslator` maps to AG-UI `REASONING_*` events.


Analyze the global car sales dataset at "car_sales.csv" and surface the single most actionable business insight.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import duckdb
from strands import Agent, tool
from strands.handlers.callback_handler import null_callback_handler
from strands.models.bedrock import BedrockModel
from strands.multiagent import GraphBuilder

# misc/samples/python/strands/samples/strands_ws.py -> misc/
DATA_PATH = (Path(__file__).resolve().parents[4] / "data" / "car_sales.csv").as_posix()


def _thinking_model() -> BedrockModel:
    """Build a Bedrock Sonnet 4.6 model with adaptive thinking.

    Sonnet 4.6 uses the adaptive form (`type: "adaptive"`); Bedrock
    accepts only `display: "summarized"` or `"omitted"` here — `"raw"`
    is rejected. With "summarized" the model thinks internally and
    emits the reasoning as one block at the end, so there is a pause
    before the reasoning section appears, then it lands all at once.
    True per-token reasoning streaming is not available on 4.6 — that
    was a 4.5-era behaviour with the legacy `type: "enabled"` form.

    Override via STRANDS_WS_MODEL_ID / STRANDS_WS_THINKING_DISPLAY.
    """
    model_id = os.getenv(
        "STRANDS_WS_MODEL_ID",
        "global.anthropic.claude-sonnet-4-6",
    )
    region = os.getenv("AWS_REGION") or os.getenv("AWS_DEFAULT_REGION", "us-east-1")
    display = os.getenv("STRANDS_WS_THINKING_DISPLAY", "summarized")

    extra: dict[str, Any] = {
        "thinking": {"type": "adaptive", "display": display},
    }

    return BedrockModel(
        model_id=model_id,
        region_name=region,
        additional_request_fields=extra,
    )


@tool
def run_sql(query: str) -> str:
    """Execute a read-only DuckDB SQL query against the car_sales dataset.

    The dataset is a CSV file referenced as a string literal in FROM clauses,
    e.g. SELECT * FROM 'car_sales.csv' LIMIT 5. The path is rewritten
    internally so agents can use the bare filename.

    Returns a pipe-delimited table truncated to 50 rows.
    """
    rewritten = query.replace("'car_sales.csv'", f"'{DATA_PATH}'")
    try:
        con = duckdb.connect(":memory:")
        cur = con.execute(rewritten)
        cols = [d[0] for d in cur.description]
        rows = cur.fetchall()
    except Exception as e:
        return f"SQL ERROR: {e}"

    truncated = len(rows) > 50
    rows = rows[:50]
    lines = [" | ".join(cols), " | ".join("---" for _ in cols)]
    for r in rows:
        lines.append(" | ".join("" if v is None else str(v) for v in r))
    if truncated:
        lines.append(f"\n_(truncated to 50 of {cur.rowcount} rows)_")
    return "\n".join(lines)


def run(model, trace_attrs: dict, *, client=None) -> None:
    if client is None:
        raise RuntimeError(
            "strands_ws sample requires --sideseat (the WS bridge lives on the SDK)"
        )

    profiler = Agent(
        name="profiler",
        model=model,
        tools=[run_sql],
        trace_attributes=trace_attrs,
        callback_handler=null_callback_handler,
        system_prompt=(
            "You are a data profiler. Use the `run_sql` tool to inspect the dataset "
            "at 'car_sales.csv'. Run AT MOST 3 SQL queries: "
            "  1) SUMMARIZE SELECT * FROM 'car_sales.csv' "
            "  2) GROUP BY region or brand to see top categories "
            "  3) one histogram of sale_price_usd. "
            "Then output a concise profile: row count, period, key dimensions, "
            "price distribution shape, and 2-3 things that look interesting and worth "
            "exploring. Keep it under 200 words."
        ),
    )

    analyst = Agent(
        name="analyst",
        model=model,
        tools=[run_sql],
        trace_attributes=trace_attrs,
        callback_handler=null_callback_handler,
        system_prompt=(
            "You are a data analyst. Read the profiler's notes from your input. "
            "Form 2 concrete hypotheses about non-obvious business patterns "
            "(e.g. channel arbitrage, depreciation by tier, EV adoption asymmetry, "
            "regional price gaps). For each hypothesis: state it, run ONE SQL query "
            "against 'car_sales.csv' to test it, and report whether it holds. "
            "Use `run_sql`. Keep total output under 350 words. Show the SQL you ran."
        ),
    )

    strategist = Agent(
        name="strategist",
        model=model,
        trace_attributes=trace_attrs,
        callback_handler=null_callback_handler,
        system_prompt=(
            "You are a business strategist. Based on the analyst's findings in your "
            "input, write ONE concrete recommendation: who should do what, in which "
            "segment, expected upside, and the main risk. No SQL. Under 150 words. "
            "End with a single line: `BOTTOM LINE: <one-sentence takeaway>`."
        ),
    )

    builder = GraphBuilder()
    builder.add_node(profiler, "profile")
    builder.add_node(analyst, "analyze")
    builder.add_node(strategist, "recommend")
    builder.add_edge("profile", "analyze")
    builder.add_edge("analyze", "recommend")
    builder.set_entry_point("profile")
    builder.set_execution_timeout(300)
    builder.set_node_timeout(120)
    car_sales_pipeline = builder.build()

    client.register(car_sales_pipeline).connect()
