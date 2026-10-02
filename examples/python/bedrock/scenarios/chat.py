from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=[{"role": "user", "content": [{"text": content.CHAT}]}],
        )
        print(response["output"]["message"]["content"][0]["text"])
