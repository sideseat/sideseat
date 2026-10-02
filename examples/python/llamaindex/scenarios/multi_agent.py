from llama_index.core.agent.workflow import AgentWorkflow, FunctionAgent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    researcher = FunctionAgent(
        name="researcher",
        description="Researches the weather.",
        llm=run.llm,
        tools=[get_weather],
        system_prompt="You research weather with the tool, then hand off to the writer.",
        can_handoff_to=["writer"],
    )
    writer = FunctionAgent(
        name="writer",
        description="Writes the packing list.",
        llm=run.llm,
        system_prompt="You write the final packing list from the researcher's findings.",
    )
    workflow = AgentWorkflow(agents=[researcher, writer], root_agent="researcher")
    with run.trace():
        print(await workflow.run(user_msg=content.MULTI_AGENT))
