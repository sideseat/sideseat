import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { Block, ContentBlock } from "@/api/otel/types";
import { AppProvider } from "@/lib/app-context";
import { ContentRenderer } from "../content";
import { OMITTED_REASONING_LABEL, WITHHELD_REASONING_LABEL } from "../content/reasoning-labels";
import { getBlockPreview } from "../thread-utils";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

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
});

async function renderBlock(block: ContentBlock) {
  await act(async () =>
    root.render(
      <AppProvider>
        <ContentRenderer block={block} markdownEnabled={false} />
      </AppProvider>,
    ),
  );
}

function previewOf(content: ContentBlock): string {
  return getBlockPreview({
    entry_type: content.type,
    content,
    role: "assistant",
    trace_id: "trace-1",
    span_id: "span-1",
    message_index: 0,
    entry_index: 0,
    span_path: [],
    timestamp: new Date(Date.UTC(2026, 0, 1)).toISOString(),
    is_error: false,
    source_type: "event",
    category: "GenAIAssistantMessage" as Block["category"],
    content_hash: "hash-1",
    is_semantic: true,
  });
}

describe("ThinkingContent", () => {
  it("names signed reasoning whose text was withheld instead of leaving a blank entry", async () => {
    const withheld: ContentBlock = { type: "thinking", text: "", signed: true };
    await renderBlock(withheld);
    expect(container.textContent).toBe(WITHHELD_REASONING_LABEL);
    expect(previewOf(withheld)).toBe(WITHHELD_REASONING_LABEL);
  });

  it("shows the text of signed reasoning that has one, and never the signature", async () => {
    const shown: ContentBlock = { type: "thinking", text: "Two plus two is four.", signed: true };
    await renderBlock(shown);
    expect(container.textContent).toContain("Two plus two is four.");
    expect(container.textContent).not.toContain(WITHHELD_REASONING_LABEL);
    expect(previewOf(shown)).not.toBe(WITHHELD_REASONING_LABEL);
  });

  it("names reasoning with no text and no signature without calling it signed", async () => {
    const empty: ContentBlock = { type: "thinking", text: "" };
    await renderBlock(empty);
    expect(container.textContent).toBe(OMITTED_REASONING_LABEL);
    expect(previewOf(empty)).toBe(OMITTED_REASONING_LABEL);
    expect(previewOf({ type: "thinking", text: "  \n" })).toBe(OMITTED_REASONING_LABEL);
  });
});
