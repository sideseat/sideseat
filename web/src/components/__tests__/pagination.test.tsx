import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";

import { Pagination } from "../pagination";

describe("Pagination", () => {
  it("resets its input only when the current page changes", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    const render = (currentPage: number) =>
      root.render(
        <Pagination
          currentPage={currentPage}
          pageSize={20}
          totalPages={10}
          onPageChange={vi.fn()}
          onPageSizeChange={vi.fn()}
        />,
      );

    await act(async () => render(1));
    const input = container.querySelector<HTMLInputElement>('input[name="current-page"]')!;
    act(() => {
      input.value = "7";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    await act(async () => render(1));
    expect(container.querySelector<HTMLInputElement>('input[name="current-page"]')!.value).toBe(
      "7",
    );

    await act(async () => render(2));
    expect(container.querySelector<HTMLInputElement>('input[name="current-page"]')!.value).toBe(
      "2",
    );

    await act(async () => root.unmount());
  });
});
