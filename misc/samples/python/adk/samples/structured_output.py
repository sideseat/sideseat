"""Pydantic structured output sample.

Uses Google ADK's native output_schema parameter for structured JSON responses.
Note: When output_schema is set, tools are disabled on the agent.
"""

from typing import Optional

from google.adk.agents import LlmAgent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types
from pydantic import BaseModel, Field

APP_NAME = "extraction_app"


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


SYSTEM_PROMPT = """You are a helpful assistant that extracts structured information from text.
Extract all relevant details including name, age, address, contact information, and skills.
Return the information in the exact format specified by the output schema."""


async def run(model, trace_attrs: dict):
    """Run the structured_output sample."""
    # Use output_schema for native structured output enforcement
    agent = LlmAgent(
        model=model,
        name="extraction_assistant",
        instruction=SYSTEM_PROMPT,
        output_schema=Person,  # ADK enforces JSON output matching this schema
    )

    session_service = InMemorySessionService()
    session = await session_service.create_session(
        app_name=APP_NAME,
        user_id="demo-user",
        session_id=trace_attrs["session.id"],
    )

    runner = Runner(
        agent=agent,
        app_name=APP_NAME,
        session_service=session_service,
    )

    prompt = (
        "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
        "New York, USA. Email: jane@example.com"
    )

    user_message = types.Content(
        role="user",
        parts=[types.Part(text=prompt)],
    )

    async for event in runner.run_async(
        session_id=session.id,
        user_id="demo-user",
        new_message=user_message,
    ):
        if event.content and event.content.parts:
            for part in event.content.parts:
                if hasattr(part, "text") and part.text:
                    try:
                        # Parse the JSON response into our Pydantic model
                        person = Person.model_validate_json(part.text)
                        print(f"Parsed Person: {person}")
                    except Exception:
                        # If parsing fails, just print the raw response
                        print(f"Raw response: {part.text}")
