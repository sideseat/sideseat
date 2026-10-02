from strands import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, system_prompt=content.SYSTEM, callback_handler=None)
    with run.trace():
        result = agent(content.STRUCTURED, structured_output_model=content.TripPlan)
        print(result.structured_output)
