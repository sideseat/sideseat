import { useCallback, useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEventStream } from "@/hooks/use-event-stream";
import { useRegistrationsClient } from "@/lib/app-context";
import { registrationsKeys } from "./keys";
import type { ListingResponse, PresenceEvent, RegistrationEntry } from "./types";

type PresenceUpdate =
  | { kind: "snapshot"; snap: ListingResponse }
  | { kind: "presence"; ev: PresenceEvent };

const EMPTY: ListingResponse = { agents: [], mcps: [], swarms: [], graphs: [] };

/**
 * React Query handle for the cached registrations list. The cache is seeded
 * by `usePresenceStream`'s SSE snapshot — this hook never auto-fetches over
 * REST.
 */
export function useRegistrationsList(projectId: string) {
  return useQuery({
    queryKey: registrationsKeys.list(projectId),
    queryFn: async (): Promise<ListingResponse> => EMPTY,
    enabled: false,
    staleTime: Infinity,
  });
}

/**
 * Subscribe to the presence SSE stream and patch the React Query cache on
 * every event. Returns the connection status from `useEventStream`.
 */
export function usePresenceStream(projectId: string) {
  const c = useRegistrationsClient();
  const qc = useQueryClient();

  const subscribe = useCallback(
    (
      onEvent: (u: PresenceUpdate) => void,
      onStreamError: (err: Error) => void,
      onOpen?: () => void,
    ) =>
      c.subscribeToPresence(projectId, {
        onOpen,
        onError: onStreamError,
        onSnapshot: (snap) => onEvent({ kind: "snapshot", snap }),
        onPresence: (ev) => onEvent({ kind: "presence", ev }),
      }),
    [c, projectId],
  );

  const invalidateKeys = useMemo(() => [] as readonly unknown[][], []);

  return useEventStream<PresenceUpdate>({
    subscribe,
    subscribeKey: projectId,
    invalidateKeys,
    debounceMs: 0,
    enabled: !!projectId,
    onEvent: (u) => {
      if (u.kind === "snapshot") {
        qc.setQueryData(registrationsKeys.list(projectId), u.snap);
        return;
      }
      qc.setQueryData<ListingResponse>(registrationsKeys.list(projectId), (prev) =>
        applyPresence(prev, u.ev),
      );
    },
  });
}

function applyPresence(prev: ListingResponse | undefined, ev: PresenceEvent): ListingResponse {
  // Snapshot has not landed yet — start from empty so the registered
  // agent isn't lost. Snapshot will overwrite on arrival.
  const base: ListingResponse = prev ?? EMPTY;

  switch (ev.event) {
    case "registered": {
      // Strip the discriminator field to recover the raw RegistrationEntry.
      const entry: Record<string, unknown> = { ...ev };
      delete entry.event;
      return upsert(base, entry as unknown as RegistrationEntry);
    }
    case "replaced": {
      return updateOwner(base, ev.kind, ev.name, {
        owner_client_id: ev.new_owner.client_id,
        owning_instance_id: ev.new_owner.instance_id,
      });
    }
    case "unregistered":
    case "expired": {
      return remove(base, ev.kind, ev.name);
    }
  }
}

function bucketName(kind: RegistrationEntry["kind"]): keyof ListingResponse {
  switch (kind) {
    case "agent":
      return "agents";
    case "mcp":
      return "mcps";
    case "swarm":
      return "swarms";
    case "graph":
      return "graphs";
  }
}

function upsert(prev: ListingResponse, entry: RegistrationEntry): ListingResponse {
  const key = bucketName(entry.kind);
  const existing = prev[key];
  const idx = existing.findIndex((e) => e.name === entry.name);
  const next = idx >= 0 ? existing.map((e, i) => (i === idx ? entry : e)) : [...existing, entry];
  return { ...prev, [key]: next };
}

function updateOwner(
  prev: ListingResponse,
  kind: RegistrationEntry["kind"],
  name: string,
  patch: Partial<Pick<RegistrationEntry, "owner_client_id" | "owning_instance_id">>,
): ListingResponse {
  const key = bucketName(kind);
  const existing = prev[key];
  const idx = existing.findIndex((e) => e.name === name);
  if (idx < 0) return prev;
  const next = existing.map((e, i) => (i === idx ? { ...e, ...patch } : e));
  return { ...prev, [key]: next };
}

function remove(
  prev: ListingResponse,
  kind: RegistrationEntry["kind"],
  name: string,
): ListingResponse {
  const key = bucketName(kind);
  return { ...prev, [key]: prev[key].filter((e) => e.name !== name) };
}
