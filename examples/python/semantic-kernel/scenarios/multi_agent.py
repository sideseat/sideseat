from semantic_kernel.agents import ChatCompletionAgent
from tools import forecast

from harness import Run, content


async def run(run: Run) -> None:
    # An agent used as a plugin of another, as Semantic Kernel documents: the writer hands the research
    # to the researcher and writes the list from its answer.
    researcher = ChatCompletionAgent(
        service=run.llm,
        name="researcher",
        description="Researches the weather forecast for a city.",
        instructions="You research weather with the tool and report the forecast.",
        plugins=[forecast],
    )
    writer = ChatCompletionAgent(
        service=run.llm,
        name="writer",
        instructions="You write the final packing list. Ask the researcher for the weather first.",
        plugins=[researcher],
    )
    with run.trace():
        print(await writer.get_response(messages=content.MULTI_AGENT))
