vec![
        // A literal member naming what kind of context a payload is.
        (
            "span",
            rule_attrs(&[("lk.chat_ctx", r#"[{"role":"user","content":"x"}]"#)]),
        ),
        // Definitions rather than a message.
        (
            "span",
            rule_attrs(&[("lk.function_tools", r#"[{"name":"weather"}]"#)]),
        ),
        // A call written across three attributes.
        (
            "span",
            rule_attrs(&[
                ("lk.function_tool.arguments", r#"{"city":"NYC"}"#),
                ("lk.function_tool.name", "weather"),
                ("lk.function_tool.id", "call_1"),
            ]),
        ),
        // The same, with no name or id: the attachments must simply be absent.
        (
            "span",
            rule_attrs(&[("lk.function_tool.arguments", "raw args")]),
        ),
        // The error flag, present and true.
        (
            "span",
            rule_attrs(&[
                ("lk.function_tool.output", r#"{"v":1}"#),
                ("lk.function_tool.name", "weather"),
                ("lk.function_tool.is_error", "true"),
            ]),
        ),
        // Present and false: the literal must not attach.
        (
            "span",
            rule_attrs(&[
                ("lk.function_tool.output", "failed"),
                ("lk.function_tool.is_error", "false"),
            ]),
        ),
        // A reply with tool calls of the same response attached.
        (
            "span",
            rule_attrs(&[
                ("lk.response.text", "here you go"),
                ("lk.response.function_calls", r#"[{"name":"t"}]"#),
            ]),
        ),
        // A reply with no calls.
        ("span", rule_attrs(&[("lk.response.text", "just text")])),
        // Calls with no text: one message, under `tool_calls` rather than `content`.
        (
            "span",
            rule_attrs(&[("lk.response.function_calls", r#"[{"name":"t"}]"#)]),
        ),
        // Empty text beside calls - the either/or is on *presence*, matching the legacy reading.
        (
            "span",
            rule_attrs(&[
                ("lk.response.text", ""),
                ("lk.response.function_calls", r#"[{"name":"t"}]"#),
            ]),
        ),
        // The conventions themselves: request and response kept whole.
        (
            "span",
            rule_attrs(&[("gen_ai.input.messages", r#"[{"role":"user","parts":[]}]"#)]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gen_ai.output.messages",
                r#"[{"role":"assistant","parts":[]}]"#,
            )]),
        ),
        // A structured instruction, which goes under `parts` rather than `content`.
        (
            "span",
            rule_attrs(&[(
                "gen_ai.system_instructions",
                r#"[{"type":"text","content":"be brief"}]"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("gen_ai.system_instructions", "not json")]),
        ),
        // A whole conversation on an agent-run span.
        (
            "span",
            rule_attrs(&[("pydantic_ai.all_messages", r#"[{"role":"user"}]"#)]),
        ),
        // A tool call as a block, with the name and id beside it.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.arguments", r#"{"city":"NYC"}"#),
                ("gen_ai.tool.name", "weather"),
                ("gen_ai.tool.call.id", "call_1"),
            ]),
        ),
        // No name attribute: the empty default must be present, not the member omitted.
        (
            "span",
            rule_attrs(&[("gen_ai.tool.call.arguments", r#"{"city":"NYC"}"#)]),
        ),
        // The result, where an absent name is *omitted* rather than defaulted.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.result", r#"{"v":"sunny"}"#),
                ("gen_ai.tool.call.id", "call_1"),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.result", "plain result"),
                ("gen_ai.tool.name", "weather"),
            ]),
        ),
        // Both halves of the pair on one span.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.arguments", r#"{"a":1}"#),
                ("gen_ai.tool.call.result", r#"{"b":2}"#),
                ("gen_ai.tool.name", "t"),
                ("gen_ai.tool.call.id", "id1"),
            ]),
        ),
        // Vercel: the prompt under each spelling, both tagged canonically.
        (
            "span",
            rule_attrs(&[("ai.prompt.messages", r#"[{"role":"user","content":"q"}]"#)]),
        ),
        (
            "span",
            rule_attrs(&[("ai.prompt", r#"[{"role":"user","content":"q"}]"#)]),
        ),
        // Non-object elements in the array must be skipped, not emitted as turns.
        (
            "span",
            rule_attrs(&[("ai.prompt.messages", r#"[{"role":"user"},"stray",42]"#)]),
        ),
        // Not an array at all.
        (
            "span",
            rule_attrs(&[("ai.prompt.messages", r#"{"role":"user"}"#)]),
        ),
        // A tool call across three attributes.
        (
            "span",
            rule_attrs(&[
                ("ai.toolCall.args", r#"{"city":"NYC"}"#),
                ("ai.toolCall.name", "weather"),
                ("ai.toolCall.id", "c1"),
            ]),
        ),
        // The composed response: current spellings.
        (
            "span",
            rule_attrs(&[
                ("ai.response.text", "the answer"),
                ("ai.response.toolCalls", r#"[{"name":"t"}]"#),
                ("ai.response.finishReason", "stop"),
            ]),
        ),
        // Legacy spellings, which must read identically.
        (
            "span",
            rule_attrs(&[
                ("ai.result.text", "legacy answer"),
                ("ai.result.toolCalls", r#"[{"name":"t"}]"#),
                ("ai.result.object", r#"{"k":1}"#),
            ]),
        ),
        // A structured object under the current name, plus a swept member.
        (
            "span",
            rule_attrs(&[
                ("ai.response.object", r#"{"k":1}"#),
                ("ai.response.id", "resp-1"),
                ("ai.response.model", "m"),
            ]),
        ),
        // The gated fallback: `output.value` is read only on evidence this is such a span.
        (
            "span",
            rule_attrs(&[("output.value", "generic"), ("ai.prompt.messages", "[]")]),
        ),
        (
            "span",
            rule_attrs(&[("output.value", "generic"), ("ai.toolCall.name", "t")]),
        ),
        // No evidence at all: the fallback must not claim the generic carrier.
        ("span", rule_attrs(&[("output.value", "generic")])),
        // Nothing of the family: no response message at all, not one holding only a role.
        (
            "span",
            rule_attrs(&[("unrelated", "x"), ("ai.somethingElse", "y")]),
        ),
        // Claude Code, under a span name its rules can match - the gate is the point of the name column.
        (
            "claude_code.interaction",
            rule_attrs(&[("user_system_prompt", "   ")]),
        ),
        (
            "claude_code.interaction",
            rule_attrs(&[("user_system_prompt", "be terse")]),
        ),
        (
            "claude_code.llm_request",
            rule_attrs(&[("response.model_output", "the reply")]),
        ),
        // The same attributes under an unrelated span name: the gate must withhold them.
        ("span", rule_attrs(&[("user_system_prompt", "be terse")])),
        // One tagged section: the user turn.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[USER PROMPT]\nwhat is 2+2")]),
        ),
        // An untagged body is still the user's turn, not discarded.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "bare text with no tag")]),
        ),
        // A bracket with no newline is not a tag - it must not swallow the first line.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[not a tag] still text")]),
        ),
        // A tool result, id-tagged: the id is what pairs it with its call.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[TOOL RESULT: toolu_abc]\nthe file contents")]),
        ),
        // The name-tagged structured duplicate: dropped, narrowly.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[TOOL RESULT: Read]\n{\"path\":\"a\"}")]),
        ),
        // Name-tagged but *not* JSON: an unrecognised section must still reach the feed.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[TOOL RESULT: Read]\nplain text result")]),
        ),
        // Several sections in one attribute, one per parallel call, each keeping its own id.
        (
            "claude_code.interaction",
            rule_attrs(&[(
                "new_context",
                "[TOOL RESULT: toolu_1]\nfirst\n\n---\n\n[TOOL RESULT: toolu_2]\nsecond",
            )]),
        ),
        // A section whose body is empty is skipped.
        (
            "claude_code.interaction",
            rule_attrs(&[(
                "new_context",
                "[USER PROMPT]\n\n\n---\n\n[USER PROMPT]\nreal",
            )]),
        ),
        // A tool call: the name is the read carrier, the input stripped of its tag then parsed.
        (
            "claude_code.tool",
            rule_attrs(&[
                ("tool_name", "Read"),
                ("tool_input", "[TOOL INPUT: Read]\n{\"path\":\"/tmp/a\"}"),
                ("tool_use_id", "toolu_9"),
            ]),
        ),
        // Unparseable input falls back to the empty form, because the block's shape requires it.
        (
            "claude_code.tool",
            rule_attrs(&[("tool_name", "Read"), ("tool_input", "not json")]),
        ),
        // No input attribute at all.
        ("claude_code.tool", rule_attrs(&[("tool_name", "Bash")])),
        // A blank name is absence: no call.
        (
            "claude_code.tool",
            rule_attrs(&[("tool_name", "  "), ("tool_use_id", "toolu_1")]),
        ),
        // CrewAI: gated, so the same payload without a marker yields nothing.
        (
            "span",
            rule_attrs(&[("output.value", r#"{"raw":"the answer"}"#)]),
        ),
        // The answer alone.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                ("output.value", r#"{"raw":"the answer"}"#),
            ]),
        ),
        // History *and* the answer - the case that reading them as alternatives lost.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                (
                    "output.value",
                    r#"{"messages":[{"role":"user","content":"q"},
                        {"role":"assistant","content":"partial"}],"raw":"the answer"}"#,
                ),
            ]),
        ),
        // A task's own turns, two levels down.
        (
            "span",
            rule_attrs(&[
                ("task_key", "t"),
                (
                    "output.value",
                    r#"{"tasks_output":[{"messages":[{"role":"user","content":"a"}]},
                        {"messages":[{"role":"assistant","content":"b"}]}]}"#,
                ),
            ]),
        ),
        // A member of a task that is not a turn must not become one.
        (
            "span",
            rule_attrs(&[
                ("crew_id", "c"),
                (
                    "output.value",
                    r#"{"messages":[{"role":"user","content":"q"},{"role":"noContent"},
                        {"content":"noRole"},{"role":"a","tool_calls":[]}]}"#,
                ),
            ]),
        ),
        // A blank answer is not an answer, and with no turns either the payload is kept whole.
        (
            "span",
            rule_attrs(&[("crew_key", "k"), ("output.value", r#"{"raw":"   "}"#)]),
        ),
        // No documented shape at all: kept whole rather than leaving the span empty.
        (
            "span",
            rule_attrs(&[("crew_key", "k"), ("output.value", r#"{"other":1}"#)]),
        ),
        // The task definitions.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                ("crew_tasks", r#"[{"name":"research"}]"#),
            ]),
        ),
        // Unparseable output: skipped, and the task list still read.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                ("crew_tasks", r#"[{"name":"n"}]"#),
                ("output.value", "not json"),
            ]),
        ),
        // Logfire: recognised events, each tagged with its own name.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"event.name":"gen_ai.user.message","content":"q"},
                    {"event.name":"gen_ai.choice","content":"a"}]"#,
            )]),
        ),
        // Unnamed events carrying multimodal blocks, grouped into runs by side.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"input_text","text":"a"}},
                    {"data":{"type":"input_image","url":"u"}},
                    {"data":{"type":"output_text","text":"b"}}]"#,
            )]),
        ),
        // Mixed: recognised events must all precede the grouped blocks.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"input_text","text":"a"}},
                    {"event.name":"gen_ai.choice","content":"answer"},
                    {"data":{"type":"output_text","text":"b"}}]"#,
            )]),
        ),
        // A run returning to a previous side is a new run, not a merge.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"input_text","text":"a"}},
                    {"data":{"type":"output_text","text":"b"}},
                    {"data":{"type":"input_text","text":"c"}}]"#,
            )]),
        ),
        // A block whose type matches neither side is skipped.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"other"}},{"data":{"type":"input_text","text":"a"}}]"#,
            )]),
        ),
        // The prompt and the whole-conversation carriers.
        (
            "span",
            rule_attrs(&[("prompt", r#"[{"role":"user","content":"q"}]"#)]),
        ),
        (
            "span",
            rule_attrs(&[(
                "all_messages_events",
                r#"[{"role":"assistant","content":"a"}]"#,
            )]),
        ),
        // The response, in both documented shapes.
        (
            "span",
            rule_attrs(&[(
                "response_data",
                r#"{"message":{"role":"assistant","content":"a"}}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("response_data", r#"{"combined_chunk_content":"streamed"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("response_data", r#"{"combined_chunk_content":""}"#)]),
        ),
        // The request payload: a fallback, read only when nothing else carried the conversation.
        (
            "span",
            rule_attrs(&[(
                "request_data",
                r#"{"messages":[{"role":"user","content":"q"}]}"#,
            )]),
        ),
        // With the events present, the request payload must not be read as well.
        (
            "span",
            rule_attrs(&[
                (
                    "events",
                    r#"[{"event.name":"gen_ai.user.message","content":"q"}]"#,
                ),
                (
                    "request_data",
                    r#"{"messages":[{"role":"user","content":"q"}]}"#,
                ),
            ]),
        ),
        // An empty request payload is not a conversation.
        (
            "span",
            rule_attrs(&[("request_data", r#"{"messages":[]}"#)]),
        ),
        // ADK: the request, with both spellings of its members.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"systemInstruction":{"parts":[{"text":"be brief"}]},
                    "contents":[{"role":"user","parts":[{"text":"q"}]},
                                {"role":"model","parts":[{"text":"a"}]}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"systemInstruction":"a bare string"}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"config":{"system_instruction":"the framework's own spelling"}}"#,
            )]),
        ),
        // An empty request is not a request.
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.llm_request", "{}")]),
        ),
        // Tools: wrapped declarations under each spelling, and a bare group.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"tools":[{"function_declarations":[{"name":"a"},{"name":"b"}]}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"tools":[{"functionDeclarations":[{"name":"c"}]}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"config":{"tools":[{"name":"bare"}]}}"#,
            )]),
        ),
        // A wrapped group beside a bare one: the per-element decision.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"tools":[{"function_declarations":[{"name":"a"}]},{"name":"bare"}]}"#,
            )]),
        ),
        // The response: the provider's role alias and the finish reason beside the content.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_response",
                r#"{"content":{"role":"model","parts":[{"text":"a"}]},"finish_reason":"STOP"}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_response",
                r#"{"content":{"role":"user","parts":[]}}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.llm_response", "{}")]),
        ),
        // The arguments fallback: read only when the request supplied nothing.
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.tool_call_args", r#"{"city":"NYC"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[
                (
                    "gcp.vertex.agent.llm_request",
                    r#"{"contents":[{"role":"user","parts":[]}]}"#,
                ),
                ("gcp.vertex.agent.tool_call_args", r#"{"city":"NYC"}"#),
            ]),
        ),
        // A tool's result, and the conversation handed to an agent.
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.tool_response", r#"{"v":"sunny"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.data", r#"[{"role":"user"}]"#)]),
        ),
        ("span", rule_attrs(&[("gcp.vertex.agent.data", "{}")])),
        ("span", rule_attrs(&[("gcp.vertex.agent.data", "[]")])),
        // LangGraph: a state object, gated.
        (
            "span",
            rule_attrs(&[(
                "output.value",
                r#"{"messages":[{"type":"human","content":"q"}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "agent"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"human","content":"q"},
                    {"type":"ai","content":"a"}]}"#,
                ),
            ]),
        ),
        // The answer beside the conversation - the shape that once lost every reply.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "agent"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"human","content":"q"}],"raw":"the answer"}"#,
                ),
            ]),
        ),
        // Serialised form: the discriminator and content under the constructor's kwargs.
        (
            "span",
            rule_attrs(&[
                ("langgraph.thread_id", "t"),
                (
                    "output.value",
                    r#"{"messages":[{"lc":{"type":"ai"},"kwargs":{"content":"a",
                    "tool_calls":[{"name":"t"}]}}]}"#,
                ),
            ]),
        ),
        // An empty tool-call list must not attach.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"ai","content":"a","tool_calls":[]}]}"#,
                ),
            ]),
        ),
        // A tool reply, with its call id and name.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"tool","content":"r",
                    "tool_call_id":"c1","name":"temp"}]}"#,
                ),
            ]),
        ),
        // Nested state, which the walk exists for.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"state":{"messages":[{"type":"human","content":"q"}]}}"#,
                ),
            ]),
        ),
        // Already canonical: kept exactly as it arrived.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"messages":[{"role":"user","content":"q","extra":1}]}"#,
                ),
            ]),
        ),
        // A single message on its own carrier.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                ("message", r#"{"type":"system","content":"be brief"}"#),
            ]),
        ),
        // The answer must be read even when the request side was found - the asymmetry that
        // closed a real loss.
        (
            "span",
            rule_attrs(&[
                (
                    "events",
                    r#"[{"event.name":"gen_ai.user.message","content":"q"}]"#,
                ),
                (
                    "response_data",
                    r#"{"message":{"role":"assistant","content":"a"}}"#,
                ),
            ]),
        ),
]
