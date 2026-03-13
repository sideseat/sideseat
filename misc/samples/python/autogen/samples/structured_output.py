"""Pydantic structured output sample."""

import json
from typing import Optional

from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import MaxMessageTermination
from autogen_agentchat.teams import RoundRobinGroupChat
from opentelemetry import trace
from pydantic import BaseModel, Field


class Address(BaseModel):
    street: str
    city: str
    country: str
    postal_code: Optional[str] = None


class Contact(BaseModel):
    email: Optional[str] = None
    phone: Optional[str] = None


class Person(BaseModel):
    """Complete person information."""

    name: str = Field(description="Full name of the person")
    age: int = Field(description="Age in years")
    address: Address = Field(description="Home address")
    contacts: list[Contact] = Field(default_factory=list, description="Contact methods")
    skills: list[str] = Field(default_factory=list, description="Professional skills")


SYSTEM_PROMPT = f"""You are a helpful assistant that extracts structured information from text.
When asked to extract information, respond ONLY with valid JSON matching this schema:

{json.dumps(Person.model_json_schema(), indent=2)}

Do not include any text before or after the JSON. Only output the JSON object."""


async def run(model_client, trace_attrs: dict):
    """Run the structured_output sample."""
    tracer = trace.get_tracer(__name__)

    agent = AssistantAgent(
        name="extraction_assistant",
        model_client=model_client,
        system_message=SYSTEM_PROMPT,
    )

    termination = MaxMessageTermination(max_messages=3)
    team = RoundRobinGroupChat([agent], termination_condition=termination)

    prompt = (
        "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
        "New York, USA. Email: jane@example.com"
    )

    with tracer.start_as_current_span(
        "autogen.session",
        attributes=trace_attrs,
    ):
        result = await team.run(task=prompt)

        # Extract the agent's response
        for message in result.messages:
            if hasattr(message, "content") and message.content:
                if hasattr(message, "source") and message.source == agent.name:
                    try:
                        # Parse the JSON response into our Pydantic model
                        person = Person.model_validate_json(message.content)
                        print(f"Parsed Person: {person}")
                    except Exception:
                        # If parsing fails, just print the raw response
                        print(f"Raw response: {message.content}")

    await model_client.close()
