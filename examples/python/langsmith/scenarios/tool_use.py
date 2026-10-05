from agent import answer, build_agent
from langchain_core.messages import HumanMessage
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm, tools=[get_weather, get_precipitation])
    with run.trace():
        print(
            answer(await agent.ainvoke({"messages": [HumanMessage(content.TOOL_USE)]}))
        )
