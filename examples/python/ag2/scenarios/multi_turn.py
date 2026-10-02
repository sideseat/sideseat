from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    # Asking the reply continues its conversation, so each question sends the history before it.
    agent = build_agent(run)
    first, *rest = content.MULTI_TURN
    with run.trace():
        reply = await agent.ask(first)
        print(await reply.content())
        for question in rest:
            reply = await reply.ask(question)
            print(await reply.content())
