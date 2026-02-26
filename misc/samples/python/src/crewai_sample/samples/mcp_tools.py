"""MCP server integration sample."""

import shutil
from pathlib import Path

from crewai import Agent, Crew, Process, Task
from crewai.mcp import MCPServerStdio
from opentelemetry import trace


def run(llm, trace_attrs: dict):
    """Run the mcp_tools sample."""
    tracer = trace.get_tracer(__name__)

    # Use local MCP calculator server from misc/mcp (has its own venv with fastmcp)
    mcp_server_dir = Path(__file__).parents[5] / "mcp"
    uv = shutil.which("uv") or "uv"

    # Configure MCP server using structured configuration
    mcp_server = MCPServerStdio(
        command=uv,
        args=["run", "--directory", str(mcp_server_dir), "mcp-calculator"],
    )

    calculator_agent = Agent(
        role="Calculator",
        goal="Help users calculate expressions accurately",
        backstory="You help users to calculate expressions.",
        llm=llm,
        mcps=[mcp_server],
        verbose=False,
    )

    calculation_task = Task(
        description="Calculate an expression for me: What is 12345 plus 6789?",
        expected_output="The result of the calculation.",
        agent=calculator_agent,
    )

    crew = Crew(
        agents=[calculator_agent],
        tasks=[calculation_task],
        process=Process.sequential,
        verbose=False,
        share_crew=False,
    )

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        result = crew.kickoff()
        print(f"Result: {result.raw}")
