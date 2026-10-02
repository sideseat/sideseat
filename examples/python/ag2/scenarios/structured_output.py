from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    # On Bedrock AG2 asks for a schema-constrained answer through Converse's native output config.
    agent = build_agent(run)
    with run.trace():
        reply = await agent.ask(content.STRUCTURED, response_schema=content.TripPlan)
        print(await reply.content())
