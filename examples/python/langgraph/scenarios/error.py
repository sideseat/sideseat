from agent import answer, build_agent
from langchain_core.messages import HumanMessage
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    # ToolNode turns the tool's exception into an error result the model reads and answers.
    agent = build_agent(run.llm, tools=[book_flight])
    with run.trace():
        print(answer(await agent.ainvoke({"messages": [HumanMessage(content.ERROR)]})))
