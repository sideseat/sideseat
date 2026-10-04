from agent import options, show
from claude_agent_sdk import AgentDefinition, query
from tools import get_weather, server

from harness import Run, content


async def run(run: Run) -> None:
    servers, weather = server(get_weather)
    agents = {
        "researcher": AgentDefinition(
            description="Researches the weather for a city with the weather tool.",
            prompt="You research weather with the tool and report the forecast.",
            tools=weather,
        ),
        "writer": AgentDefinition(
            description="Writes a packing list from a weather report.",
            prompt="You write the final packing list from the researcher's findings.",
            tools=[],
        ),
    }
    settings = options(
        run,
        system_prompt=(
            "You coordinate two agents: ask the researcher for the weather, then the writer for "
            "the packing list, and return the writer's list."
        ),
        tools=["Agent"],
        allowed_tools=["Agent", *weather],
        mcp_servers=servers,
        agents=agents,
    )
    with run.trace():
        await show(query(prompt=content.MULTI_AGENT, options=settings))
