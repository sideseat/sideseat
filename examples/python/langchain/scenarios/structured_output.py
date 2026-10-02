from langchain_core.prompts import ChatPromptTemplate

from harness import Run, content


async def run(run: Run) -> None:
    prompt = ChatPromptTemplate.from_messages(
        [("system", content.SYSTEM), ("human", "{request}")]
    )
    # Bedrock's native structured output: forced tool choice, the function-calling method, is not
    # available on current Claude models.
    chain = prompt | run.llm.with_structured_output(
        content.TripPlan, method="json_schema"
    )
    with run.trace():
        print(await chain.ainvoke({"request": content.STRUCTURED}))
