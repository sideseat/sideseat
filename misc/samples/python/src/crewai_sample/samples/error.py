"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from crewai import Agent, Crew, Process, Task
from crewai import LLM
from opentelemetry import trace

INVALID_MODEL_ID = "bedrock/nonexistent-model-id-12345"


def run(llm, trace_attrs: dict):
    """Run the error sample with an invalid model ID."""
    tracer = trace.get_tracer(__name__)

    invalid_llm = LLM(model=INVALID_MODEL_ID)

    agent = Agent(
        role="Assistant",
        goal="Answer questions",
        backstory="You are a helpful assistant.",
        llm=invalid_llm,
        verbose=False,
    )

    task = Task(
        description="What is 2 + 2?",
        expected_output="A number.",
        agent=agent,
    )

    crew = Crew(
        agents=[agent],
        tasks=[task],
        process=Process.sequential,
        verbose=False,
        share_crew=False,
    )

    with tracer.start_as_current_span("crewai.session", attributes=trace_attrs):
        crew.kickoff()
