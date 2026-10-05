from tools import system

from harness import Run, content


def run(run: Run) -> None:
    model = run.llm
    with run.trace():
        # `parse` derives a strict JSON schema from the model class and validates the answer.
        response = model.client.chat.completions.parse(
            model=model.id,
            messages=[system(), {"role": "user", "content": content.STRUCTURED}],
            response_format=content.TripPlan,
        )
        print(response.choices[0].message.parsed)
