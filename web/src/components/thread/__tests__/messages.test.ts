import { describe, expect, it, vi } from "vitest";

import type { Block, ContentBlock } from "@/api/otel/types";
import {
  getMessageCopyText,
  getMessagePreview,
  groupBlocksIntoMessages,
  messageHasError,
  messageModel,
  messageRowConfig,
} from "../messages";
import { structuredData, summariseStructured } from "../thread-utils";

let seq = 0;

function block(content: ContentBlock, overrides: Partial<Block> = {}): Block {
  seq += 1;
  return {
    entry_type: content.type,
    content,
    role: "assistant",
    trace_id: "trace-1",
    span_id: "span-1",
    message_index: 0,
    entry_index: seq,
    span_path: [],
    timestamp: new Date(Date.UTC(2026, 0, 1, 0, 0, seq)).toISOString(),
    is_error: false,
    source_type: "event",
    category: "GenAIAssistantMessage" as Block["category"],
    content_hash: `hash-${seq}`,
    is_semantic: true,
    ...overrides,
  };
}

describe("groupBlocksIntoMessages", () => {
  it("puts the blocks of one message in one message", () => {
    const blocks = [
      block({ type: "text", text: "Checking the weather." }),
      block({ type: "tool_use", id: "call-1", name: "get_weather", input: { city: "Paris" } }),
      block({ type: "tool_use", id: "call-2", name: "get_weather", input: { city: "Tokyo" } }),
    ];

    const messages = groupBlocksIntoMessages(blocks);

    expect(messages).toHaveLength(1);
    expect(messages[0].blocks).toHaveLength(3);
    expect(messages[0].lead).toBe(blocks[0]);
    expect(messages[0].lastBlockIndex).toBe(2);
  });

  it("separates messages of one span by message index, and keeps every block", () => {
    const blocks = [
      block({ type: "text", text: "question" }, { role: "user", message_index: 0 }),
      block({ type: "text", text: "answer" }, { message_index: 1 }),
      block({ type: "tool_use", id: "c", name: "t", input: {} }, { message_index: 1 }),
    ];

    const messages = groupBlocksIntoMessages(blocks);

    expect(messages.map((m) => m.blocks.length)).toEqual([1, 2]);
    expect(messages.flatMap((m) => m.blocks)).toEqual(blocks);
  });

  it("separates the same message index in different spans and traces", () => {
    const blocks = [
      block({ type: "text", text: "one" }, { span_id: "span-a" }),
      block({ type: "text", text: "two" }, { span_id: "span-b" }),
      block({ type: "text", text: "three" }, { span_id: "span-b", trace_id: "trace-2" }),
    ];

    expect(groupBlocksIntoMessages(blocks)).toHaveLength(3);
  });

  it("does not merge one message index across an interruption, which would reorder the thread", () => {
    const blocks = [
      block({ type: "text", text: "first" }, { message_index: 2 }),
      block({ type: "text", text: "between" }, { message_index: 3 }),
      block({ type: "text", text: "again" }, { message_index: 2 }),
    ];

    const messages = groupBlocksIntoMessages(blocks);

    expect(messages).toHaveLength(3);
    expect(messages.map((m) => m.key)).toHaveLength(new Set(messages.map((m) => m.key)).size);
  });

  it("gives every message a key of its own", () => {
    const blocks = [
      block({ type: "text", text: "a" }, { message_index: 0 }),
      block({ type: "text", text: "b" }, { message_index: 1 }),
    ];

    const keys = groupBlocksIntoMessages(blocks).map((m) => m.key);

    expect(new Set(keys).size).toBe(2);
  });
});

describe("a message's summary", () => {
  it("leads with the text and counts the other parts", () => {
    const message = groupBlocksIntoMessages([
      block({ type: "text", text: "Checking two cities." }),
      block({ type: "tool_use", id: "c1", name: "get_weather", input: {} }),
      block({ type: "tool_use", id: "c2", name: "get_weather", input: {} }),
      block({ type: "thinking", text: "hmm" }),
    ])[0];

    expect(getMessagePreview(message)).toBe(
      "Checking two cities. · 2 tool calls · 1 thinking block",
    );
  });

  it("keeps a single block's own preview", () => {
    const message = groupBlocksIntoMessages([
      block({ type: "tool_use", id: "c", name: "get_weather", input: { city: "Paris" } }),
    ])[0];

    expect(getMessagePreview(message)).toBe('get_weather({"city":"Paris"})');
  });

  it("copies every block of the message", () => {
    const message = groupBlocksIntoMessages([
      block({ type: "text", text: "answer" }),
      block({ type: "tool_use", id: "c", name: "t", input: { a: 1 } }),
    ])[0];

    expect(getMessageCopyText(message)).toBe('answer\n\n{\n  "a": 1\n}');
  });

  it("reports an error and a model any of its blocks carries", () => {
    const message = groupBlocksIntoMessages([
      block({ type: "text", text: "answer" }),
      block({ type: "tool_result", content: "boom" }, { is_error: true, model: "claude" }),
    ])[0];

    expect(messageHasError(message)).toBe(true);
    expect(messageModel(message)).toBe("claude");
  });
});

describe("a message's row presentation", () => {
  it("keeps a lone tool call's own label", () => {
    const message = groupBlocksIntoMessages([
      block({ type: "tool_use", id: "c", name: "t", input: {} }),
    ])[0];

    expect(messageRowConfig(message).label).toBe("Tool Call");
  });

  it("names a turn of several parts after its role, not after its first part", () => {
    const message = groupBlocksIntoMessages([
      block({ type: "tool_use", id: "c", name: "t", input: {} }),
      block({ type: "text", text: "and the answer" }),
    ])[0];

    const config = messageRowConfig(message);

    expect(config.label).toBe("Assistant");
    expect(config.showMetadata).toBe(true);
  });
});

describe("structured output", () => {
  it("reads a json block's data without SideSeat's envelope", () => {
    const data = { city: "Vienna", days: ["one", "two"], budget_eur: 450 };

    expect(structuredData(block({ type: "json", data }))).toEqual(data);
    expect(getMessagePreview(groupBlocksIntoMessages([block({ type: "json", data })])[0])).toBe(
      "{city, days, budget_eur}",
    );
    expect(getMessageCopyText(groupBlocksIntoMessages([block({ type: "json", data })])[0])).toBe(
      JSON.stringify(data, null, 2),
    );
  });

  it("reads an answer a framework reported only as text", () => {
    const data = { city: "Vienna", budget_eur: 450 };

    expect(structuredData(block({ type: "text", text: JSON.stringify(data) }))).toEqual(data);
    expect(structuredData(block({ type: "text", text: `  [1, 2]  ` }))).toEqual([1, 2]);
  });

  it("leaves prose alone, including prose that starts like JSON", () => {
    expect(structuredData(block({ type: "text", text: "Vienna is lovely." }))).toBeUndefined();
    expect(structuredData(block({ type: "text", text: "42" }))).toBeUndefined();
    expect(structuredData(block({ type: "text", text: '"a quoted answer"' }))).toBeUndefined();
    expect(structuredData(block({ type: "text", text: "{not json at all" }))).toBeUndefined();
  });

  it("parses one text block once, however often a row re-renders", () => {
    const text = JSON.stringify({ city: "Vienna" });
    const parsed = block({ type: "text", text });
    const parse = vi.spyOn(JSON, "parse");

    expect(structuredData(parsed)).toEqual({ city: "Vienna" });
    expect(structuredData(parsed)).toEqual({ city: "Vienna" });
    expect(parse).toHaveBeenCalledTimes(1);

    const prose = block({ type: "text", text: "{not json" });
    expect(structuredData(prose)).toBeUndefined();
    expect(structuredData(prose)).toBeUndefined();
    // The failed parse is remembered too, so prose that starts like JSON is not re-parsed either.
    expect(parse).toHaveBeenCalledTimes(2);
    parse.mockRestore();
  });

  it("leaves a text block too large to be a schema answer as prose", () => {
    const huge = `{"a":"${"x".repeat(70_000)}"}`;

    expect(structuredData(block({ type: "text", text: huge }))).toBeUndefined();
  });

  it("summarises an array by its length and an object by its keys", () => {
    expect(summariseStructured([1, 2, 3])).toBe("[3 items]");
    expect(summariseStructured([1])).toBe("[1 item]");
    expect(summariseStructured({ a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7 })).toBe(
      "{a, b, c, d, e, f, …}",
    );
  });
});
