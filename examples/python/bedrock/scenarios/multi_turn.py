from typing import Any

from tools import answer as text_of

from harness import Run, content


def run(run: Run) -> None:
    # The application keeps the history and re-sends it with every request.
    messages: list[dict[str, Any]] = []
    with run.trace():
        for question in content.MULTI_TURN:
            messages.append({"role": "user", "content": [{"text": question}]})
            response = run.llm.client.converse(
                modelId=run.llm.model_id,
                system=[{"text": content.SYSTEM}],
                messages=messages,
            )
            answer = response["output"]["message"]
            messages.append(answer)
            print(text_of(answer))
