from agent import answer, build_agent
from langchain_core.messages import HumanMessage

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = build_agent(run.llm)
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(answer(await agent.ainvoke({"messages": [HumanMessage(question)]})))
