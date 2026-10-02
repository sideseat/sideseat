import json
from typing import Any

from tools import call, spec

from harness import Run, content


def _stream(run: Run, messages: list[dict[str, Any]]) -> dict[str, Any]:
    """Streams one ConverseStream response to stdout and returns it as a message."""
    response = run.llm.client.converse_stream(
        modelId=run.llm.model_id,
        system=[{"text": content.SYSTEM}],
        messages=messages,
        toolConfig={"tools": [spec(content.get_weather)]},
    )
    blocks: list[dict[str, Any]] = []
    tool_input = ""
    for event in response["stream"]:
        if start := event.get("contentBlockStart", {}).get("start", {}).get("toolUse"):
            blocks.append({"toolUse": {**start, "input": {}}})
            tool_input = ""
        elif delta := event.get("contentBlockDelta", {}).get("delta"):
            if "text" in delta:
                if not blocks or "text" not in blocks[-1]:
                    blocks.append({"text": ""})
                blocks[-1]["text"] += delta["text"]
                print(delta["text"], end="", flush=True)
            elif "toolUse" in delta:
                tool_input += delta["toolUse"]["input"]
            elif reasoning := delta.get("reasoningContent"):
                # Thinking blocks go back with the tool results, as Claude requires.
                if not blocks or "reasoningContent" not in blocks[-1]:
                    blocks.append({"reasoningContent": {"reasoningText": {"text": ""}}})
                text = blocks[-1]["reasoningContent"]["reasoningText"]
                text["text"] += reasoning.get("text", "")
                if "signature" in reasoning:
                    text["signature"] = reasoning["signature"]
        elif "contentBlockStop" in event and blocks and "toolUse" in blocks[-1]:
            blocks[-1]["toolUse"]["input"] = json.loads(tool_input or "{}")
    return {"role": "assistant", "content": blocks}


def run(run: Run) -> None:
    messages: list[dict[str, Any]] = [
        {"role": "user", "content": [{"text": content.STREAMING}]}
    ]
    with run.trace():
        while True:
            message = _stream(run, messages)
            messages.append(message)
            uses = [b["toolUse"] for b in message["content"] if "toolUse" in b]
            if not uses:
                break
            tools = {"get_weather": content.get_weather}
            messages.append(
                {"role": "user", "content": [call(tools, use) for use in uses]}
            )
        print()
