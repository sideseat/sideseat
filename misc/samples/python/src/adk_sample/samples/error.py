"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from google.adk.agents import LlmAgent
from google.adk.models.lite_llm import LiteLlm
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types

INVALID_MODEL_ID = "bedrock/nonexistent-model-id-12345"
APP_NAME = "error_app"


async def run(model, trace_attrs: dict):
    """Run the error sample with an invalid model ID."""
    invalid_model = LiteLlm(model=INVALID_MODEL_ID)

    agent = LlmAgent(
        model=invalid_model,
        name="assistant",
        instruction="You are a helpful assistant.",
    )

    session_service = InMemorySessionService()
    session = await session_service.create_session(
        app_name=APP_NAME,
        user_id="demo-user",
        session_id=trace_attrs["session.id"],
    )

    runner = Runner(
        agent=agent,
        app_name=APP_NAME,
        session_service=session_service,
    )

    user_message = types.Content(
        role="user",
        parts=[types.Part(text="What is 2 + 2?")],
    )

    async for event in runner.run_async(
        session_id=session.id,
        user_id="demo-user",
        new_message=user_message,
    ):
        pass
