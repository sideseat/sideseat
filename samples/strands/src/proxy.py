import boto3
from strands import Agent
from strands.models import BedrockModel


MODEL_ID = "us.anthropic.claude-haiku-4-5-20251001-v1:0"
ENDPOINT_URL = "http://localhost:5001/proxy/converse"


def main():
    """Main function to run the proxy example."""
    session = boto3.Session()
    bedrock_model = BedrockModel(
        model_id=MODEL_ID,
        boto_session=session,
        endpoint_url=ENDPOINT_URL,
        streaming=False,
    )

    agent = Agent(model=bedrock_model)
    response = agent("Say hello to the world!")
    print(f"Agent response: {response}")


if __name__ == "__main__":
    main()
