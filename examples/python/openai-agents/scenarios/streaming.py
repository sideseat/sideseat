from agents import Agent, Runner
from openai.types.responses import ResponseTextDeltaEvent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        name="assistant",
        instructions=content.SYSTEM,
        model=run.llm,
        tools=[get_weather],
    )
    with run.trace():
        result = Runner.run_streamed(agent, content.STREAMING)
        async for event in result.stream_events():
            if event.type == "raw_response_event" and isinstance(
                event.data, ResponseTextDeltaEvent
            ):
                print(event.data.delta, end="", flush=True)
        print()
