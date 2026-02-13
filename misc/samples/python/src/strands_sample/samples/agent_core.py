"""AWS AgentCore services sample demonstrating memory, code interpreter, and S3 sessions.

This sample shows how to:
1. Store memories using the AgentCore batch_create_memory_records API
2. Retrieve memories via semantic search using the Strands memory tool
3. Execute code using the AgentCore code interpreter
4. Persist agent conversation state using S3 session management

Prerequisites:
- AGENT_CORE_MEMORY_ID: Memory store ID from AWS Bedrock AgentCore
- AGENT_CORE_MEMORY_STRATEGY_ID: Memory strategy ID configured in the store
- S3_BUCKET_NAME: S3 bucket for session storage (optional)
- AWS credentials with bedrock-agentcore and s3 permissions
"""

import os
import uuid
from datetime import datetime, timezone

import boto3
from strands import Agent
from strands.session.s3_session_manager import S3SessionManager
from strands_tools.agent_core_memory import AgentCoreMemoryToolProvider
from strands_tools.code_interpreter import AgentCoreCodeInterpreter

AWS_REGION = os.getenv("AWS_REGION", "us-east-1")
AGENT_CORE_MEMORY_ID = os.getenv("AGENT_CORE_MEMORY_ID")
AGENT_CORE_MEMORY_STRATEGY_ID = os.getenv("AGENT_CORE_MEMORY_STRATEGY_ID")
S3_BUCKET_NAME = os.getenv("S3_BUCKET_NAME")

SYSTEM_PROMPT = """You are an AI assistant that validates answers through code execution.
When asked about code, algorithms, or calculations, write Python code to verify your answers.
When asked about user preferences or personal information (like favorite numbers, names, etc.),
always check your memory tool first to retrieve any stored information."""


def memory_exists(
    client, memory_id: str, strategy_id: str, query: str, namespace: str = "default"
) -> bool:
    """Check if a memory matching the query already exists."""
    response = client.retrieve_memory_records(
        memoryId=memory_id,
        namespace=namespace,
        searchCriteria={
            "searchQuery": query,
            "memoryStrategyId": strategy_id,
            "topK": 1,
        },
    )
    records = response.get("memoryRecordSummaries", [])
    # Consider a match if score > 0.5 (semantic search scores are typically 0.5-0.7)
    return len(records) > 0 and records[0].get("score", 0) > 0.5


def create_memory_record(
    client, memory_id: str, strategy_id: str, text: str, namespace: str = "default"
):
    """Create a searchable memory record using the AgentCore API.

    Uses batch_create_memory_records which creates immediately searchable records,
    unlike create_event which stores raw events requiring extraction.
    """
    return client.batch_create_memory_records(
        memoryId=memory_id,
        records=[
            {
                "requestIdentifier": str(uuid.uuid4()),
                "namespaces": [namespace],
                "content": {"text": text},
                "timestamp": datetime.now(timezone.utc),
                "memoryStrategyId": strategy_id,
            }
        ],
    )


def discover_strategy_ids(client, memory_id: str, namespace: str = "default") -> set[str]:
    """Discover available memory strategy IDs from existing records."""
    try:
        response = client.list_memory_records(
            memoryId=memory_id, namespace=namespace, maxResults=10
        )
        return {r["memoryStrategyId"] for r in response.get("memoryRecordSummaries", [])}
    except Exception:
        return set()


def print_setup_help(client, memory_id: str | None):
    """Print helpful setup instructions."""
    print("\nAgentCore Memory Setup Help")
    print("-" * 50)

    if not memory_id:
        print("AGENT_CORE_MEMORY_ID is not set.")
        print("\nTo find your memory ID:")
        print("  1. Go to AWS Console > Amazon Bedrock > AgentCore > Memories")
        print("  2. Create or select a memory store")
        print("  3. Copy the Memory ID (e.g., 'memory_abc123')")
        print("  4. Set AGENT_CORE_MEMORY_ID in misc/.env")
        return

    print(f"Memory ID: {memory_id}")
    print("\nLooking up strategy IDs from existing records...")

    strategy_ids = discover_strategy_ids(client, memory_id)
    if strategy_ids:
        print(f"\nFound {len(strategy_ids)} strategy ID(s) in existing records:")
        for sid in sorted(strategy_ids):
            print(f"  - {sid}")
        print("\nSet AGENT_CORE_MEMORY_STRATEGY_ID in misc/.env to one of these IDs")
    else:
        print("\nNo existing records found to discover strategy IDs.")
        print(
            "Find your strategy ID in AWS Console > Bedrock > AgentCore > Memories > [your memory]"
        )


def create_session_manager(boto_session: boto3.Session) -> S3SessionManager | None:
    """Create S3 session manager if bucket is configured."""
    if not S3_BUCKET_NAME:
        print("S3_BUCKET_NAME not set, session persistence disabled")
        return None

    session_id = f"{datetime.now(timezone.utc):%Y-%m-%d-%H-%M-%S}-{uuid.uuid4().hex[:8]}"
    print(f"S3 session: s3://{S3_BUCKET_NAME}/sessions/{session_id}")

    return S3SessionManager(
        session_id=session_id,
        bucket=S3_BUCKET_NAME,
        prefix="sessions",
        boto_session=boto_session,
    )


def run(model, trace_attrs: dict):
    """Run the agent_core sample."""
    # Create shared boto3 session for all AWS clients
    boto_session = boto3.Session(region_name=AWS_REGION)
    agentcore = boto_session.client("bedrock-agentcore")

    if not AGENT_CORE_MEMORY_ID or not AGENT_CORE_MEMORY_STRATEGY_ID:
        print_setup_help(agentcore, AGENT_CORE_MEMORY_ID)
        if not AGENT_CORE_MEMORY_ID:
            raise ValueError("AGENT_CORE_MEMORY_ID environment variable is required")
        raise ValueError("AGENT_CORE_MEMORY_STRATEGY_ID environment variable is required")

    namespace = "default"
    memory_text = "User's favorite number is 7"

    # Store memory only if it doesn't already exist
    if memory_exists(
        agentcore,
        AGENT_CORE_MEMORY_ID,
        AGENT_CORE_MEMORY_STRATEGY_ID,
        "favorite number",
        namespace,
    ):
        print("Memory already exists, skipping creation")
    else:
        print("Storing memory...")
        result = create_memory_record(
            client=agentcore,
            memory_id=AGENT_CORE_MEMORY_ID,
            strategy_id=AGENT_CORE_MEMORY_STRATEGY_ID,
            text=memory_text,
            namespace=namespace,
        )
        record_id = result["successfulRecords"][0]["memoryRecordId"]
        print(f"Memory stored: {record_id}")

    # Create S3 session manager for conversation persistence
    session_manager = create_session_manager(boto_session)

    # Create agent with memory and code interpreter tools
    memory_tool = AgentCoreMemoryToolProvider(
        memory_id=AGENT_CORE_MEMORY_ID,
        actor_id="demo-user",
        session_id="demo-session",
        namespace=namespace,
        region=AWS_REGION,
    )
    code_tool = AgentCoreCodeInterpreter(region=AWS_REGION)

    agent = Agent(
        model=model,
        tools=[memory_tool.tools, code_tool.code_interpreter],
        system_prompt=SYSTEM_PROMPT,
        trace_attributes=trace_attrs,
        session_manager=session_manager,
    )

    # Query the agent - it should retrieve memory and calculate
    print("\nQuerying agent...")
    prompt = "Calculate my favorite number squared."
    print(f"Prompt: {prompt}")

    response = agent(prompt)
    print(f"Response: {response.message['content'][0]['text']}")
