import json
from typing import Any

from harness import Run, content


def _closed(schema: dict[str, Any]) -> dict[str, Any]:
    # Bedrock's structured output accepts only object schemas that forbid extra properties.
    return {**schema, "additionalProperties": False}


def run(run: Run) -> None:
    schema = _closed(content.TripPlan.model_json_schema())
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=[{"role": "user", "content": [{"text": content.STRUCTURED}]}],
            outputConfig={
                "textFormat": {
                    "type": "json_schema",
                    "structure": {
                        "jsonSchema": {
                            "name": "TripPlan",
                            "description": content.TripPlan.__doc__ or "",
                            "schema": json.dumps(schema),
                        }
                    },
                }
            },
        )
        text = response["output"]["message"]["content"][0]["text"]
        print(content.TripPlan.model_validate_json(text))
