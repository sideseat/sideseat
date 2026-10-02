from strands import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, system_prompt=content.SYSTEM, callback_handler=None)
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
        print(agent(prompt))
