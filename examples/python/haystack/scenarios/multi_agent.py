from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage
from haystack.tools import ComponentTool
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    writer = build_agent(
        run.llm,
        system="You write the final packing list from the researcher's findings.",
    )
    # The researcher hands off by calling the writer agent as a tool.
    handoff = ComponentTool(
        component=writer,
        name="writer",
        description="Write the packing list from the weather findings.",
        outputs_to_string={"source": "last_message"},
    )
    researcher = build_agent(
        run.llm,
        tools=[get_weather, handoff],
        system="You research weather with the tool, then hand off to the writer.",
    )
    with run.trace():
        print(answer(ask(researcher, ChatMessage.from_user(content.MULTI_AGENT))))
