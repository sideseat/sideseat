import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useIsMobile } from "../use-mobile";

describe("useIsMobile", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("subscribes to the mobile media query", async () => {
    let matches = false;
    const listeners = new Set<() => void>();
    vi.stubGlobal(
      "matchMedia",
      vi.fn(
        () =>
          ({
            get matches() {
              return matches;
            },
            media: "(max-width: 767px)",
            addEventListener: (_type: string, listener: () => void) => {
              listeners.add(listener);
            },
            removeEventListener: (_type: string, listener: () => void) => {
              listeners.delete(listener);
            },
          }) as unknown as MediaQueryList,
      ),
    );

    function Probe() {
      return <span>{useIsMobile() ? "mobile" : "desktop"}</span>;
    }

    const container = document.createElement("div");
    const root = createRoot(container);
    await act(async () => root.render(<Probe />));
    expect(container.textContent).toBe("desktop");

    act(() => {
      matches = true;
      listeners.forEach((listener) => listener());
    });
    expect(container.textContent).toBe("mobile");

    await act(async () => root.unmount());
    expect(listeners.size).toBe(0);
  });
});
