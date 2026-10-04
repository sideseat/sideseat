from models import THINKING
from tools import converse

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = converse(
            run.llm,
            [{"role": "user", "content": content.REASONING}],
            thinking=THINKING,
            output_config={"effort": "max"},
        )
        print(answer)
