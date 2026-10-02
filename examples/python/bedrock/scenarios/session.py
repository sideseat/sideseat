from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            response = run.llm.client.converse(
                modelId=run.llm.model_id,
                system=[{"text": content.SYSTEM}],
                messages=[{"role": "user", "content": [{"text": question}]}],
            )
            print(response["output"]["message"]["content"][0]["text"])
