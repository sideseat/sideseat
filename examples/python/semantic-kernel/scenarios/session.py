from semantic_kernel.agents import ChatCompletionAgent

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = ChatCompletionAgent(
        service=run.llm, name="assistant", instructions=content.SYSTEM
    )
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(await agent.get_response(messages=question))
