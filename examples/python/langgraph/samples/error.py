"""Error sample — queries the selected provider with a nonexistent model ID."""

from langgraph.prebuilt import create_react_agent

INVALID_MODEL_ID = "nonexistent-model-id-12345"


def run(model, trace_attrs: dict):
    """Run the error sample with an invalid model from the selected provider.

    Reusing the concrete model class matters for conformance captures: hard-coding Bedrock made an
    OpenAI run fail locally on missing AWS credentials, so it never exercised the provider error
    path selected by the user.
    """
    invalid_model = type(model)(model=INVALID_MODEL_ID)
    agent = create_react_agent(model=invalid_model, tools=[])

    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

    agent.invoke({"messages": [("user", "What is 2 + 2?")]}, config=config)
