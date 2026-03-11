export interface Credential {
  id: string;
  provider_key: string;
  display_name: string;
  endpoint_url: string | null;
  extra_config: Record<string, unknown> | null;
  key_preview: string | null;
  /** "stored", "env", or "ambient" (cloud-platform managed identity) */
  source: "stored" | "env" | "ambient";
  env_var_name: string | null;
  read_only: boolean;
  created_by: string | null;
  created_at: string | null;
}

export interface CreateCredentialRequest {
  display_name: string;
  provider_key: string;
  secret_value?: string;
  endpoint_url?: string;
  extra_config?: Record<string, unknown>;
}

export interface UpdateCredentialRequest {
  display_name?: string;
  /** undefined = no change; null = clear; string = set */
  endpoint_url?: string | null;
  /** undefined = no change; null = clear; object = set */
  extra_config?: Record<string, unknown> | null;
}

export interface TestResult {
  success: boolean;
  latency_ms: number;
  error?: string;
  model_hint?: string;
}

export interface CredentialPermission {
  id: string;
  credential_id: string;
  organization_id: string;
  project_id: string | null;
  access: "allow" | "deny";
  created_by: string | null;
  created_at: string;
}

export interface CreatePermissionRequest {
  project_id?: string | null;
  access: "allow" | "deny";
}
