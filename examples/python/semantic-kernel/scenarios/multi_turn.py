from semantic_kernel.agents import AgentThread, ChatCompletionAgent

from harness import Run, content


async def run(run: Run) -> None:
    agent = ChatCompletionAgent(
        service=run.llm, name="assistant", instructions=content.SYSTEM
    )
    # The thread holds the conversation, so each request re-sends the earlier turns.
    thread: AgentThread | None = None
    with run.trace():
        for question in content.MULTI_TURN:
            response = await agent.get_response(messages=question, thread=thread)
            thread = response.thread
            print(response)
