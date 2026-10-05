from agent import answer, build_agent
from langchain_core.messages import HumanMessage
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(build(run.model, reasoning=True), system=None)
    with run.trace():
        print(
            answer(await agent.ainvoke({"messages": [HumanMessage(content.REASONING)]}))
        )
