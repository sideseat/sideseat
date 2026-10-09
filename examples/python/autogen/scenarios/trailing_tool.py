"""A turn that ends on a tool result: nothing re-sends what the tool returned.

Reflection off, so the agent's last message is the tool's result rather than a model answer about it.
That is the shape in which a producer's own record of an execution is the only copy of the result - no
later request carries it - which is what this scenario exists to capture.
"""

from agent import answer, assistant
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        agent = assistant(run.llm, tools=[get_weather], reflect_on_tool_use=False)
        print(answer(await agent.run(task=content.TRAILING_TOOL)))
