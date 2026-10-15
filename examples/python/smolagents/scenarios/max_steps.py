from smolagents import ChatMessageStreamDelta, FinalAnswerStep, ToolCallingAgent
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    # One step, which the weather lookup takes, so the streamed run ends in smolagents' fallback: a
    # further, non-streamed model call that asks for the answer to the task.
    agent = ToolCallingAgent(
        tools=[get_weather],
        model=run.llm,
        instructions=content.SYSTEM,
        stream_outputs=True,
        max_steps=1,
    )
    with run.trace():
        for event in agent.run(content.MAX_STEPS, stream=True):
            if isinstance(event, ChatMessageStreamDelta) and event.content:
                print(event.content, end="", flush=True)
            elif isinstance(event, FinalAnswerStep):
                print(f"\n{event.output}")
