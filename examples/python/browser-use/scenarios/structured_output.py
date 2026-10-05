from agents import agent, answer

from harness import Run, content


async def run(run: Run) -> None:
    # output_model_schema makes the done action's result a TripPlan.
    with run.trace():
        browser = agent(run, content.STRUCTURED, output_model_schema=content.TripPlan)
        history = await browser.run(max_steps=5)
        answer(history)
        print(history.structured_output)
