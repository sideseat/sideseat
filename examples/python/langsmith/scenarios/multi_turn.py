from agent import answer, build_agent
from langchain_core.messages import HumanMessage
from langgraph.checkpoint.memory import InMemorySaver

from harness import Run, content


async def run(run: Run) -> None:
    # The checkpointer keeps the thread's messages, so each turn sends the history before it.
    agent = build_agent(run.llm, checkpointer=InMemorySaver())
    config = {"configurable": {"thread_id": run.session_id}}
    with run.trace():
        for question in content.MULTI_TURN:
            state = await agent.ainvoke({"messages": [HumanMessage(question)]}, config)
            print(answer(state))
