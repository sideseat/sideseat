"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import MaxMessageTermination
from autogen_agentchat.teams import RoundRobinGroupChat
from autogen_core.models import ModelInfo
from autogen_ext.models.openai import OpenAIChatCompletionClient
from opentelemetry import trace

INVALID_MODEL_ID = "nonexistent-model-id-12345"


async def run(model_client, trace_attrs: dict):
    """Run the error sample with an invalid model ID."""
    tracer = trace.get_tracer(__name__)

    invalid_client = OpenAIChatCompletionClient(
        model=INVALID_MODEL_ID,
        model_info=ModelInfo(
            vision=False,
            function_calling=True,
            json_output=True,
            family="unknown",
        ),
    )

    agent = AssistantAgent(
        name="assistant",
        model_client=invalid_client,
        system_message="You are a helpful assistant.",
    )

    termination = MaxMessageTermination(max_messages=3)
    team = RoundRobinGroupChat([agent], termination_condition=termination)

    with tracer.start_as_current_span("autogen.session", attributes=trace_attrs):
        await team.run(task="What is 2 + 2?")

    await invalid_client.close()
