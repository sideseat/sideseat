from conversation import Conversation
from google.adk.agents import LlmAgent

from harness import Run, content


async def run(run: Run) -> None:
    plans: list[content.TripPlan] = []

    # ADK's output_schema asks LiteLLM for Bedrock's native output format, or a forced tool choice,
    # and current Claude models reject both; the schema is a tool the model chooses to hand its plan to.
    def trip_plan(city: str, days: list[str], budget_eur: int) -> str:
        """Record the finished trip plan.

        Args:
            city: The destination city.
            days: One activity per day.
            budget_eur: Estimated total budget in euros.
        """
        plans.append(content.TripPlan(city=city, days=days, budget_eur=budget_eur))
        return "Plan recorded."

    agent = LlmAgent(
        name="planner",
        model=run.llm,
        instruction=f"{content.SYSTEM} Hand the finished plan to trip_plan.",
        tools=[trip_plan],
    )
    with run.trace():
        await Conversation(run, agent).ask(content.STRUCTURED)
    print(plans[-1])
