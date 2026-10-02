from smolagents import ChatMessageStreamDelta, FinalAnswerStep, ToolCallingAgent
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(
        tools=[get_weather],
        model=run.llm,
        instructions=content.SYSTEM,
        stream_outputs=True,
    )
    with run.trace():
        for event in agent.run(content.STREAMING, stream=True):
            if isinstance(event, ChatMessageStreamDelta) and event.content:
                print(event.content, end="", flush=True)
            elif isinstance(event, FinalAnswerStep):
                print(f"\n{event.output}")
