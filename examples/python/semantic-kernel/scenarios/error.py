from semantic_kernel.agents import ChatCompletionAgent
from tools import booking

from harness import Run, content


async def run(run: Run) -> None:
    agent = ChatCompletionAgent(
        service=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        plugins=[booking],
    )
    with run.trace():
        print(await agent.get_response(messages=content.ERROR))
