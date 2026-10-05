from agents import agent, answer
from tools import tools
from travel_site import serve

from harness import Run, content


async def run(run: Run) -> None:
    # Browser actions on the local site (open it, follow a link, read a table) and a custom action,
    # called once per city.
    with serve() as site, run.trace():
        task = (
            f"{content.TOOL_USE} Read the forecast on the travel site at {site} and check the "
            "chance of rain for each city with the get_precipitation tool."
        )
        browser = agent(run, task, tools=tools(content.get_precipitation))
        print(answer(await browser.run(max_steps=8)))
