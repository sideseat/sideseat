import boto3
from strands import Agent
from strands.models import BedrockModel

session = boto3.Session()
bedrock_model = BedrockModel(
    model_id="us.anthropic.claude-sonnet-4-20250514-v1:0",
    boto_session=session,
    endpoint_url="http://localhost:5001/proxy/converse",
)

agent = Agent(model=bedrock_model)

agent("Say hello to the world!")
