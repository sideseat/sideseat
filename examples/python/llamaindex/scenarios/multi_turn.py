from agent import build_agent
from llama_index.core.memory import Memory

from harness import Run, content


async def run(run: Run) -> None:
    # The memory holds the chat history across runs, so each question sends the history before it.
    agent = build_agent(run.llm)
    memory = Memory.from_defaults(session_id=run.session_id)
    with run.trace():
        for question in content.MULTI_TURN:
            print(await agent.run(user_msg=question, memory=memory))
