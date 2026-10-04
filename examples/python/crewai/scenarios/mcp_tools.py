import os

from crew import kickoff, one_task_crew, travel_agent
from crewai.mcp import MCPServerStdio

from harness import Run, content
from harness.run import mcp_calculator_command


def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    # CrewAI installs its own, older uv into the environment, which comes first on PATH and refuses
    # the repository's required version; `UV` is the uv running this program.
    command = os.environ.get("UV", command)
    agent = travel_agent(run.llm, mcps=[MCPServerStdio(command=command, args=args)])
    crew = one_task_crew(agent, content.MCP)
    with run.trace():
        print(kickoff(crew).raw)
