from harness import Run, content


def run(run: Run) -> None:
    # Current Claude models reject both Converse's native output format and a forced tool choice, so the
    # schema is offered as a tool the model is free to call, which it does to answer.
    schema = content.TripPlan.model_json_schema()
    tool = {
        "toolSpec": {
            "name": "TripPlan",
            "description": content.TripPlan.__doc__ or "",
            "inputSchema": {"json": schema},
        }
    }
    with run.trace():
        response = run.llm.client.converse(
            modelId=run.llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=[{"role": "user", "content": [{"text": content.STRUCTURED}]}],
            toolConfig={"tools": [tool], "toolChoice": {"auto": {}}},
        )
        blocks = response["output"]["message"]["content"]
        plan = next(block["toolUse"]["input"] for block in blocks if "toolUse" in block)
        print(content.TripPlan.model_validate(plan))
