from typing import cast

from agent import build_agent
from llama_index.core.agent.workflow import AgentOutput

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm, output_cls=content.TripPlan)
    with run.trace():
        result = cast(AgentOutput, await agent.run(user_msg=content.STRUCTURED))
        print(result.structured_response)
