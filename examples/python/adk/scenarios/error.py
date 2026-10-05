from typing import Any

from conversation import Conversation
from google.adk.agents import LlmAgent
from google.adk.tools import BaseTool, ToolContext
from tools import book_flight

from harness import Run, content


def report_error(
    tool: BaseTool, args: dict[str, Any], tool_context: ToolContext, error: Exception
) -> dict[str, Any]:
    # ADK ends the run when a tool raises unless an error callback answers for the tool; this one
    # hands the error to the model, as ADK's callback documentation shows.
    return {"error": f"{type(error).__name__}: {error}"}


async def run(run: Run) -> None:
    agent = LlmAgent(
        name="assistant",
        model=run.llm,
        instruction=content.SYSTEM,
        tools=[book_flight],
        on_tool_error_callback=report_error,
    )
    with run.trace():
        print(await Conversation(run, agent).ask(content.ERROR))
