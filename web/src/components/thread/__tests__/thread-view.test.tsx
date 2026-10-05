import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Block, ContentBlock, SpanEnvelope } from "@/api/otel/types";
import { AppProvider } from "@/lib/app-context";
import { JsonContent } from "../content/json-content";
import { getBlockKey, getBlockPreview } from "../thread-utils";
import { placeSpanErrors } from "../span-errors";
import { LARGE_THREAD_BLOCKS, ThreadView } from "../thread-view";
import type { ThreadViewProps } from "../types";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let seq = 0;

function block(content: ContentBlock, overrides: Partial<Block> = {}): Block {
  seq += 1;
  return {
    entry_type: content.type,
    content,
    role: "assistant",
    trace_id: "trace-1",
    span_id: `span-${seq}`,
    message_index: 0,
    entry_index: 0,
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

function envelope(overrides: Partial<SpanEnvelope>): SpanEnvelope {
  return {
    trace_id: "trace-1",
    span_id: "span-x",
    start_time: new Date(Date.UTC(2026, 0, 1)).toISOString(),
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    cost_input: 0,
    cost_output: 0,
    cost_total: 0,
    ...overrides,
  };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

async function renderThread(props: Partial<ThreadViewProps> & { blocks: Block[] }) {
  await act(async () =>
    root.render(
      <AppProvider>
        <ThreadView {...props} />
      </AppProvider>,
    ),
  );
}

const rowTriggers = () =>
  Array.from(container.querySelectorAll<HTMLButtonElement>("[data-thread-row-trigger]"));
const region = () => container.querySelector<HTMLElement>('[role="region"]')!;

function press(target: EventTarget, key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

describe("JsonContent", () => {
  async function renderJson(data: unknown) {
    await act(async () => root.render(<JsonContent data={data} />));
    return container.textContent;
  }

  it("prints a bare string as text, not as indexed characters", async () => {
    expect(await renderJson("look up the weather")).toBe("look up the weather");
  });

  it("prints numbers and booleans instead of an empty object", async () => {
    expect(await renderJson(42)).toBe("42");
    expect(await renderJson(false)).toBe("false");
  });

  it("renders null instead of throwing", async () => {
    expect(await renderJson(null)).toBe("null");
  });
});

describe("ThreadView states", () => {
  it("announces loading instead of rendering nothing", async () => {
    await renderThread({ blocks: [], isLoading: true });
    expect(container.querySelector('[role="status"]')?.getAttribute("aria-label")).toBe(
      "Loading messages",
    );
  });

  it("reports a load failure as an alert", async () => {
    await renderThread({ blocks: [], error: new Error("boom") });
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("boom");
  });

  it("does not claim the empty thread is a trace", async () => {
    await renderThread({ blocks: [] });
    expect(container.textContent).toContain("No messages");
    expect(container.textContent).not.toContain("This trace");
  });
});

describe("ThreadView rows", () => {
  it("uses a real button as the disclosure, with the copy control outside it", async () => {
    await renderThread({ blocks: [block({ type: "text", text: "hello" })] });
    const [trigger] = rowTriggers();
    expect(trigger.tagName).toBe("BUTTON");
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(trigger.querySelector("button")).toBeNull();
    const copy = container.querySelector('button[aria-label="Copy assistant"]');
    expect(copy).not.toBeNull();
    expect(trigger.contains(copy)).toBe(false);

    act(() => trigger.click());
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
  });

  it("keys blocks by trace, since span ids repeat across the traces of a session", async () => {
    const shared = { span_id: "span-1", message_index: 0, entry_index: 0 };
    const a = block({ type: "text", text: "first trace" }, { ...shared, trace_id: "trace-a" });
    const b = block({ type: "text", text: "second trace" }, { ...shared, trace_id: "trace-b" });
    expect(getBlockKey(a)).not.toBe(getBlockKey(b));

    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    await renderThread({ blocks: [a, b] });
    expect(consoleError).not.toHaveBeenCalledWith(
      expect.stringContaining("same key"),
      expect.anything(),
      expect.anything(),
    );
    expect(container.textContent).toContain("first trace");
    expect(container.textContent).toContain("second trace");
  });

  it("names the tool in collapsed call and result previews", () => {
    const call = block({
      type: "tool_use",
      id: "c1",
      name: "get_weather",
      input: { city: "Oslo" },
    });
    const result = block(
      { type: "tool_result", tool_use_id: "c1", content: "sunny" },
      { role: "tool", tool_name: "get_weather" },
    );
    expect(getBlockPreview(call)).toBe('get_weather({"city":"Oslo"})');
    expect(getBlockPreview(result)).toBe("get_weather → sunny");
  });

  it("names the tool on an expanded result", async () => {
    await renderThread({
      blocks: [
        block(
          { type: "tool_result", tool_use_id: "c1", content: "sunny" },
          { role: "tool", tool_name: "get_weather" },
        ),
      ],
    });
    expect(container.textContent).toContain("name: get_weather");
  });

  it("marks a message the model did not finish", async () => {
    await renderThread({
      blocks: [block({ type: "text", text: "The answer is" }, { finish_reason: "length" })],
    });
    expect(container.textContent).toContain("Truncated");
  });

  it("starts long threads collapsed so their content is not all mounted", async () => {
    const many = Array.from({ length: LARGE_THREAD_BLOCKS + 1 }, (_, i) =>
      block({ type: "text", text: `message body ${i}` }),
    );
    await renderThread({ blocks: many });
    expect(rowTriggers().every((t) => t.getAttribute("aria-expanded") === "false")).toBe(true);
    // Collapsed rows show a one-line preview but do not mount the rendered Markdown body.
    expect(container.querySelector(".prose")).toBeNull();
    expect(container.querySelector('button[aria-label="Expand all"]')).not.toBeNull();
  });
});

describe("ThreadView keyboard", () => {
  it("leaves keys outside the thread alone", async () => {
    await renderThread({ blocks: [block({ type: "text", text: "a" })] });
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const event = press(outside, "ArrowDown");
    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).not.toBe(rowTriggers()[0]);
    outside.remove();
  });

  it("moves focus between rows with the arrow keys", async () => {
    await renderThread({
      blocks: [block({ type: "text", text: "a" }), block({ type: "text", text: "b" })],
    });
    press(region(), "ArrowDown");
    expect(document.activeElement).toBe(rowTriggers()[0]);
    press(rowTriggers()[0], "j");
    expect(document.activeElement).toBe(rowTriggers()[1]);
    press(rowTriggers()[1], "k");
    expect(document.activeElement).toBe(rowTriggers()[0]);
  });

  it("does not hijack Cmd/Ctrl+C", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await renderThread({ blocks: [block({ type: "text", text: "a" })] });
    press(region(), "ArrowDown");
    press(rowTriggers()[0], "c", { metaKey: true });
    press(rowTriggers()[0], "c", { ctrlKey: true });
    expect(writeText).not.toHaveBeenCalled();
    press(rowTriggers()[0], "c");
    expect(writeText).toHaveBeenCalledWith("a");
  });
});

describe("span errors", () => {
  it("places a span's exception after that span's last block", () => {
    const first = block({ type: "text", text: "q" }, { span_id: "llm" });
    const second = block({ type: "text", text: "partial" }, { span_id: "llm" });
    const later = block({ type: "text", text: "later" }, { span_id: "other" });
    const failed = envelope({ span_id: "llm", status_code: "ERROR", exception_message: "x" });
    const placed = placeSpanErrors([first, second, later], [failed, envelope({ span_id: "ok" })]);
    expect(placed.leading).toEqual([]);
    expect([...placed.after.entries()]).toEqual([[1, [failed]]]);
  });

  it("places a failure without blocks by its start time", () => {
    const early = block({ type: "text", text: "q" }, { timestamp: "2026-01-01T00:00:01Z" });
    const late = block({ type: "text", text: "a" }, { timestamp: "2026-01-01T00:00:10Z" });
    const raised = envelope({
      span_id: "silent",
      start_time: "2026-01-01T00:00:05Z",
      exception_type: "RateLimitError",
    });
    const before = envelope({
      span_id: "first",
      start_time: "2026-01-01T00:00:00Z",
      exception_type: "Timeout",
    });
    const placed = placeSpanErrors([early, late], [raised, before]);
    expect(placed.after.get(0)).toEqual([raised]);
    expect(placed.leading).toEqual([before]);
  });

  it("shows the exception in the thread, even when the span recorded no message", async () => {
    await renderThread({
      blocks: [],
      envelopes: [
        envelope({
          span_name: "chat gpt-5",
          status_code: "ERROR",
          exception_type: "RateLimitError",
          exception_message: "429 Too Many Requests",
        }),
      ],
    });
    expect(container.textContent).not.toContain("No messages");
    expect(container.textContent).toContain("RateLimitError");
    expect(container.textContent).toContain("429 Too Many Requests");
    expect(container.textContent).toContain("in chat gpt-5");
  });
});

describe("tools tab", () => {
  it("lists tool names that arrived without definitions", async () => {
    await renderThread({
      blocks: [block({ type: "text", text: "a" })],
      activeTab: "tools",
      toolDefinitions: [{ type: "function", function: { name: "search" } }],
      toolNames: ["search", "fetch_url"],
    });
    expect(container.textContent).not.toContain("No tool definitions available");
    expect(container.textContent).toContain("fetch_url");
    expect(container.textContent).toContain("without a recorded definition");
  });

  it("is not empty when only tool names were reported", async () => {
    await renderThread({
      blocks: [block({ type: "text", text: "a" })],
      activeTab: "tools",
      toolNames: ["fetch_url"],
    });
    expect(container.textContent).not.toContain("No tool definitions available");
    expect(container.textContent).toContain("fetch_url");
  });
});
