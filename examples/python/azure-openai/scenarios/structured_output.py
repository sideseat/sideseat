from tools import system

from harness import Run, content


def run(run: Run) -> None:
    deployment = run.llm
    with run.trace():
        # `parse` derives a strict JSON schema from the model class and validates the answer.
        response = deployment.client.chat.completions.parse(
            model=deployment.id,
            messages=[system(), {"role": "user", "content": content.STRUCTURED}],
            response_format=content.TripPlan,
        )
        print(response.choices[0].message.parsed)
