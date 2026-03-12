export type FieldType = "api_key" | "url" | "text" | "select" | "json_secret";

export interface CredentialField {
  name: string;
  label: string;
  type: FieldType;
  required: boolean;
  placeholder?: string;
  defaultValue?: string;
  description?: string;
  options?: { value: string; label: string }[];
  inExtraConfig?: boolean;
  inEndpointUrl?: boolean;
}

export interface AuthMode {
  id: string;
  label: string;
  fields: CredentialField[];
}

export interface ProviderEntry {
  key: string;
  displayName: string;
  description: string;
  group: "Cloud APIs" | "OpenAI-compatible";
  accentColor: string;
  abbrev: string;
  authModes?: AuthMode[];
  fields?: CredentialField[];
  defaultEndpoint?: string;
  supportsEndpointOverride: boolean;
}

export const PROVIDER_CATALOG: ProviderEntry[] = [
  {
    key: "bedrock",
    displayName: "Amazon Bedrock",
    description: "AWS-managed foundation models with IAM or bearer token auth",
    group: "Cloud APIs",
    accentColor: "#b45309",
    abbrev: "BK",
    supportsEndpointOverride: false,
    authModes: [
      {
        id: "bearer",
        label: "Bearer Token",
        fields: [
          { name: "api_key", label: "Bearer Token", type: "api_key", required: true },
          { name: "region", label: "Region", type: "text", required: true, placeholder: "us-east-1", defaultValue: "us-east-1", inExtraConfig: true },
        ],
      },
      {
        id: "access_keys",
        label: "Access Keys",
        fields: [
          { name: "access_key_id", label: "Access Key ID", type: "text", required: true, placeholder: "AKIA..." },
          { name: "secret_access_key", label: "Secret Access Key", type: "api_key", required: true },
          { name: "session_token", label: "Session Token", type: "api_key", required: false, description: "Optional — for temporary credentials" },
          { name: "region", label: "Region", type: "text", required: true, placeholder: "us-east-1", defaultValue: "us-east-1", inExtraConfig: true },
          { name: "role_arn", label: "Assume Role ARN", type: "text", required: false, placeholder: "arn:aws:iam::123456789012:role/MyRole", description: "Optional — assume a role after authenticating", inExtraConfig: true },
          { name: "external_id", label: "External ID", type: "text", required: false, description: "Optional — for cross-account role assumption", inExtraConfig: true },
        ],
      },
    ],
  },
  {
    key: "anthropic",
    displayName: "Anthropic",
    description: "Claude — frontier AI assistant and coding models",
    group: "Cloud APIs",
    accentColor: "#c2410c",
    abbrev: "AN",
    supportsEndpointOverride: true,
    fields: [
      { name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "sk-ant-api..." },
      { name: "endpoint_url", label: "Endpoint URL", type: "url", required: false, placeholder: "https://api.anthropic.com", description: "Override for proxies or self-hosted deployments", inEndpointUrl: true },
    ],
  },
  {
    key: "openai",
    displayName: "OpenAI",
    description: "GPT and reasoning models via OpenAI API",
    group: "Cloud APIs",
    accentColor: "#16a34a",
    abbrev: "OA",
    supportsEndpointOverride: true,
    fields: [
      { name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "sk-..." },
      { name: "endpoint_url", label: "Endpoint URL", type: "url", required: false, placeholder: "https://api.openai.com/v1", description: "Override for proxies or OpenAI-compatible APIs", inEndpointUrl: true },
      {
        name: "api_variant",
        label: "API Variant",
        type: "select",
        required: true,
        defaultValue: "chat_completions",
        inExtraConfig: true,
        options: [
          { value: "chat_completions", label: "Chat Completions" },
          { value: "responses_api", label: "Responses API (stateful)" },
        ],
      },
    ],
  },
  {
    key: "gemini",
    displayName: "Google Gemini",
    description: "Gemini models via Google AI Studio API",
    group: "Cloud APIs",
    accentColor: "#1d4ed8",
    abbrev: "GM",
    supportsEndpointOverride: false,
    fields: [
      { name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "AIzaSy..." },
      {
        name: "api_variant",
        label: "API Variant",
        type: "select",
        required: true,
        defaultValue: "standard",
        inExtraConfig: true,
        options: [
          { value: "standard", label: "Standard" },
          { value: "interactions", label: "Interactions API (stateful)" },
        ],
      },
    ],
  },
  {
    key: "vertex-ai",
    displayName: "Google Vertex AI",
    description: "Gemini and partner models via GCP Vertex AI",
    group: "Cloud APIs",
    accentColor: "#4338ca",
    abbrev: "VA",
    supportsEndpointOverride: false,
    authModes: [
      {
        id: "bearer",
        label: "Bearer Token",
        fields: [
          { name: "project_id", label: "Project ID", type: "text", required: true, placeholder: "my-gcp-project", inExtraConfig: true },
          { name: "location", label: "Region", type: "text", required: true, placeholder: "us-central1", defaultValue: "us-central1", inExtraConfig: true },
          { name: "api_key", label: "Bearer Token", type: "api_key", required: true, description: "OAuth2 bearer token (e.g. `gcloud auth print-access-token`)", placeholder: "ya29...." },
        ],
      },
      {
        id: "service_account",
        label: "Service Account JSON",
        fields: [
          { name: "service_account_json", label: "Service Account JSON", type: "json_secret", required: true, description: "Paste the JSON key file downloaded from GCP Console → IAM → Service Accounts" },
          { name: "location", label: "Region", type: "text", required: true, placeholder: "us-central1", defaultValue: "us-central1", inExtraConfig: true },
          { name: "project_id", label: "Project ID Override", type: "text", required: false, placeholder: "auto-detected from JSON", description: "Leave blank to use the project_id from the service account JSON", inExtraConfig: true },
        ],
      },
    ],
  },
  {
    key: "azure-ai-foundry",
    displayName: "Azure AI Foundry",
    description: "Azure OpenAI Service and AI Studio deployments",
    group: "Cloud APIs",
    accentColor: "#0284c7",
    abbrev: "AZ",
    supportsEndpointOverride: true,
    fields: [
      { name: "api_key", label: "API Key", type: "api_key", required: true },
      { name: "endpoint_url", label: "Endpoint URL", type: "url", required: true, placeholder: "https://your-resource.openai.azure.com/", inEndpointUrl: true },
      {
        name: "api_variant",
        label: "API Variant",
        type: "select",
        required: true,
        defaultValue: "standard",
        inExtraConfig: true,
        options: [
          { value: "standard", label: "Standard (deployment name in URL)" },
          { value: "v1", label: "Modern Foundry (model in request body)" },
        ],
      },
      { name: "deployment_name", label: "Deployment Name", type: "text", required: false, description: "Required for Standard variant", inExtraConfig: true },
    ],
  },
  {
    key: "xai",
    displayName: "xAI",
    description: "Grok models via xAI API",
    group: "Cloud APIs",
    accentColor: "#334155",
    abbrev: "XA",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "xai-..." }],
  },
  {
    key: "mistral",
    displayName: "Mistral AI",
    description: "European frontier models — chat, code and multimodal",
    group: "OpenAI-compatible",
    accentColor: "#e11d48",
    abbrev: "MI",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true }],
  },
  {
    key: "cohere",
    displayName: "Cohere",
    description: "Command R+, Embed and Rerank models",
    group: "OpenAI-compatible",
    accentColor: "#dc2626",
    abbrev: "CO",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "..." }],
  },
  {
    key: "deepseek",
    displayName: "DeepSeek",
    description: "Reasoning and chat models from DeepSeek",
    group: "OpenAI-compatible",
    accentColor: "#0d9488",
    abbrev: "DS",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true }],
  },
  {
    key: "openrouter",
    displayName: "OpenRouter",
    description: "Unified access to 200+ models from any provider",
    group: "OpenAI-compatible",
    accentColor: "#ea580c",
    abbrev: "OR",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "sk-or-..." }],
  },
  {
    key: "groq",
    displayName: "Groq",
    description: "Ultra-fast LPU inference for open models",
    group: "OpenAI-compatible",
    accentColor: "#65a30d",
    abbrev: "GQ",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "gsk_..." }],
  },
  {
    key: "cerebras",
    displayName: "Cerebras",
    description: "Wafer-scale chip inference — fastest token speeds",
    group: "OpenAI-compatible",
    accentColor: "#0369a1",
    abbrev: "CB",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "csk-..." }],
  },
  {
    key: "fireworks",
    displayName: "Fireworks AI",
    description: "Serverless open-source model hosting",
    group: "OpenAI-compatible",
    accentColor: "#c026d3",
    abbrev: "FW",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "fw_..." }],
  },
  {
    key: "together",
    displayName: "Together AI",
    description: "Open-source model hosting and fine-tuning",
    group: "OpenAI-compatible",
    accentColor: "#7c3aed",
    abbrev: "TO",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true }],
  },
  {
    key: "perplexity",
    displayName: "Perplexity",
    description: "Sonar models with real-time web search",
    group: "OpenAI-compatible",
    accentColor: "#059669",
    abbrev: "PX",
    supportsEndpointOverride: false,
    fields: [{ name: "api_key", label: "API Key", type: "api_key", required: true, placeholder: "pplx-..." }],
  },
  {
    key: "ollama",
    displayName: "Ollama",
    description: "Run any model locally on your own hardware",
    group: "OpenAI-compatible",
    accentColor: "#525252",
    abbrev: "OL",
    supportsEndpointOverride: false,
    defaultEndpoint: "http://localhost:11434",
    fields: [
      { name: "endpoint_url", label: "Endpoint URL", type: "url", required: true, placeholder: "http://localhost:11434", defaultValue: "http://localhost:11434", inEndpointUrl: true },
    ],
  },
  {
    key: "custom",
    displayName: "Custom (OpenAI-compatible)",
    description: "Any OpenAI-compatible endpoint — self-hosted models, proxies, or private APIs",
    group: "OpenAI-compatible",
    accentColor: "#6b7280",
    abbrev: "CU",
    supportsEndpointOverride: false,
    fields: [
      { name: "endpoint_url", label: "Endpoint URL", type: "url", required: true, placeholder: "https://your-server/v1", inEndpointUrl: true },
      { name: "api_key", label: "API Key", type: "api_key", required: false, description: "Leave blank if the endpoint does not require authentication" },
    ],
  },
];

export function getProvider(key: string): ProviderEntry | undefined {
  return PROVIDER_CATALOG.find((p) => p.key === key);
}

/**
 * Build a CreateCredentialRequest from form values.
 * Returns { secret_value, endpoint_url, extra_config }
 */
export function buildCreatePayload(
  provider: ProviderEntry,
  authModeId: string | null,
  values: Record<string, string>,
): { secret_value?: string; endpoint_url?: string; extra_config?: Record<string, unknown> } {
  const extra_config: Record<string, unknown> = {};
  let secret_value: string | undefined;
  let endpoint_url: string | undefined;

  const fields =
    authModeId && provider.authModes
      ? provider.authModes.find((m) => m.id === authModeId)?.fields ?? []
      : (provider.fields ?? []);

  // Bedrock access_keys: pack credential fields into JSON secret
  if (provider.key === "bedrock" && authModeId === "access_keys") {
    const payload: Record<string, string> = {
      access_key_id: values["access_key_id"] ?? "",
      secret_access_key: values["secret_access_key"] ?? "",
    };
    if (values["session_token"]) payload["session_token"] = values["session_token"];
    secret_value = JSON.stringify(payload);
  } else {
    // Standard: api_key field → secret_value
    const apiKeyField = fields.find((f) => f.name === "api_key" && !f.inExtraConfig && !f.inEndpointUrl);
    if (apiKeyField && values["api_key"]) {
      secret_value = values["api_key"];
    }
    // json_secret fields that aren't in extra_config/endpoint_url → secret_value
    if (!secret_value) {
      const jsonSecretField = fields.find((f) => f.type === "json_secret" && !f.inExtraConfig && !f.inEndpointUrl);
      if (jsonSecretField && values[jsonSecretField.name]) {
        secret_value = values[jsonSecretField.name];
      }
    }
  }

  // Map other fields
  for (const field of fields) {
    if (field.name === "api_key" && !field.inExtraConfig && !field.inEndpointUrl) continue;
    // access_key_id, secret_access_key, session_token are packed into secret_value above
    if (
      provider.key === "bedrock" &&
      authModeId === "access_keys" &&
      (field.name === "access_key_id" || field.name === "secret_access_key" || field.name === "session_token")
    )
      continue;

    const value = values[field.name];
    if (!value) continue;

    if (field.inEndpointUrl) {
      endpoint_url = value;
    } else if (field.inExtraConfig) {
      extra_config[field.name] = value;
    }
  }

  // Store auth_mode in extra_config for multi-mode providers
  if (provider.authModes && authModeId) {
    extra_config["auth_mode"] = authModeId;
  }

  return {
    secret_value: secret_value || undefined,
    endpoint_url: endpoint_url || undefined,
    extra_config: Object.keys(extra_config).length > 0 ? extra_config : undefined,
  };
}
