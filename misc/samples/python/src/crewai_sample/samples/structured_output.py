"""Pydantic structured output sample."""

from typing import Optional

from crewai import Agent, Crew, Process, Task
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


def run(llm, trace_attrs: dict):
    """Run the structured_output sample."""
    tracer = trace.get_tracer(__name__)

    extraction_agent = Agent(
        role="Information Extractor",
        goal="Extract structured information from text accurately",
        backstory="You are an expert at parsing text and extracting structured data. You always return complete and accurate information.",
        llm=llm,
        verbose=False,
    )

    extraction_task = Task(
        description=(
            "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
            "New York, USA. Email: jane@example.com"
        ),
        expected_output="A structured Person object with all extracted information.",
        agent=extraction_agent,
        output_pydantic=Person,
    )

    crew = Crew(
        agents=[extraction_agent],
        tasks=[extraction_task],
        process=Process.sequential,
        verbose=False,
        share_crew=False,
    )

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        result = crew.kickoff()
        print(f"Parsed Person: {result.pydantic}")
