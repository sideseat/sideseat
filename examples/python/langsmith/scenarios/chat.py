from agent import answer, build_agent
from langchain_core.messages import HumanMessage

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm)
    with run.trace():
        print(answer(await agent.ainvoke({"messages": [HumanMessage(content.CHAT)]})))
