from harness import Run, content


def run(run: Run) -> None:
    deployment = run.llm
    with run.trace():
        # Reasoning summaries come back only from the Responses API.
        response = deployment.client.responses.create(
            model=deployment.id,
            instructions=content.SYSTEM,
            input=content.REASONING,
            reasoning={"effort": "high", "summary": "detailed"},
        )
        print(response.output_text)
