"""Pydantic structured output sample."""

from typing import Optional

from pydantic import BaseModel, Field
from strands import Agent


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


def run(model, trace_attrs: dict):
    """Run the structured_output sample."""
    agent = Agent(
        model=model,
        trace_attributes=trace_attrs,
    )

    result = agent.structured_output(
        Person,
        (
            "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
            "New York, USA. Email: jane@example.com"
        ),
    )

    print(result)
