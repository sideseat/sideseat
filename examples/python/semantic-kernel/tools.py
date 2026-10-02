"""The shared example tools as Semantic Kernel plugins."""

from collections.abc import Callable

from semantic_kernel.functions import KernelPlugin, kernel_function

from harness import content


def plugin(*functions: Callable[..., object]) -> KernelPlugin:
    return KernelPlugin(
        name="travel",
        description="Weather and booking tools.",
        functions=[kernel_function(function) for function in functions],
    )


weather = plugin(content.get_weather, content.get_precipitation)
forecast = plugin(content.get_weather)
booking = plugin(content.book_flight)
