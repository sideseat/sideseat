from agent import assistant
from autogen_agentchat.messages import StructuredMessage

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        agent = assistant(run.llm, output_content_type=content.TripPlan)
        result = await agent.run(task=content.STRUCTURED)
        reply = result.messages[-1]
        assert isinstance(reply, StructuredMessage)
        print(reply.content)
