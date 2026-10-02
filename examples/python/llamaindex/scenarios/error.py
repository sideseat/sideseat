from agent import build_agent
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    # The agent returns the tool's exception to the model as an error result.
    agent = build_agent(run.llm, tools=[book_flight])
    with run.trace():
        print(await agent.run(user_msg=content.ERROR))
