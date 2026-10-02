from semantic_kernel.agents import ChatCompletionAgent
from tools import forecast

from harness import Run, content


async def run(run: Run) -> None:
    agent = ChatCompletionAgent(
        service=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        plugins=[forecast],
    )
    with run.trace():
        async for chunk in agent.invoke_stream(messages=content.STREAMING):
            print(chunk.content, end="", flush=True)
        print()
