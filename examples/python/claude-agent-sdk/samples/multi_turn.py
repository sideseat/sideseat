"""Multi-turn conversation with a reused session.

Demonstrates:
- ClaudeSDKClient, which holds one session across turns (query() starts fresh each call)
- receive_response(), which yields messages up to and including the ResultMessage
- Per-call cost accumulation

All turns share one session.id, so they group into a single timeline in SideSeat.

NOTE: avoid `break` while iterating the message stream; it can leave asyncio cleanup
half-done. Track state with a flag instead.
"""

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import ClaudeSDKClient

TURNS = [
    "What is the capital of France?",
    "What is the population of that city?",
    "How does that compare to the city I first asked about?",
]


async def run(model, trace_attrs: dict, client, env: dict):
    options = build_options(
        model,
        env,
        allowed_tools=[],
        system_prompt="You are a concise geography assistant. Answer in one sentence.",
    )

    total_cost = 0.0

    with trace_scope(client, "claude-agent-multi-turn", trace_attrs):
        async with ClaudeSDKClient(options=options) as agent:
            for turn, prompt in enumerate(TURNS, start=1):
                print(f"--- Turn {turn}: {prompt} ---")
                await agent.query(prompt)
                result = await print_stream(agent.receive_response())
                if result is not None:
                    total_cost += result.total_cost_usd or 0.0
                print()

    # Each query() reports only its own cost; the SDK provides no session total.
    print(f"Session total (estimate): ${total_cost:.6f}")
