"""Pydantic structured output sample.

Demonstrates:
- Defining Pydantic models for structured extraction
- Using with_structured_output() for type-safe LLM responses
- Nested model structures (Person with Address and Contact)
- Field descriptions for better extraction guidance
"""

from typing import Optional

from pydantic import BaseModel, Field, ValidationError


class Address(BaseModel):
    """Physical address information."""

    street: str = Field(description="Street address including number")
    city: str = Field(description="City name")
    country: str = Field(description="Country name")
    postal_code: Optional[str] = Field(default=None, description="Postal/ZIP code")


class Contact(BaseModel):
    """Contact information."""

    email: Optional[str] = Field(default=None, description="Email address")
    phone: Optional[str] = Field(default=None, description="Phone number")


class Person(BaseModel):
    """Complete person information for extraction."""

    name: str = Field(description="Full name of the person")
    age: int = Field(description="Age in years")
    address: Address = Field(description="Home address")
    contacts: list[Contact] = Field(default_factory=list, description="Contact methods")
    skills: list[str] = Field(default_factory=list, description="Professional skills")


def run(model, trace_attrs: dict):
    """Run the structured_output sample demonstrating Pydantic extraction.

    This sample shows:
    - Defining nested Pydantic models for structured data
    - Using with_structured_output() to constrain LLM output
    - Extracting typed data from natural language descriptions

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    # Wrap model with structured output capability
    structured_model = model.with_structured_output(Person)

    prompt = (
        "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
        "New York, USA. Email: jane@example.com"
    )

    try:
        result = structured_model.invoke(prompt)
    except ValidationError as e:
        print(f"[Validation Error: {e}]")
        return
    except Exception as e:
        print(f"[Error: {e}]")
        return

    # Display extracted data
    print("Extracted Person:")
    print(f"  Name: {result.name}")
    print(f"  Age: {result.age}")
    print(f"  Address: {result.address.street}, {result.address.city}, {result.address.country}")

    if result.address.postal_code:
        print(f"  Postal Code: {result.address.postal_code}")

    if result.contacts:
        contact = result.contacts[0]
        if contact.email:
            print(f"  Email: {contact.email}")
        if contact.phone:
            print(f"  Phone: {contact.phone}")

    if result.skills:
        print(f"  Skills: {', '.join(result.skills)}")

    print()
    print(f"Raw result: {result}")
