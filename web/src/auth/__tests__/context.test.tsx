import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const auth = vi.hoisted(() => ({
  getStatus: vi.fn(),
  exchangeToken: vi.fn(),
  logout: vi.fn(),
}));

vi.mock("@/api/api-client", () => ({
  apiClient: { auth },
}));

vi.mock("sonner", () => ({
  toast: { error: vi.fn() },
}));

import { AuthProvider, useAuth } from "../context";
import type { AuthStatusResult } from "@/api/auth-client";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function Probe() {
  const { authenticated, loading } = useAuth();
  return <span>{`${authenticated}:${loading}`}</span>;
}

describe("AuthProvider", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    auth.getStatus.mockReset();
    auth.exchangeToken.mockReset();
    auth.logout.mockReset();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function renderProvider() {
    await act(async () => {
      root.render(
        <AuthProvider>
          <Probe />
        </AuthProvider>,
      );
    });
  }

  it("keeps the newest authentication result", async () => {
    const older = deferred<AuthStatusResult>();
    const newer = deferred<AuthStatusResult>();
    auth.getStatus
      .mockResolvedValueOnce({ status: "unauthenticated" })
      .mockReturnValueOnce(older.promise)
      .mockReturnValueOnce(newer.promise);

    await renderProvider();
    expect(container.textContent).toBe("false:false");

    act(() => window.dispatchEvent(new Event("focus")));
    act(() => window.dispatchEvent(new Event("focus")));

    await act(async () => {
      newer.resolve({
        status: "authenticated",
        data: { authenticated: true, version: "test" },
      });
      await newer.promise;
    });
    expect(container.textContent).toBe("true:false");

    await act(async () => {
      older.resolve({ status: "unauthenticated" });
      await older.promise;
    });
    expect(container.textContent).toBe("true:false");
  });

  it("does not restore a request invalidated by auth:required", async () => {
    const pending = deferred<AuthStatusResult>();
    auth.getStatus.mockReturnValueOnce(pending.promise);

    await renderProvider();
    act(() => window.dispatchEvent(new CustomEvent("auth:required")));
    expect(container.textContent).toBe("false:false");

    await act(async () => {
      pending.resolve({
        status: "authenticated",
        data: { authenticated: true, version: "test" },
      });
      await pending.promise;
    });
    expect(container.textContent).toBe("false:false");
  });
});
