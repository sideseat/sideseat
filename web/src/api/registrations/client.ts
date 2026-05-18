import type { ApiClient } from "../api-client";
import type { ListingResponse, PresenceStreamHandlers } from "./types";

export class RegistrationsClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  private base(projectId: string): string {
    return `/project/${projectId}`;
  }

  async list(projectId: string): Promise<ListingResponse> {
    return this.client.get(`${this.base(projectId)}/registrations`);
  }

  /**
   * Subscribe to the presence SSE stream for a project. The first frame is
   * a `snapshot` (full ListingResponse); subsequent frames are `presence`
   * deltas. Returns an unsubscribe function.
   */
  subscribeToPresence(projectId: string, handlers: PresenceStreamHandlers): () => void {
    return this.client.connectSSE(`${this.base(projectId)}/presence`, {
      events: {
        snapshot: (raw) => handlers.onSnapshot(JSON.parse(raw)),
        presence: (raw) => handlers.onPresence(JSON.parse(raw)),
      },
      onOpen: handlers.onOpen,
      onError: handlers.onError,
      onClose: handlers.onClose,
    });
  }
}
