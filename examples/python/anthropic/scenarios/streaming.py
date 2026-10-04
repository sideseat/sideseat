from typing import Any

from models import MAX_TOKENS
from tools import assistant_turn, definitions, results

from harness import Run, content


def run(run: Run) -> None:
    claude = run.llm
    messages: list[dict[str, Any]] = [{"role": "user", "content": content.STREAMING}]
    with run.trace():
        while True:
            with claude.client.messages.stream(
                model=claude.model,
                system=content.SYSTEM,
                max_tokens=MAX_TOKENS,
                messages=messages,
                tools=definitions("get_weather"),
            ) as stream:
                for text in stream.text_stream:
                    print(text, end="", flush=True)
                response = stream.get_final_message()
            messages.append(assistant_turn(response))
            if response.stop_reason != "tool_use":
                break
            messages.append({"role": "user", "content": results(response.content)})
        print()
