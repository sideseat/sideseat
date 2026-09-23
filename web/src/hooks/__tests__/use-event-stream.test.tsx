import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  useEventStream,
  type EventStreamOptions,
  type EventStreamResult,
} from "../use-event-stream";

interface Subscription<TEvent> {
  onEvent: (event: TEvent) => void;
  onError: (error: Error) => void;
  onOpen: () => void;
  cleanup: ReturnType<typeof vi.fn>;
}

function createSubscribe<TEvent>() {
  const subscriptions: Subscription<TEvent>[] = [];
  const subscribe = vi.fn(
    (onEvent: (event: TEvent) => void, onError: (error: Error) => void, onOpen?: () => void) => {
      const cleanup = vi.fn();
      subscriptions.push({ onEvent, onError, onOpen: onOpen ?? (() => undefined), cleanup });
      return cleanup;
    },
  );
  return { subscribe, subscriptions };
}

describe("useEventStream", () => {
  let container: HTMLDivElement;
  let root: Root;
  let queryClient: QueryClient;

  beforeEach(() => {
    vi.useFakeTimers();
    container = document.createElement("div");
    root = createRoot(container);
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    queryClient.clear();
    vi.useRealTimers();
  });

  async function render<TEvent>(
    options: EventStreamOptions<TEvent>,
    capture?: (result: EventStreamResult) => void,
  ) {
    function Probe({ current }: { current: EventStreamOptions<TEvent> }) {
      const result = useEventStream(current);
      capture?.(result);
      return <span>{`${result.status}:${result.retryCount}`}</span>;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Probe current={options} />
        </QueryClientProvider>,
      );
    });

    return async (next: EventStreamOptions<TEvent>) => {
      await act(async () => {
        root.render(
          <QueryClientProvider client={queryClient}>
            <Probe current={next} />
          </QueryClientProvider>,
        );
      });
    };
  }

  it("closes a failed subscription before retrying", async () => {
    const { subscribe, subscriptions } = createSubscribe<string>();
    await render({
      subscribe,
      subscribeKey: "stream",
      invalidateKeys: [],
      retryBaseDelay: 100,
    });

    expect(container.textContent).toBe("connecting:0");
    act(() => subscriptions[0].onOpen());
    expect(container.textContent).toBe("connected:0");

    act(() => subscriptions[0].onError(new Error("offline")));
    expect(subscriptions[0].cleanup).toHaveBeenCalledOnce();
    expect(container.textContent).toBe("error:0");

    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(subscribe).toHaveBeenCalledTimes(2);
    expect(container.textContent).toBe("connecting:1");

    act(() => subscriptions[1].onOpen());
    expect(container.textContent).toBe("connected:0");
  });

  it("ignores duplicate errors from a failed attempt", async () => {
    const { subscribe, subscriptions } = createSubscribe<string>();
    const onError = vi.fn();
    await render({
      subscribe,
      invalidateKeys: [],
      retryBaseDelay: 100,
      onError,
    });

    act(() => {
      subscriptions[0].onError(new Error("first"));
      subscriptions[0].onError(new Error("duplicate"));
      vi.advanceTimersByTime(100);
    });

    expect(onError).toHaveBeenCalledOnce();
    expect(subscribe).toHaveBeenCalledTimes(2);
  });

  it("cleans up when subscribe reports an error synchronously", async () => {
    const cleanups: ReturnType<typeof vi.fn>[] = [];
    let attempt = 0;
    const subscribe = vi.fn(
      (_onEvent: (event: string) => void, onError: (error: Error) => void) => {
        const cleanup = vi.fn();
        cleanups.push(cleanup);
        attempt = attempt + 1;
        if (attempt === 1) {
          onError(new Error("synchronous failure"));
        }
        return cleanup;
      },
    );

    await render({
      subscribe,
      invalidateKeys: [],
      retryBaseDelay: 100,
    });

    expect(cleanups[0]).toHaveBeenCalledOnce();
    expect(container.textContent).toBe("error:0");

    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(subscribe).toHaveBeenCalledTimes(2);
    expect(container.textContent).toBe("connecting:1");
  });

  it("uses current callbacks and query keys without reconnecting", async () => {
    const { subscribe, subscriptions } = createSubscribe<string>();
    const firstEvent = vi.fn();
    const currentEvent = vi.fn();
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const rerender = await render({
      subscribe,
      subscribeKey: "stable",
      invalidateKeys: [["old"]],
      debounceMs: 50,
      onEvent: firstEvent,
    });

    await rerender({
      subscribe,
      subscribeKey: "stable",
      invalidateKeys: [["current"]],
      debounceMs: 50,
      onEvent: currentEvent,
    });
    act(() => {
      subscriptions[0].onEvent("payload");
      vi.advanceTimersByTime(50);
    });

    expect(subscribe).toHaveBeenCalledOnce();
    expect(firstEvent).not.toHaveBeenCalled();
    expect(currentEvent).toHaveBeenCalledWith("payload");
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["current"] });
  });

  it("cleans up on disable and manual reconnect", async () => {
    const { subscribe, subscriptions } = createSubscribe<string>();
    let result: EventStreamResult | undefined;
    const options = {
      subscribe,
      subscribeKey: "stream",
      invalidateKeys: [],
    };
    const rerender = await render(options, (next) => {
      result = next;
    });

    act(() => subscriptions[0].onOpen());
    expect(container.textContent).toBe("connected:0");

    await rerender({ ...options, enabled: false });
    expect(subscriptions[0].cleanup).toHaveBeenCalledOnce();
    expect(container.textContent).toBe("disconnected:0");

    await rerender(options);
    expect(subscribe).toHaveBeenCalledTimes(2);
    expect(container.textContent).toBe("connecting:0");

    act(() => result?.reconnect());
    expect(subscriptions[1].cleanup).toHaveBeenCalledOnce();
    expect(subscribe).toHaveBeenCalledTimes(3);
    expect(container.textContent).toBe("connecting:0");
  });
});
