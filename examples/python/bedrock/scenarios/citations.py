from tools import answer

from harness import Run, content


def run(run: Run) -> None:
    prompt = [
        {"text": content.CITATIONS},
        {
            "document": {
                "format": "pdf",
                "name": "task",
                "source": {"bytes": run.asset("task.pdf").read_bytes()},
                "citations": {"enabled": True},
            }
        },
    ]
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=[{"role": "user", "content": prompt}],
        )
        print(answer(response["output"]["message"]))
