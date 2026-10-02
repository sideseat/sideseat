from langchain_core.output_parsers import StrOutputParser
from langchain_core.prompts import ChatPromptTemplate

from harness import Run, content


async def run(run: Run) -> None:
    prompt = ChatPromptTemplate.from_messages(
        [("system", content.SYSTEM), ("human", "{question}")]
    )
    chain = prompt | run.llm | StrOutputParser()
    with run.trace():
        print(await chain.ainvoke({"question": content.CHAT}))
