"""The prompts and tools every suite uses, so all frameworks hold the same conversations.

Tools are plain functions with docstrings; each suite wraps them with its framework's tool decorator.
Their results are deterministic so a recapture differs only where the model's wording does.
"""

from __future__ import annotations

from pydantic import BaseModel, Field

SYSTEM = (
    "You are a concise travel assistant. Use the tools for weather and bookings instead of "
    "guessing, and answer in at most three sentences."
)

CHAT = "In one sentence, what is Kyoto best known for?"

MULTI_TURN = (
    "I am planning a weekend in Lisbon. Name one neighbourhood to stay in.",
    "What is one dish I should try there?",
    "Summarise your two suggestions in one sentence.",
)

TOOL_USE = (
    "What will the weather be in Paris and in Tokyo over the next two days, and should I pack an "
    "umbrella for either city?"
)

SESSION = (
    "Suggest one museum to visit in Madrid.",
    "Suggest one park to visit in Madrid.",
)

ERROR = "Book me a flight from London to Oslo on 2026-11-14."

STREAMING = "Check the weather in Rome for the next day and tell me what to wear."

STRUCTURED = "Plan a two-day trip to Vienna."

#: A turn that ends on a tool result: the agent runs the tool and stops, so nothing re-sends what the tool
#: returned. What a producer's telemetry does with the last result of a conversation is only visible here.
TRAILING_TOOL = (
    "Look up the weather in Oslo for tomorrow. Report only the tool's result, verbatim."
)

REASONING = (
    "Four travellers must cross a bridge at night with one torch. At most two cross at a time and a "
    "pair moves at the slower person's pace. They take 1, 2, 5 and 10 minutes. What is the fastest "
    "total crossing time, and what is the schedule?"
)

FILES = (
    "Describe the image in one sentence, then summarise the document in one sentence."
)

#: Attachments sent by reference rather than as bytes: an image by its URL and a document by the id an
#: upload gave it, so a reconstruction can show only where each one is.
FILE_REFERENCES = "Describe the linked image in one sentence, then summarise the uploaded document in one sentence."

#: The image the ``file_references`` scenario links to. Only a live provider would fetch it.
IMAGE_URL = "https://upload.wikimedia.org/wikipedia/commons/a/a8/Tour_Eiffel_Wikimedia_Commons.jpg"

#: Answered from a document sent with citations enabled: a provider that cites returns the passages its
#: answer rests on beside the answer.
CITATIONS = "Using only the attached document, say in one sentence what it asks the reader to do."

MULTI_AGENT = (
    "Research the weather in Barcelona for the next two days, then write a one-paragraph packing "
    "list based on it."
)

MCP = "Use the calculator to compute (17 * 23) + 4, then report the result."

#: Answered with a tool the provider runs itself: the request declares no function, only the
#: provider's own web search, and the reply holds the search beside the answer.
SERVER_TOOLS = (
    "Search the web for the Louvre's opening hours, then answer in one sentence."
)


class TripPlan(BaseModel):
    """A short itinerary."""

    city: str = Field(description="The destination city")
    days: list[str] = Field(description="One activity per day")
    budget_eur: int = Field(description="Estimated total budget in euros")


def get_weather(city: str, days: int = 1) -> dict[str, object]:
    """Get the weather forecast for a city.

    Args:
        city: The city name.
        days: How many days to forecast, from 1 to 7.
    """
    rainy = city.strip().lower() in {"tokyo", "london", "oslo", "barcelona"}
    return {
        "city": city,
        "forecast": [
            {
                "day": day + 1,
                "condition": "rain" if rainy and day == 0 else "sunny",
                "high_c": 21 + day,
            }
            for day in range(max(1, min(days, 7)))
        ],
    }


def get_precipitation(city: str) -> str:
    """Get the chance of rain in a city for tomorrow.

    Args:
        city: The city name.
    """
    chance = (
        80 if city.strip().lower() in {"tokyo", "london", "oslo", "barcelona"} else 10
    )
    return f"{chance}% chance of rain in {city} tomorrow."


class BookingUnavailable(RuntimeError):
    pass


def book_flight(origin: str, destination: str, date: str) -> str:
    """Book a flight.

    Args:
        origin: Departure city.
        destination: Arrival city.
        date: Travel date.
    """
    raise BookingUnavailable(
        f"No seats from {origin} to {destination} on {date}: the booking system is offline."
    )
