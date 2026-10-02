from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            messages=[{"role": "user", "content": [{"text": content.REASONING}]}],
            inferenceConfig={"maxTokens": 16_000},
            # Current Claude models think by default and omit the reasoning text; this asks for a summary
            # so the telemetry carries visible reasoning.
            additionalModelRequestFields={
                "thinking": {"type": "adaptive", "display": "summarized"}
            },
            outputConfig={"effort": "max"},
        )
        for block in response["output"]["message"]["content"]:
            if "text" in block:
                print(block["text"])
