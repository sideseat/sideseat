from agent import options
from claude_agent_sdk import ResultMessage, StreamEvent, query
from tools import get_weather, server

from harness import Run, content


async def run(run: Run) -> None:
    servers, allowed = server(get_weather)
    settings = options(
        run, mcp_servers=servers, allowed_tools=allowed, include_partial_messages=True
    )
    with run.trace():
        async for message in query(prompt=content.STREAMING, options=settings):
            if isinstance(message, StreamEvent):
                delta = message.event.get("delta", {})
                if delta.get("type") == "text_delta":
                    print(delta["text"], end="", flush=True)
            elif isinstance(message, ResultMessage) and message.is_error:
                raise RuntimeError(f"the agent run did not succeed: {message}")
        print()
