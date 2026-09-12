"""Pydantic structured output sample."""

from typing import Optional

from agents import Agent, Runner
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


def run(model_id: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the structured_output sample."""
    tracer = trace.get_tracer(__name__)

    agent = Agent(
        name="InformationExtractor",
        model=model_id,
        instructions="You are a helpful assistant that extracts structured information from text.",
        output_type=Person,
    )

    prompt = (
        "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
        "New York, USA. Email: jane@example.com"
    )

    with tracer.start_as_current_span(
        "openai_agents.session",
        attributes=trace_attrs,
    ):
        result = Runner.run_sync(agent, prompt)
        print(f"Parsed Person: {result.final_output}")
