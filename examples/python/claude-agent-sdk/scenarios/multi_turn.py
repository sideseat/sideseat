from agent import options, show
from claude_agent_sdk import ClaudeSDKClient

from harness import Run, content


async def run(run: Run) -> None:
    # One client is one conversation: each question is a new turn of the same session.
    with run.trace():
        async with ClaudeSDKClient(options=options(run)) as client:
            for question in content.MULTI_TURN:
                await client.query(question)
                await show(client.receive_response())
