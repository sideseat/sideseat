"""The shared example tools as Pydantic AI tools."""

from pydantic_ai import ModelRetry, Tool

from harness import content

get_weather = Tool(content.get_weather)
get_precipitation = Tool(content.get_precipitation)


def book_flight(origin: str, destination: str, date: str) -> str:
    """Book a flight.

    Args:
        origin: Departure city.
        destination: Arrival city.
        date: Travel date.
    """
    # Pydantic AI ends the run on any other exception; ModelRetry is how a tool reports a failure to
    # the model.
    try:
        return content.book_flight(origin, destination, date)
    except content.BookingUnavailable as error:
        raise ModelRetry(str(error)) from error
