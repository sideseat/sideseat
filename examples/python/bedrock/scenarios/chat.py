from tools import answer

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=[{"role": "user", "content": [{"text": content.CHAT}]}],
        )
        print(answer(response["output"]["message"]))
