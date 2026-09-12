"""Image generation and critic evaluation sample."""

from strands import Agent
from strands_tools import generate_image, image_reader


def run(model, trace_attrs: dict):
    """Run the image_gen sample."""
    # Artist agent that generates images based on prompts
    artist = Agent(
        model=model,
        tools=[generate_image],
        system_prompt=(
            "You will be instructed to generate a number of images of a given subject. "
            "Vary the prompt for each generated image to create a variety of options. "
            "Your final output must contain ONLY a comma-separated list of the filesystem paths of generated images."
        ),
        trace_attributes=trace_attrs,
    )

    # Critic agent that evaluates and selects the best image
    critic = Agent(
        model=model,
        tools=[image_reader],
        system_prompt=(
            "You will be provided with a list of filesystem paths, each containing an image. "
            "Describe each image, and then choose which one is best. "
            "Your final line of output must be as follows: "
            "FINAL DECISION: <path to final decision image>"
        ),
        trace_attributes=trace_attrs,
    )

    # Generate multiple images using the artist agent
    result = artist("Generate 3 images of a dog")
    print(f"Artist result: {result}")

    # Pass the image paths to the critic agent for evaluation
    result = critic(str(result))
    print(f"Critic result: {result}")
