"""Deterministic AutoGen coverage for text, streaming, tools, and sessions."""

from autogen_agentchat.agents import AssistantAgent
from opentelemetry import trace


async def get_weather(location: str) -> str:
    """Return deterministic weather for fixture capture."""
    return f"Sunny, 22°C, light breeze in {location}."


async def run(model_client, trace_attrs: dict) -> None:
    """Run the compact production-readiness conversation."""
    tracer = trace.get_tracer(__name__)
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    plain_agent = AssistantAgent(
        name="plain_assistant",
        model_client=model_client,
        system_message="Answer in one sentence.",
    )
    with tracer.start_as_current_span(
        "autogen.plain",
        attributes={"session.id": session_id, "user.id": user_id},
    ):
        result = await plain_agent.run(task="What is the boiling point of water?")
        print(f"Plain: {result.messages[-1].content}")

    streaming_agent = AssistantAgent(
        name="streaming_assistant",
        model_client=model_client,
        system_message="Answer in one sentence.",
        model_client_stream=True,
    )
    with tracer.start_as_current_span(
        "autogen.streaming",
        attributes={"session.id": session_id, "user.id": user_id},
    ):
        result = await streaming_agent.run(task="What is the speed of light?")
        print(f"Streaming: {result.messages[-1].content}")

    tool_agent = AssistantAgent(
        name="weather_assistant",
        model_client=model_client,
        system_message="Use get_weather for weather questions.",
        tools=[get_weather],
        reflect_on_tool_use=True,
    )
    with tracer.start_as_current_span(
        "autogen.tool",
        attributes={"session.id": session_id, "user.id": user_id},
    ):
        result = await tool_agent.run(task="What's the weather in Paris?")
        print(f"Tool: {result.messages[-1].content}")

    await model_client.close()
