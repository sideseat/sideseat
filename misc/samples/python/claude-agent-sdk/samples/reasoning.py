"""Extended thinking.

Demonstrates:
- thinking config and ThinkingBlock output
- effort levels

The Agent SDK docs state the thinking config is not sent to Bedrock, but verified
against bedrock-haiku on 2026-08-25 ThinkingBlock output does arrive. Treat Bedrock
thinking support as version-dependent rather than guaranteed: if a run produces no
thinking blocks, that is the documented behaviour reasserting itself, not a bug here.
"""

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import query

PROBLEMS = [
    (
        "logic-puzzle",
        "Three switches outside a room control three bulbs inside. You may flip "
        "switches freely but enter the room only once. How do you determine which "
        "switch controls which bulb?",
    ),
    (
        "arithmetic",
        "A train leaves at 14:20 travelling 80 km/h. Another leaves the same station "
        "at 15:05 travelling 110 km/h on the same track. When does the second catch "
        "the first?",
    ),
]


async def run(model, trace_attrs: dict, client, env: dict):
    options = build_options(
        model,
        env,
        thinking={"type": "adaptive", "display": "summarized"},
        effort="high",
        # No tools needed; this is pure reasoning.
        allowed_tools=[],
        system_prompt="Think the problem through, then give a short final answer.",
    )

    with trace_scope(client, "claude-agent-reasoning", trace_attrs):
        for name, prompt in PROBLEMS:
            print(f"--- {name} ---")
            await print_stream(
                query(prompt=prompt, options=options), show_thinking=True
            )
            print()
