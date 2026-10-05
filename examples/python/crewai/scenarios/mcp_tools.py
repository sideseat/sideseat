import os

from crew import kickoff, one_task_crew, travel_agent
from crewai.mcp import MCPServerStdio

from harness import Run, content
from harness.run import mcp_calculator_command


def run(run: Run) -> None:
    command, *args = mcp_calculator_command(relative=True)
    # CrewAI names the server's tools after `command` and `args`, and the model sees those names, so
    # neither may hold a path of this machine. CrewAI also installs its own, older uv into the
    # environment, first on PATH, which refuses the repository's required version, so the directory
    # of `UV`, the uv running this program, goes first on this process's PATH, which the server
    # inherits. Not through the server's `env`: CrewAI serialises that into its telemetry.
    if uv := os.environ.get("UV"):
        os.environ["PATH"] = os.pathsep.join(
            [os.path.dirname(uv), os.environ.get("PATH", "")]
        )
    agent = travel_agent(run.llm, mcps=[MCPServerStdio(command=command, args=args)])
    crew = one_task_crew(agent, content.MCP)
    with run.trace():
        print(kickoff(crew).raw)
