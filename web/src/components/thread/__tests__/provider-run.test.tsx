import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { ContentBlock } from "@/api/otel/types";
import { AppProvider } from "@/lib/app-context";
import { ContentRenderer } from "../content";

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

async function render(block: ContentBlock): Promise<string> {
  await act(async () =>
    root.render(
      <AppProvider>
        <ContentRenderer block={block} markdownEnabled={false} />
      </AppProvider>,
    ),
  );
  return container.textContent ?? "";
}

describe("a tool the provider ran itself", () => {
  it("marks the call and its result as run by the provider", async () => {
    const call = await render({
      type: "tool_use",
      id: "ws_1",
      name: "web_search",
      input: { query: "Louvre opening hours" },
      provider_executed: true,
    });
    expect(call).toContain("run by provider");
    const result = await render({
      type: "tool_result",
      tool_use_id: "ws_1",
      content: { sources: ["https://www.louvre.fr"] },
      provider_executed: true,
    });
    expect(result).toContain("run by provider");
  });

  it("leaves a call the application ran unmarked", async () => {
    const call = await render({ type: "tool_use", id: "c1", name: "get_weather", input: {} });
    expect(call).not.toContain("run by provider");
  });
});

describe("a text that cites its sources", () => {
  it("lists each source beneath the text, and an unread one as such", async () => {
    const shown = await render({
      type: "text",
      text: "The Louvre opens at 9 am.",
      citations: [
        { kind: "url", source: "https://www.louvre.fr/en/visit", title: "Louvre hours" },
        { kind: "document", source: "guide", cited_text: "Open from 9 am." },
        { kind: "unknown", raw: { type: "page_location", page: 2 } },
      ],
    });
    expect(shown).toContain("Louvre hours");
    expect(shown).toContain("guide");
    expect(shown).toContain("Open from 9 am.");
    expect(shown).toContain("Citation in a shape not yet read");
    const link = container.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://www.louvre.fr/en/visit");
  });
});
