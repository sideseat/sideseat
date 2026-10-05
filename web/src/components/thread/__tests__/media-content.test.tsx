import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { AppProvider } from "@/lib/app-context";
import { MediaContent } from "../content/media-content";
import { MediaGalleryProvider } from "../image-gallery-context";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// Long enough not to count as placeholder data.
const PNG_BASE64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk";

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

async function renderMedia(data: string) {
  await act(async () =>
    root.render(
      <AppProvider>
        <MediaGalleryProvider blocks={[]}>
          <MediaContent type="image" mediaType="image/png" source="base64" data={data} />
        </MediaGalleryProvider>
      </AppProvider>,
    ),
  );
}

describe("MediaContent", () => {
  it("says an image failed to load and keeps its download", async () => {
    await renderMedia(PNG_BASE64);
    const img = container.querySelector("img")!;
    expect(img.getAttribute("alt")).toContain("image attachment");
    act(() => {
      img.dispatchEvent(new Event("error"));
    });
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("Could not be loaded");
    expect(container.querySelector('button[aria-label="Download Image"]')).not.toBeNull();
  });

  it("says when the content itself was not captured", async () => {
    await renderMedia("<replaced>");
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("Content not captured");
  });
});
