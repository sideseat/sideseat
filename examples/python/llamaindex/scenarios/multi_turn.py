from agent import build_agent
from llama_index.core.workflow import Context

from harness import Run, content


async def run(run: Run) -> None:
    # The context keeps the chat history, so each question sends the history before it.
    agent = build_agent(run.llm)
    context = Context(agent)
    with run.trace():
        for question in content.MULTI_TURN:
            print(await agent.run(user_msg=question, ctx=context))
