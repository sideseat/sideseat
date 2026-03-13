"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from langchain_aws import ChatBedrockConverse
from langgraph.prebuilt import create_react_agent

INVALID_MODEL_ID = "nonexistent-model-id-12345"


def run(model, trace_attrs: dict):
    """Run the error sample with an invalid model ID."""
    invalid_model = ChatBedrockConverse(model=INVALID_MODEL_ID, region_name="us-east-1")
    agent = create_react_agent(model=invalid_model, tools=[])

    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

    agent.invoke({"messages": [("user", "What is 2 + 2?")]}, config=config)
