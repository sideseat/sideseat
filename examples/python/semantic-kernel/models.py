"""Maps a harness model alias to a Semantic Kernel chat completion service."""

from typing import Any

import boto3
from botocore.config import Config
from semantic_kernel.connectors.ai.bedrock import BedrockChatCompletion
from semantic_kernel.contents import ChatHistory, ChatMessageContent
from semantic_kernel.contents.streaming_chat_message_content import (
    StreamingChatMessageContent,
)
from semantic_kernel.contents.utils.author_role import AuthorRole

from harness import Model
from harness.models import region


class ClaudeOnBedrock(BedrockChatCompletion):
    """Semantic Kernel's Bedrock connector, adjusted for what current Claude models send and require.

    The models may answer with a reasoning block, which the connector rejects as an unsupported content
    type, and they call tools in parallel, whose results the connector sends as one user message each
    where Converse requires them together. The reasoning blocks are left out of what Semantic Kernel
    sees, and consecutive tool results are joined into one message.
    """

    # Restated so type checkers see the connector's constructor rather than the model fields pydantic
    # derives for a subclass.
    def __init__(self, *, model_id: str, runtime_client: Any) -> None:
        super().__init__(model_id=model_id, runtime_client=runtime_client)

    def _prepare_chat_history_for_request(
        self,
        chat_history: ChatHistory,
        role_key: str = "role",
        content_key: str = "content",
    ) -> Any:
        merged: list[dict[str, Any]] = []
        for message in super()._prepare_chat_history_for_request(
            chat_history, role_key, content_key
        ):
            if merged and _is_tool_results(message) and _is_tool_results(merged[-1]):
                merged[-1]["content"].extend(message["content"])
            else:
                merged.append(message)
        return merged

    def _create_chat_message_content(
        self, response: dict[str, Any]
    ) -> ChatMessageContent:
        message = response["output"]["message"]
        message["content"] = [
            block for block in message["content"] if "reasoningContent" not in block
        ]
        return super()._create_chat_message_content(response)

    def _parse_content_block_delta_event(
        self, event: dict[str, Any], function_invoke_attempt: int
    ) -> StreamingChatMessageContent:
        if "reasoningContent" in event["contentBlockDelta"]["delta"]:
            return StreamingChatMessageContent(
                ai_model_id=self.ai_model_id,
                role=AuthorRole.ASSISTANT,
                items=[],
                choice_index=0,
                inner_content=event,
                function_invoke_attempt=function_invoke_attempt,
            )
        return super()._parse_content_block_delta_event(event, function_invoke_attempt)


def _is_tool_results(message: dict[str, Any]) -> bool:
    return message["role"] == "user" and all(
        "toolResult" in block for block in message["content"]
    )


def build(model: Model) -> BedrockChatCompletion:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the Semantic Kernel suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    return ClaudeOnBedrock(
        model_id=model.id,
        # A long answer can outlast botocore's 60-second read timeout, which retries a request the model
        # is still answering.
        runtime_client=boto3.client(
            "bedrock-runtime", region_name=region(), config=Config(read_timeout=600)
        ),
    )
