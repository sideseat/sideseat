from pydantic_ai import Agent, RunContext
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    # Agent delegation, as Pydantic AI documents it: the writer calls the researcher from a tool.
    researcher = Agent(
        run.llm,
        name="researcher",
        instructions="You research weather with the tool and report the forecast.",
        tools=[get_weather],
    )
    writer = Agent(
        run.llm,
        name="writer",
        instructions="You write the final packing list. Ask the researcher for the weather first.",
    )

    @writer.tool
    async def ask_researcher(ctx: RunContext, request: str) -> str:
        """Ask the weather researcher a question.

        Args:
            request: What to research.
        """
        return (await researcher.run(request, usage=ctx.usage)).output

    with run.trace():
        print(writer.run_sync(content.MULTI_AGENT).output)
