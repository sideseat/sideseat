vec![
        // The richer copy joined on by position: the flattened form redacts a url, and the serialised copy
        // of the same conversation on the same span keeps it. The list opens with a member whose `id` is a
        // *string*, so the witness - "some member states a class path, which this dialect writes as an
        // array" - has to be asked of every match. Asked of the first only, the overlay would not fire and
        // the redacted url would stand, which is what the retired code's `.any()` avoided.
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "user"),
                (
                    "llm.input_messages.0.message.contents.0.message_content.type",
                    "text",
                ),
                (
                    "llm.input_messages.0.message.contents.0.message_content.text",
                    "look",
                ),
                ("llm.input_messages.1.message.role", "user"),
                (
                    "llm.input_messages.1.message.contents.0.message_content.type",
                    "image",
                ),
                (
                    "llm.input_messages.1.message.contents.0.message_content.image.image.url",
                    "__REDACTED__",
                ),
                (
                    "input.value",
                    r#"{"messages": [[{"id": "a string, not a class path", "content": "first"}, {"id": ["langchain", "schema", "messages", "HumanMessage"], "content": [{"type": "text", "text": "look"}, {"type": "image_url", "image_url": {"url": "https://real/img.png"}}]}]]}"#,
                ),
            ]),
        ),
        // The OpenInference dialect: indexed message families, retrieval result sets, and the two
        // single-attribute carriers.
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "user"),
                (
                    "llm.input_messages.0.message.content",
                    "what is the weather",
                ),
            ]),
        ),
        // A role with nothing to show is not a turn: an index exists as soon as any key mentions it.
        (
            "span",
            rule_attrs(&[("llm.input_messages.0.message.role", "user")]),
        ),
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "user"),
                (
                    "llm.input_messages.0.message.contents.0.message_content.type",
                    "text",
                ),
                (
                    "llm.input_messages.0.message.contents.0.message_content.text",
                    "see this",
                ),
                ("llm.input_messages.1.message.role", "assistant"),
                (
                    "llm.input_messages.1.message.tool_calls.0.tool_call.id",
                    "c1",
                ),
                (
                    "llm.input_messages.1.message.tool_calls.0.tool_call.function.name",
                    "search",
                ),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "tool"),
                ("llm.input_messages.0.message.tool_call_id", "c1"),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "assistant"),
                ("llm.input_messages.0.message.function_call.name", "legacy"),
            ]),
        ),
        // An entry-level member beside the nested message, and a JSON-valued one.
        (
            "span",
            rule_attrs(&[
                ("llm.output_messages.0.message.role", "assistant"),
                ("llm.output_messages.0.message.content", "sunny"),
                ("llm.output_messages.0.finish_reason", "stop"),
                ("llm.output_messages.0.message.extra", r#"{"a":1}"#),
            ]),
        ),
        // Retrieval: one message holding every document, not one each.
        (
            "span",
            rule_attrs(&[
                ("retrieval.documents.0.document.id", "d1"),
                ("retrieval.documents.0.document.content", "first"),
                ("retrieval.documents.0.document.score", "0.9"),
                ("retrieval.documents.0.document.metadata", r#"{"src":"kb"}"#),
                ("retrieval.documents.1.document.content", "second"),
                ("retrieval.documents.1.document.score", "not a number"),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("reranker.input_documents.0.document.content", "a"),
                ("reranker.output_documents.0.document.content", "a"),
                ("reranker.query", "which is best"),
            ]),
        ),
        ("span", rule_attrs(&[("embedding.text", "vectorise me")])),
        // The AutoGen dialect, whose messages are typed objects: one case per type, the four
        // selection points it writes them at, and its logging channel.
        // The typed table, one case each.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "SystemMessage", "content": "be brief"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "UserMessage", "content": "hi", "source": "user"}"#,
            )]),
        ),
        // Reasoning beside the reply becomes a thinking block ahead of it.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "AssistantMessage", "content": "ok", "source": "planner", "thought": "thinking it over"}"#,
            )]),
        ),
        // A null thought is not a thought.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "AssistantMessage", "content": "ok", "source": "planner", "thought": null}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "AssistantMessage", "content": "ok"}"#,
            )]),
        ),
        // `source` names the speaker, so anything but the user is the assistant.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "TextMessage", "content": "hello", "source": "user"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "TextMessage", "content": "hello", "source": "planner"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"type": "TextMessage", "content": "hello"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "MultiModalMessage", "content": [{"type": "text", "text": "see"}], "source": "critic"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "StopMessage", "content": "done", "source": "user"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "HandoffMessage", "content": "over to you", "source": "planner"}"#,
            )]),
        ),
        // A non-string speaker names nobody.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "HandoffMessage", "content": "over to you", "source": {"not": "a string"}}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ThoughtEvent", "content": "pondering"}"#,
            )]),
        ),
        // An empty thought is not one.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"type": "ThoughtEvent", "content": ""}"#)]),
        ),
        // Arguments arrive as serialised JSON as often as an object.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallRequestEvent", "source": "planner", "content": [{"id": "c1", "name": "search", "arguments": "{\"q\":\"x\"}"}]}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallRequestEvent", "content": [{"id": "c1", "name": "search", "arguments": {"q": "x"}}]}"#,
            )]),
        ),
        // A call with no id pairs with nothing, so the reading finds none.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallRequestEvent", "content": [{"name": "nameless"}]}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallExecutionEvent", "content": [{"call_id": "c1", "name": "search", "content": "found"}]}"#,
            )]),
        ),
        // The batch's call id is a fallback for a result that carries none.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "FunctionExecutionResultMessage", "call_id": "c9", "content": [{"name": "search", "content": "found"}, {"content": "second"}]}"#,
            )]),
        ),
        // A result with nothing in it at all.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallExecutionEvent", "content": [{}]}"#,
            )]),
        ),
        // Claimed and read for nothing: the results are already reported individually.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallSummaryMessage", "content": "repr noise"}"#,
            )]),
        ),
        // A type the table does not know still carries a message.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "SomethingNew", "content": "unknown but usable"}"#,
            )]),
        ),
        // ...and one that does not carries none.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"type": "SomethingNew", "source": "x"}"#)]),
        ),
        // A list, including shapes the table refuses that this point still reads.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"messages": [{"type": "TextMessage", "content": "a", "source": "user"}, {"content": ""}, {"content": "loose"}], "output_task_messages": true}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"message": {"type": "ToolCallRequestEvent", "content": [{"id": "c2", "name": "t", "arguments": {}}]}}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"message": {"content": "loose"}}"#)]),
        ),
        // A response carries its reply *and* the turns that produced it.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"response": {"chat_message": {"type": "TextMessage", "content": "final", "source": "planner"}, "inner_messages": [{"type": "ThoughtEvent", "content": "first"}]}}"#,
            )]),
        ),
        // Untyped, recognised by shape.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": "hi", "source": "planner"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": "hi", "source": "user"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"role": "user", "content": "already a message"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"content": [{"id": "c3", "name": "t", "arguments": {}}], "source": "planner"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"content": [{"call_id": "c4", "content": "r"}]}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": [{"name": "t", "content": "r"}]}"#)]),
        ),
        // Untyped and empty: nothing to read.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": ""}"#)]),
        ),
        // The two sentinels the carrier uses for absence.
        (
            "autogen process",
            rule_attrs(&[("message", r#"No Message"#)]),
        ),
        ("autogen process", rule_attrs(&[("message", r#"{}"#)])),
        ("autogen process", rule_attrs(&[("message", r#"not json"#)])),
        // An aggregate span's input: framework internals, claimed and read for nothing.
        (
            "autogen process",
            rule_attrs(&[("input.value", r#"{"cancellation_token":"tok"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[("input.value", r#"{"output_task_messages":true}"#)]),
        ),
        // ...and one that is not, which this dialect does not claim.
        (
            "autogen process",
            rule_attrs(&[("input.value", r#"{"other":1}"#)]),
        ),
        // The logging channel: one carrier holds the conversation, the reply and the tools.
        (
            "autogen process",
            rule_attrs(&[(
                "body",
                r#"{"type": "LLMCall", "messages": [{"role": "user", "content": "q"}, {"content": ""}], "response": {"content": "a", "tool_calls": [{"id": "c5", "type": "function", "function": {"name": "t", "arguments": {}}}]}, "tools": [{"name": "t"}]}"#,
            )]),
        ),
        // The reply in an OpenAI-shaped choice, with an empty call list that is not one.
        (
            "autogen process",
            rule_attrs(&[(
                "log.body",
                r#"{"type": "LLMStreamEnd", "messages": [{"role": "user", "content": "q"}], "response": {"choices": [{"message": {"content": "a", "tool_calls": []}}]}}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "autogen.event",
                r#"{"type": "ToolCall", "tool_name": "search", "arguments": {"q": "x"}, "result": "found"}"#,
            )]),
        ),
        // A tool execution with nothing recorded but its type.
        (
            "autogen process",
            rule_attrs(&[("autogen.event", r#"{"type": "ToolCall"}"#)]),
        ),
        // An event of another type: the gate is the type, so nothing is read.
        (
            "autogen process",
            rule_attrs(&[(
                "body",
                r#"{"type": "Unrelated", "messages": [{"role": "user", "content": "q"}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("traceloop.entity.input", r#"{"a":1}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("traceloop.entity.output", r#"[1,2]"#)]),
        ),
        (
            "span",
            rule_attrs(&[
                ("traceloop.entity.input", r#"{"a":1}"#),
                ("traceloop.entity.output", r#""done""#),
            ]),
        ),
        // Not JSON: `Json` mode must skip it, which is what the legacy `extract_json` did.
        (
            "span",
            rule_attrs(&[("traceloop.entity.input", "not json at all")]),
        ),
        (
            "span",
            rule_attrs(&[("mlflow.spanInputs", r#"{"messages":[]}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("mlflow.spanOutputs", r#"{"choices":[]}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("mlflow.chat.tools", r#"[{"name":"t"}]"#)]),
        ),
        ("span", rule_attrs(&[("mlflow.spanInputs", "unparseable")])),
        // `JsonOrString` mode: the string must survive rather than be dropped.
        (
            "span",
            rule_attrs(&[("tool_arguments", r#"{"city":"NYC"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("tool_arguments", "plain text arguments")]),
        ),
        ("span", rule_attrs(&[("tool_response", r#"{"v":"sunny"}"#)])),
        ("span", rule_attrs(&[("tool_response", "just a string")])),
        ("span", rule_attrs(&[("tool_response", "42")])),
        // All of them at once, which is also the ordering check.
        (
            "span",
            rule_attrs(&[
                ("traceloop.entity.input", r#"{"a":1}"#),
                ("mlflow.spanInputs", r#"{"b":2}"#),
                ("mlflow.chat.tools", r#"[{"name":"t"}]"#),
                ("tool_arguments", r#"{"c":3}"#),
                ("tool_response", "text"),
            ]),
        ),
        // Nothing at all: both must report `false` and emit nothing.
        ("span", rule_attrs(&[("unrelated.key", "x")])),
        // LangSmith: gated, so the same keys without a marker must yield nothing at all.
        (
            "span",
            rule_attrs(&[(
                "gen_ai.prompt",
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                (
                    "gen_ai.prompt",
                    r#"{"messages":[{"role":"user","content":"hi"}]}"#,
                ),
            ]),
        ),
        // Two turns, and a member of the request that is not a turn: it must not become one.
        (
            "span",
            rule_attrs(&[
                ("langsmith.trace.name", "t"),
                (
                    "gen_ai.prompt",
                    r#"{"messages":[{"role":"system","content":"s"},{"role":"user","content":"u"},
                    {"role":"noContent"}],"temperature":0.5}"#,
                ),
            ]),
        ),
        // A single message written directly, rather than in an array.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                ("gen_ai.prompt", r#"{"role":"user","content":"direct"}"#),
            ]),
        ),
        // The completion's choices array, with the finish reason beside each message.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                (
                    "gen_ai.completion",
                    r#"{"choices":[{"message":{"role":"assistant","content":"a"},
                    "finish_reason":"stop"}]}"#,
                ),
            ]),
        ),
        // Two choices, one without a finish reason: the lift must be per element.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                (
                    "gen_ai.completion",
                    r#"{"choices":[{"message":{"role":"assistant","content":"a"},"finish_reason":"stop"},
                    {"message":{"role":"assistant","content":"b"}}]}"#,
                ),
            ]),
        ),
        // A response carrying content and no role - which `any_of` admits and `all_of` would drop.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                ("gen_ai.completion", r#"{"content":"no role here"}"#),
            ]),
        ),
        // Both sides at once, plus a prefix-only marker.
        (
            "span",
            rule_attrs(&[
                ("langsmith.anything", "x"),
                (
                    "gen_ai.prompt",
                    r#"{"messages":[{"role":"user","content":"q"}]}"#,
                ),
                (
                    "gen_ai.completion",
                    r#"{"choices":[{"message":{"role":"assistant","content":"a"},
                    "finish_reason":"length"}]}"#,
                ),
            ]),
        ),
        // Unparseable on both sides: skipped, not stored as prose.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                ("gen_ai.prompt", "not json"),
                ("gen_ai.completion", "also not json"),
            ]),
        ),
        // Indexed families. Two turns, in the flattened encoding.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.0.role", "system"),
                ("gen_ai.prompt.0.content", "be brief"),
                ("gen_ai.prompt.1.role", "user"),
                ("gen_ai.prompt.1.content", "hello"),
            ]),
        ),
        // An index mentioned by a key that is not content: it must not become a turn.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.0.role", "user"),
                ("gen_ai.prompt.0.content", "q"),
                ("gen_ai.prompt.1.role", "assistant"),
            ]),
        ),
        // Nested content, which the convention also writes: presence must be satisfied by it.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.0.role", "user"),
                ("gen_ai.prompt.0.content.0.text", "nested"),
            ]),
        ),
        // Both families at once, and out of key order - entries must come back by index.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.completion.1.content", "second"),
                ("gen_ai.completion.0.content", "first"),
                ("gen_ai.prompt.0.role", "user"),
                ("gen_ai.prompt.0.content", "q"),
            ]),
        ),
        // A member whose value opens as JSON, and one that merely contains a brace.
        (
            "span",
            rule_attrs(&[
                (
                    "gen_ai.completion.0.content",
                    r#"[{"type":"text","text":"a"}]"#,
                ),
                ("gen_ai.completion.0.finish_reason", "stop"),
                ("gen_ai.completion.0.note", "not {json} really"),
            ]),
        ),
        // Opens as JSON and does not parse: the raw text must survive.
        (
            "span",
            rule_attrs(&[("gen_ai.completion.0.content", "{ truncated")]),
        ),
        // A double-digit index, so the parse is not one character wide.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.10.role", "user"),
                ("gen_ai.prompt.10.content", "tenth"),
            ]),
        ),
        // A family with no index at all.
        ("span", rule_attrs(&[("gen_ai.prompt.content", "no index")])),
        // LiveKit: prose, and the empty-value skip.
        ("span", rule_attrs(&[("lk.instructions", "be brief")])),
        ("span", rule_attrs(&[("lk.instructions", "")])),
        // Either key for the user's turn, and the tag must be the one found.
        ("span", rule_attrs(&[("lk.user_input", "hello")])),
        ("span", rule_attrs(&[("lk.input_text", "hello")])),
        (
            "span",
            rule_attrs(&[("lk.user_input", "first"), ("lk.input_text", "second")]),
        ),
        ("span", rule_attrs(&[("lk.input_text", "")])),
]
