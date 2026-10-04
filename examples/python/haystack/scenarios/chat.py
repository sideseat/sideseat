from agent import answer, build_agent
from haystack import Pipeline
from haystack.dataclasses import ChatMessage

from harness import Run, content


def run(run: Run) -> None:
    # The one scenario that runs the agent as a pipeline component, so the pipeline's own tracing
    # is captured as well.
    pipeline = Pipeline()
    pipeline.add_component("agent", build_agent(run.llm))
    with run.trace():
        result = pipeline.run(
            {"agent": {"messages": [ChatMessage.from_user(content.CHAT)]}}
        )
        print(answer(result["agent"]))
