export const API_KEY_SCOPES = ["ingest", "read", "write", "full"] as const;
export type ApiKeyScope = (typeof API_KEY_SCOPES)[number];

export const SCOPE_DESCRIPTIONS: Record<ApiKeyScope, string> = {
  read: "Read-only access to traces, spans, sessions",
  ingest: "Send telemetry data (OTEL ingestion)",
  write: "Read + ingest + modify/delete data",
  full: "Full access including key management",
};

export const SCOPE_BADGE_VARIANT: Record<ApiKeyScope, "secondary" | "default" | "outline"> = {
  read: "secondary",
  ingest: "secondary",
  write: "default",
  full: "default",
};

export const EXPIRATION_PRESETS = [
  { label: "Never", days: null },
  { label: "30 days", days: 30 },
  { label: "90 days", days: 90 },
  { label: "1 year", days: 365 },
] as const;

/** API key from list endpoint (metadata only) */
export interface ApiKey {
  id: string;
  name: string;
  key_prefix: string;
  scope: ApiKeyScope;
  created_by: string | null;
  created_at: string;
  expires_at: string | null;
  last_used_at: string | null;
}

/** Response from create endpoint (includes one-time full key) */
export interface CreateApiKeyResponse {
  id: string;
  name: string;
  /** Full API key - shown only once! */
  key: string;
  key_prefix: string;
  scope: ApiKeyScope;
  created_at: string;
  expires_at: string | null;
}

/** Request body for creating an API key */
export interface CreateApiKeyRequest {
  name: string;
  scope: ApiKeyScope;
  /** Unix timestamp in seconds (not days!) */
  expires_at?: number;
}
