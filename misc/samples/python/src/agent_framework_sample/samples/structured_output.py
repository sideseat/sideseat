"""Pydantic structured output sample."""

from typing import Optional

from agent_framework import Agent
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

    agent = Agent(
        client=client,
        instructions="You are a helpful assistant that extracts structured information from text.",
    )

    prompt = (
        "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
        "New York, USA. Email: jane@example.com"
    )

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        result = await agent.run(prompt, options={"response_format": Person})

        if result.value is not None:
            print(f"Parsed Person: {result.value}")
        else:
            print(f"Raw response: {result.text}")
