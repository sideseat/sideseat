from ag2 import PromptedSchema
from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    # Current Claude models on Bedrock reject Converse's native output format, so the schema goes in
    # the prompt and AG2 validates the reply against it.
    agent = build_agent(run)
    with run.trace():
        reply = await agent.ask(
            content.STRUCTURED, response_schema=PromptedSchema(content.TripPlan)
        )
        print(await reply.content())
