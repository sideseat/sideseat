import { useEffect, useRef, useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

export interface EventStreamOptions<TEvent> {
  /** Subscribe function that returns cleanup. Must call onError for async errors! */
  subscribe: (
    onEvent: (event: TEvent) => void,
    onError: (error: Error) => void,
    onOpen?: () => void,
  ) => () => void;
  /** Key that triggers reconnection when changed (e.g., serialized params) */
  subscribeKey?: string;
  /** Query keys to invalidate on events (memoize with useMemo!) */
  invalidateKeys: readonly unknown[][];
  /** Debounce invalidation (ms) */
  debounceMs?: number;
  /** Enabled flag */
  enabled?: boolean;
  /** User callback for each event */
  onEvent?: (event: TEvent) => void;
  /** Error callback */
  onError?: (error: Error) => void;
  /** Callback when connection opens */
  onOpen?: () => void;
  /** Max reconnection attempts (0 = infinite) */
  maxRetries?: number;
  /** Base delay for exponential backoff (ms) */
  retryBaseDelay?: number;
  /** Max delay between retries (ms) - caps exponential backoff */
  maxRetryDelay?: number;
}

export interface EventStreamResult {
  status: ConnectionStatus;
  reconnect: () => void;
  retryCount: number;
}

export function useEventStream<TEvent>({
  subscribe,
  subscribeKey,
  invalidateKeys,
  debounceMs = 500,
  enabled = true,
  onEvent,
  onError,
  onOpen,
  maxRetries = 5,
  retryBaseDelay = 1000,
  maxRetryDelay = 30_000,
}: EventStreamOptions<TEvent>): EventStreamResult {
  const queryClient = useQueryClient();
  const cleanupRef = useRef<(() => void) | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingRef = useRef(false);
  const retryCountRef = useRef(0);
  const mountedRef = useRef(true);

  const [status, setStatus] = useState<ConnectionStatus>("disconnected");
  const [retryCount, setRetryCount] = useState(0);

  // Track mounted state for safe async updates
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Store callbacks in refs to avoid effect re-runs
  const onEventRef = useRef(onEvent);
  const onErrorRef = useRef(onError);
  const onOpenRef = useRef(onOpen);
  const subscribeRef = useRef(subscribe);
  const invalidateKeysRef = useRef(invalidateKeys);

  // Update refs on each render
  onEventRef.current = onEvent;
  onErrorRef.current = onError;
  onOpenRef.current = onOpen;
  subscribeRef.current = subscribe;
  invalidateKeysRef.current = invalidateKeys;

  const scheduleInvalidation = useCallback(() => {
    pendingRef.current = true;
    if (timerRef.current) {
      return;
    }

    timerRef.current = setTimeout(() => {
      if (pendingRef.current && mountedRef.current) {
        invalidateKeysRef.current.forEach((key) => {
          queryClient.invalidateQueries({ queryKey: key });
        });
        pendingRef.current = false;
      }
      timerRef.current = null;
    }, debounceMs);
  }, [queryClient, debounceMs]);

  // Stable connect function - uses refs for callbacks
  const connect = useCallback(() => {
    if (!mountedRef.current) return;
    setStatus("connecting");

    const handleError = (err: Error) => {
      if (!mountedRef.current) return;
      setStatus("error");
      onErrorRef.current?.(err);

      // Schedule retry with capped exponential backoff
      const currentRetry = retryCountRef.current;
      if (maxRetries === 0 || currentRetry < maxRetries) {
        const delay = Math.min(retryBaseDelay * Math.pow(2, currentRetry), maxRetryDelay);
        retryTimerRef.current = setTimeout(() => {
          if (!mountedRef.current) return;
          retryCountRef.current += 1;
          setRetryCount(retryCountRef.current);
          connect();
        }, delay);
      }
    };

    const handleEvent = (event: TEvent) => {
      if (!mountedRef.current) return;
      setStatus("connected");
      retryCountRef.current = 0;
      setRetryCount(0);
      scheduleInvalidation();
      onEventRef.current?.(event);
    };

    const handleOpen = () => {
      if (!mountedRef.current) return;
      setStatus("connected");
      retryCountRef.current = 0;
      setRetryCount(0);
      onOpenRef.current?.();
    };

    // SSE errors are async - passed via onError callback
    cleanupRef.current = subscribeRef.current(handleEvent, handleError, handleOpen);
  }, [scheduleInvalidation, maxRetries, retryBaseDelay, maxRetryDelay]);

  const reconnect = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;
    if (retryTimerRef.current) clearTimeout(retryTimerRef.current);
    retryCountRef.current = 0;
    setRetryCount(0);
    connect();
  }, [connect]);

  // Reconnect when enabled or subscribeKey changes
  useEffect(() => {
    if (!enabled) {
      setStatus("disconnected");
      cleanupRef.current?.();
      cleanupRef.current = null;
      return;
    }

    connect();

    return () => {
      cleanupRef.current?.();
      cleanupRef.current = null;
      if (timerRef.current) clearTimeout(timerRef.current);
      if (retryTimerRef.current) clearTimeout(retryTimerRef.current);
    };
  }, [enabled, connect, subscribeKey]);

  return { status, reconnect, retryCount };
}
