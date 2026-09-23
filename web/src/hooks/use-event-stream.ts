import { useCallback, useEffect, useEffectEvent, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

export interface EventStreamOptions<TEvent> {
  subscribe: (
    onEvent: (event: TEvent) => void,
    onError: (error: Error) => void,
    onOpen?: () => void,
  ) => () => void;
  /** Identity of the remote subscription. Changing it reconnects the stream. */
  subscribeKey?: string;
  /** Query keys invalidated after incoming events. */
  invalidateKeys: readonly unknown[][];
  debounceMs?: number;
  enabled?: boolean;
  onEvent?: (event: TEvent) => void;
  onError?: (error: Error) => void;
  onOpen?: () => void;
  /** Maximum retry attempts; zero retries indefinitely. */
  maxRetries?: number;
  retryBaseDelay?: number;
  maxRetryDelay?: number;
}

export interface EventStreamResult {
  status: ConnectionStatus;
  reconnect: () => void;
  retryCount: number;
}

interface ConnectionState {
  id: object;
  status: ConnectionStatus;
  retryCount: number;
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
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
  const [reconnectVersion, setReconnectVersion] = useState(0);

  const connection = useMemo(
    () => ({
      enabled,
      subscribeKey,
      reconnectVersion,
      debounceMs,
      maxRetries,
      retryBaseDelay,
      maxRetryDelay,
    }),
    [
      enabled,
      subscribeKey,
      reconnectVersion,
      debounceMs,
      maxRetries,
      retryBaseDelay,
      maxRetryDelay,
    ],
  );

  const [state, setState] = useState<ConnectionState>(() => ({
    id: connection,
    status: enabled ? "connecting" : "disconnected",
    retryCount: 0,
  }));

  const subscribeToStream = useEffectEvent(
    (
      handleEvent: (event: TEvent) => void,
      handleError: (error: Error) => void,
      handleOpen: () => void,
    ) => subscribe(handleEvent, handleError, handleOpen),
  );
  const notifyEvent = useEffectEvent((event: TEvent) => onEvent?.(event));
  const notifyError = useEffectEvent((error: Error) => onError?.(error));
  const notifyOpen = useEffectEvent(() => onOpen?.());
  const invalidateQueries = useEffectEvent(() => {
    invalidateKeys.forEach((key) => {
      void queryClient.invalidateQueries({ queryKey: key });
    });
  });

  useEffect(() => {
    if (!connection.enabled) {
      return;
    }

    let disposed = false;
    let activeAttempt = 0;
    let retries = 0;
    let cleanup: (() => void) | undefined;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let invalidationTimer: ReturnType<typeof setTimeout> | undefined;

    const publish = (status: ConnectionStatus) => {
      setState({ id: connection, status, retryCount: retries });
    };

    const closeConnection = () => {
      const unsubscribe = cleanup;
      cleanup = undefined;
      unsubscribe?.();
    };

    const clearRetry = () => {
      if (retryTimer !== undefined) {
        clearTimeout(retryTimer);
        retryTimer = undefined;
      }
    };

    const scheduleInvalidation = () => {
      if (invalidationTimer !== undefined) {
        return;
      }
      invalidationTimer = setTimeout(() => {
        invalidationTimer = undefined;
        if (!disposed) {
          invalidateQueries();
        }
      }, connection.debounceMs);
    };

    function connect() {
      activeAttempt = activeAttempt + 1;
      const attempt = activeAttempt;
      closeConnection();

      const isActive = () => !disposed && attempt === activeAttempt;

      const handleOpen = () => {
        if (!isActive()) {
          return;
        }
        clearRetry();
        retries = 0;
        publish("connected");
        notifyOpen();
      };

      const handleEvent = (event: TEvent) => {
        if (!isActive()) {
          return;
        }
        clearRetry();
        retries = 0;
        publish("connected");
        scheduleInvalidation();
        notifyEvent(event);
      };

      const handleError = (error: Error) => {
        if (!isActive()) {
          return;
        }

        activeAttempt = activeAttempt + 1;
        closeConnection();
        clearRetry();
        publish("error");
        notifyError(error);

        if (connection.maxRetries !== 0 && retries >= connection.maxRetries) {
          return;
        }

        const delay = Math.min(connection.retryBaseDelay * 2 ** retries, connection.maxRetryDelay);
        retryTimer = setTimeout(() => {
          retryTimer = undefined;
          if (disposed) {
            return;
          }
          retries = retries + 1;
          publish("connecting");
          connect();
        }, delay);
      };

      try {
        const unsubscribe = subscribeToStream(handleEvent, handleError, handleOpen);
        if (isActive()) {
          cleanup = unsubscribe;
        } else {
          unsubscribe();
        }
      } catch (error) {
        handleError(asError(error));
      }
    }

    connect();

    return () => {
      disposed = true;
      activeAttempt = activeAttempt + 1;
      clearRetry();
      if (invalidationTimer !== undefined) {
        clearTimeout(invalidationTimer);
      }
      closeConnection();
    };
  }, [connection]);

  const reconnect = useCallback(() => {
    setReconnectVersion((version) => version + 1);
  }, []);

  if (!enabled) {
    return { status: "disconnected", reconnect, retryCount: 0 };
  }
  if (state.id !== connection) {
    return { status: "connecting", reconnect, retryCount: 0 };
  }
  return { status: state.status, reconnect, retryCount: state.retryCount };
}
