export type RegistrationKind = "agent" | "mcp" | "swarm" | "graph";

export interface RegistrationManifest {
  name: string;
  framework?: string | null;
  runtime?: unknown;
  model?: string | null;
  system_prompt?: string | null;
  tools: unknown[];
  metadata: unknown;
}

export interface RegistrationEntry {
  project_id: string;
  kind: RegistrationKind;
  name: string;
  manifest: RegistrationManifest;
  owner_client_id: string;
  owning_instance_id: string;
  last_heartbeat_secs: number;
}

export interface ListingResponse {
  agents: RegistrationEntry[];
  mcps: RegistrationEntry[];
  swarms: RegistrationEntry[];
  graphs: RegistrationEntry[];
}

export interface DisplacedOwner {
  client_id: string;
  instance_id: string;
}

export type PresenceEvent =
  | ({ event: "registered" } & RegistrationEntry)
  | {
      event: "replaced";
      project_id: string;
      kind: RegistrationKind;
      name: string;
      prev_owner: DisplacedOwner;
      new_owner: DisplacedOwner;
    }
  | {
      event: "unregistered";
      project_id: string;
      kind: RegistrationKind;
      name: string;
      owner: DisplacedOwner;
    }
  | {
      event: "expired";
      project_id: string;
      kind: RegistrationKind;
      name: string;
      owner: DisplacedOwner;
    };

export interface PresenceStreamHandlers {
  onSnapshot: (snap: ListingResponse) => void;
  onPresence: (ev: PresenceEvent) => void;
  onOpen?: () => void;
  onError?: (err: Error) => void;
  onClose?: () => void;
}
