from harness import Run, content


def run(run: Run) -> None:
    prompt = [
        {"text": content.FILES},
        {
            "image": {
                "format": "jpeg",
                "source": {"bytes": run.asset("img.jpg").read_bytes()},
            }
        },
        {
            "document": {
                "format": "pdf",
                "name": "task",
                "source": {"bytes": run.asset("task.pdf").read_bytes()},
            }
        },
    ]
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=[{"role": "user", "content": prompt}],
        )
        print(response["output"]["message"]["content"][0]["text"])
