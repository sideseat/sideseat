"""Pydantic structured output sample."""

from typing import Optional

from agent_framework import ChatAgent
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


async def run(client, trace_attrs: dict):
    """Run the structured_output sample."""
    tracer = trace.get_tracer(__name__)

    agent = ChatAgent(
        chat_client=client,
        instructions="You are a helpful assistant that extracts structured information from text.",
    )

    prompt = (
        "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
        "New York, USA. Email: jane@example.com"
    )

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        result = await agent.run(prompt, response_format=Person)

        parsed = result.try_parse_value(Person)
        if parsed is not None:
            print(f"Parsed Person: {parsed}")
        else:
            print(f"Raw response: {result.text}")
