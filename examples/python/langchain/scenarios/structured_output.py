from langchain_core.prompts import ChatPromptTemplate

from harness import Run, content


async def run(run: Run) -> None:
    prompt = ChatPromptTemplate.from_messages(
        [("system", content.SYSTEM), ("human", "{request}")]
    )
    # The schema is offered as a tool. Current Claude models reject both forced tool choice and
    # Bedrock's native output format, so the tool is offered with automatic choice.
    chain = prompt | run.llm.with_structured_output(content.TripPlan)
    with run.trace():
        print(await chain.ainvoke({"request": content.STRUCTURED}))
