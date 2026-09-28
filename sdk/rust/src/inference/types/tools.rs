use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Enable strict schema validation (OpenAI Chat/Responses only).
    /// When true, `additionalProperties: false` is automatically added
    /// and OpenAI validates that all call arguments match the schema exactly.
    /// Ignored by other providers.
    pub strict: bool,
    /// Example inputs for documentation / few-shot prompting (not forwarded to providers).
    #[serde(default)]
    pub input_examples: Vec<serde_json::Value>,
}

impl Tool {
    /// Create a tool definition.
    ///
    /// - `name` — identifier used by the model to call the tool (no spaces)
    /// - `description` — natural-language description of what the tool does (shown to the model)
    /// - `input_schema` — JSON Schema object describing the expected arguments
    ///
    /// Use [`Self::with_strict`] to enable strict schema validation (OpenAI),
    /// and [`Self::with_input_examples`] to provide few-shot examples.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            strict: false,
            input_examples: vec![],
        }
    }

    /// Enable strict schema validation for this tool (OpenAI).
    pub fn with_strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Attach example inputs for documentation / few-shot prompting.
    pub fn with_input_examples(mut self, examples: Vec<serde_json::Value>) -> Self {
        self.input_examples = examples;
        self
    }
}

// ---------------------------------------------------------------------------
// Tool choice
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to use tools (default)
    Auto,
    /// Model must use at least one tool
    Any,
    /// Do not use tools
    None,
    /// Force a specific tool by name
    Tool { name: String },
    /// Allow the model to choose from a named subset of the defined tools.
    ///
    /// Maps to OpenAI `{"type": "allowed_tools", "mode": "auto", "tools": [...]}`.
    AllowedTools { tools: Vec<String> },
}

// ---------------------------------------------------------------------------
// Reasoning effort (OpenAI o-series, xAI, DeepSeek)
// ---------------------------------------------------------------------------

/// Reasoning effort level for models with extended thinking.
///
/// - OpenAI o-series: sent as `reasoning_effort`
/// - Anthropic (Opus 4.6, Sonnet 4.6, Opus 4.5): sent as `output_config.effort`
///   `Max` is only valid for Anthropic claude-opus-4-6.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    /// Maximum effort — only available on Anthropic claude-opus-4-6.
    Max,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Response format (structured output)
// ---------------------------------------------------------------------------

/// Desired output format for the model's response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseFormat {
    /// Plain text (default)
    Text,
    /// Unstructured JSON — model must output valid JSON, no schema enforced
    Json,
    /// Structured JSON output validated against a schema
    JsonSchema {
        /// Schema name (used as the response_format name in the API)
        name: String,
        /// JSON Schema object describing the expected structure
        schema: serde_json::Value,
        /// Strict schema validation. Honored by OpenAI Chat, OpenAI Responses, Mistral, and xAI.
        /// Ignored by Anthropic, Gemini, Gemini Interactions, Cohere, and Bedrock.
        /// When `true`, automatically adds `additionalProperties: false` to the schema.
        strict: bool,
    },
}

impl ResponseFormat {
    /// Shorthand for `JsonSchema` with `strict: true`.
    pub fn json_schema_strict(name: impl Into<String>, schema: serde_json::Value) -> Self {
        Self::JsonSchema {
            name: name.into(),
            schema,
            strict: true,
        }
    }
}

// ---------------------------------------------------------------------------
// JsonSchema builder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing JSON Schema objects.
///
/// # Example
///
/// ```
/// use sideseat::types::{JsonSchema, Tool};
///
/// let schema = JsonSchema::object()
///     .required_field("city", JsonSchema::string().description("City name"))
///     .field("units", JsonSchema::string().enum_values(["celsius", "fahrenheit"]))
///     .build();
///
/// let _tool = Tool::new("get_weather", "Get current weather", schema);
/// ```
pub struct JsonSchema {
    schema: serde_json::Value,
}

impl JsonSchema {
    pub fn object() -> Self {
        Self {
            schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
        }
    }

    pub fn string() -> Self {
        Self {
            schema: serde_json::json!({"type": "string"}),
        }
    }

    pub fn number() -> Self {
        Self {
            schema: serde_json::json!({"type": "number"}),
        }
    }

    pub fn integer() -> Self {
        Self {
            schema: serde_json::json!({"type": "integer"}),
        }
    }

    pub fn boolean() -> Self {
        Self {
            schema: serde_json::json!({"type": "boolean"}),
        }
    }

    pub fn null() -> Self {
        Self {
            schema: serde_json::json!({"type": "null"}),
        }
    }

    pub fn array(items: Self) -> Self {
        Self {
            schema: serde_json::json!({"type": "array", "items": items.schema}),
        }
    }

    pub fn description(mut self, desc: &str) -> Self {
        self.schema["description"] = serde_json::Value::String(desc.to_string());
        self
    }

    /// Add an optional property to an object schema.
    pub fn field(mut self, name: &str, schema: Self) -> Self {
        if let Some(props) = self
            .schema
            .get_mut("properties")
            .and_then(|v| v.as_object_mut())
        {
            props.insert(name.to_string(), schema.schema);
        }
        self
    }

    /// Add a required property to an object schema.
    pub fn required_field(mut self, name: &str, schema: Self) -> Self {
        if let Some(props) = self
            .schema
            .get_mut("properties")
            .and_then(|v| v.as_object_mut())
        {
            props.insert(name.to_string(), schema.schema);
        }
        if let Some(req) = self
            .schema
            .get_mut("required")
            .and_then(|v| v.as_array_mut())
        {
            req.push(serde_json::Value::String(name.to_string()));
        }
        self
    }

    pub fn enum_values<S: AsRef<str>>(mut self, values: impl IntoIterator<Item = S>) -> Self {
        self.schema["enum"] = serde_json::Value::Array(
            values
                .into_iter()
                .map(|v| serde_json::Value::String(v.as_ref().to_string()))
                .collect(),
        );
        self
    }

    /// Allow null in addition to the current type.
    pub fn nullable(mut self) -> Self {
        if let Some(t) = self
            .schema
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        {
            self.schema["type"] = serde_json::json!([t, "null"]);
        }
        self
    }

    pub fn build(self) -> serde_json::Value {
        self.schema
    }
}

impl From<JsonSchema> for serde_json::Value {
    fn from(s: JsonSchema) -> Self {
        s.build()
    }
}

// ---------------------------------------------------------------------------
// Service tier
// ---------------------------------------------------------------------------

/// Processing tier sent to the provider.
///
/// | Variant | OpenAI | Anthropic |
/// |---|---|---|
/// | `Auto` | `"auto"` | `"auto"` |
/// | `Default` | `"default"` | — |
/// | `Flex` | `"flex"` | — |
/// | `StandardOnly` | — | `"standard_only"` |
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// Use priority capacity, falling back to standard (both OpenAI and Anthropic).
    Auto,
    /// Standard capacity only (OpenAI).
    Default,
    /// Lower cost, potentially slower (OpenAI).
    Flex,
    /// Standard capacity only, no priority fallback (Anthropic).
    StandardOnly,
}

impl ServiceTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Default => "default",
            Self::Flex => "flex",
            Self::StandardOnly => "standard_only",
        }
    }
}

impl std::fmt::Display for ServiceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Cache control (Anthropic prompt caching)
// ---------------------------------------------------------------------------

/// Anthropic prompt cache control — marks a message or system prompt for caching.
///
/// Single-variant enum kept as enum for forward compatibility; future variants may add
/// `Persistent` or `Ttl(Duration)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheControl {
    /// Ephemeral cache (5-minute TTL, 1.25× token cost for cache writes)
    Ephemeral,
}

// ---------------------------------------------------------------------------
// Web search configuration
// ---------------------------------------------------------------------------

/// Approximate geographic context for web search results (OpenAI).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSearchUserLocation {
    /// Always `"approximate"`.
    #[serde(rename = "type")]
    pub location_type: String,
    /// ISO 3166-1 alpha-2 country code (e.g. `"US"`, `"GB"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// City name (e.g. `"London"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// State or region name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// IANA timezone (e.g. `"America/Chicago"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl WebSearchUserLocation {
    pub fn new() -> Self {
        Self {
            location_type: "approximate".into(),
            ..Default::default()
        }
    }
    pub fn with_country(mut self, c: impl Into<String>) -> Self {
        self.country = Some(c.into());
        self
    }
    pub fn with_city(mut self, c: impl Into<String>) -> Self {
        self.city = Some(c.into());
        self
    }
    pub fn with_region(mut self, r: impl Into<String>) -> Self {
        self.region = Some(r.into());
        self
    }
    pub fn with_timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }
}

/// Remote MCP server configuration for [`BuiltinTool::mcp`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolConfig {
    /// Always `"mcp"` — set automatically by [`BuiltinTool::mcp`].
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Human-readable label for the server (used in tool names and logs).
    pub server_label: String,
    /// MCP server URL (for remote servers). Mutually exclusive with `connector_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// OpenAI-maintained connector ID (e.g. `"connector_dropbox"`). Mutually exclusive with `server_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    /// Human-readable description of the server's capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_description: Option<String>,
    /// When to require user approval: `"never"`, `"always"`, or an object specifying individual tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_approval: Option<serde_json::Value>,
    /// Restrict which MCP tools the model may call. `None` means all tools are allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Bearer token or OAuth access token for server authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
}

impl McpToolConfig {
    /// Remote MCP server accessible via URL.
    pub fn new(server_label: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            tool_type: "mcp".into(),
            server_label: server_label.into(),
            server_url: Some(server_url.into()),
            connector_id: None,
            server_description: None,
            require_approval: None,
            allowed_tools: None,
            authorization: None,
        }
    }
    /// OpenAI-maintained connector (e.g. Dropbox, Gmail).
    pub fn connector(server_label: impl Into<String>, connector_id: impl Into<String>) -> Self {
        Self {
            tool_type: "mcp".into(),
            server_label: server_label.into(),
            server_url: None,
            connector_id: Some(connector_id.into()),
            server_description: None,
            require_approval: None,
            allowed_tools: None,
            authorization: None,
        }
    }
    pub fn with_description(mut self, d: impl Into<String>) -> Self {
        self.server_description = Some(d.into());
        self
    }
    /// Set `require_approval` to `"never"` or `"always"`.
    pub fn with_require_approval(mut self, v: impl Into<String>) -> Self {
        self.require_approval = Some(serde_json::json!(v.into()));
        self
    }
    pub fn with_allowed_tools(mut self, tools: Vec<impl Into<String>>) -> Self {
        self.allowed_tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }
    pub fn with_authorization(mut self, token: impl Into<String>) -> Self {
        self.authorization = Some(token.into());
        self
    }
}

/// A built-in OpenAI tool for the Responses API (and where noted, Chat Completions).
///
/// Use the typed constructors ([`BuiltinTool::file_search`], [`BuiltinTool::mcp`], etc.)
/// or [`BuiltinTool::raw`] for custom tool configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinTool(pub(crate) serde_json::Value);

/// Server-side context compaction configuration for the Responses API.
///
/// When the accumulated context exceeds `compact_threshold` tokens, the server
/// automatically compacts the conversation before generating the next response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextManagementConfig {
    /// Token count that triggers automatic server-side compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact_threshold: Option<u32>,
}

impl ContextManagementConfig {
    pub fn with_compact_threshold(compact_threshold: u32) -> Self {
        Self {
            compact_threshold: Some(compact_threshold),
        }
    }
}

impl BuiltinTool {
    /// Search over uploaded files. Uses OpenAI's vector stores.
    pub fn file_search() -> Self {
        Self(serde_json::json!({"type": "file_search"}))
    }

    /// File search restricted to specific vector store IDs.
    pub fn file_search_with_ids(ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let ids: Vec<String> = ids.into_iter().map(Into::into).collect();
        Self(serde_json::json!({"type": "file_search", "vector_store_ids": ids}))
    }

    /// Code interpreter running in an auto-provisioned OpenAI container.
    pub fn code_interpreter() -> Self {
        Self(serde_json::json!({"type": "code_interpreter", "container": {"type": "auto"}}))
    }

    /// Code interpreter with pre-uploaded file IDs available in the container.
    pub fn code_interpreter_with_files(
        file_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let ids: Vec<String> = file_ids.into_iter().map(Into::into).collect();
        Self(serde_json::json!({
            "type": "code_interpreter",
            "container": {"type": "auto", "file_ids": ids}
        }))
    }

    /// GPT Image generation tool. The model generates images inline during the conversation.
    pub fn image_generation() -> Self {
        Self(serde_json::json!({"type": "image_generation"}))
    }

    /// Computer use (browser/desktop automation).
    ///
    /// `environment`: `"browser"`, `"mac"`, `"windows"`, or `"ubuntu"`.
    pub fn computer_use(
        display_width: u32,
        display_height: u32,
        environment: impl Into<String>,
    ) -> Self {
        Self(serde_json::json!({
            "type": "computer_use_preview",
            "display_width": display_width,
            "display_height": display_height,
            "environment": environment.into(),
        }))
    }

    /// Remote MCP server or OpenAI-maintained connector.
    pub fn mcp(config: McpToolConfig) -> Self {
        Self(serde_json::to_value(config).unwrap_or_else(|e| {
            tracing::debug!("BuiltinTool::mcp: failed to serialize McpToolConfig: {e}");
            serde_json::json!({"type": "mcp"})
        }))
    }

    /// Shell tool in an auto-provisioned OpenAI container.
    pub fn shell_auto() -> Self {
        Self(serde_json::json!({"type": "shell", "environment": {"type": "container_auto"}}))
    }

    /// Shell tool running in your own local runtime.
    pub fn shell_local() -> Self {
        Self(serde_json::json!({"type": "shell", "environment": {"type": "local"}}))
    }

    /// Local shell (`codex-mini-latest` only). Execution runs entirely in your runtime.
    pub fn local_shell() -> Self {
        Self(serde_json::json!({"type": "local_shell"}))
    }

    /// Apply-patch tool for structured file diffs (`gpt-5.1` only).
    pub fn apply_patch() -> Self {
        Self(serde_json::json!({"type": "apply_patch"}))
    }

    /// Escape hatch — provide a raw JSON tool object for custom or future tool types.
    pub fn raw(v: serde_json::Value) -> Self {
        Self(v)
    }

    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }
}

/// Built-in web search tool configuration (Anthropic, OpenAI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Maximum number of web searches the model may perform
    pub max_uses: Option<u32>,
    /// Domain whitelist — only these domains are searched (cannot combine with blocked_domains)
    pub allowed_domains: Option<Vec<String>>,
    /// Domain blacklist — these domains are excluded from results
    pub blocked_domains: Option<Vec<String>>,
    /// Approximate user location for geographically relevant results (OpenAI only).
    pub user_location: Option<WebSearchUserLocation>,
    /// Controls how much web context is fetched per search. One of `"low"`, `"medium"`, `"high"`.
    /// Higher values improve accuracy at greater latency and cost (OpenAI only).
    pub search_context_size: Option<String>,
}

impl WebSearchConfig {
    pub fn new() -> Self {
        Self {
            max_uses: None,
            allowed_domains: None,
            blocked_domains: None,
            user_location: None,
            search_context_size: None,
        }
    }

    pub fn with_max_uses(mut self, max_uses: u32) -> Self {
        self.max_uses = Some(max_uses);
        self
    }

    pub fn with_allowed_domains(mut self, domains: Vec<impl Into<String>>) -> Self {
        self.allowed_domains = Some(domains.into_iter().map(|s| s.into()).collect());
        self
    }

    pub fn with_blocked_domains(mut self, domains: Vec<impl Into<String>>) -> Self {
        self.blocked_domains = Some(domains.into_iter().map(|s| s.into()).collect());
        self
    }

    /// Set approximate user location for geographically relevant results (OpenAI only).
    pub fn with_user_location(mut self, loc: WebSearchUserLocation) -> Self {
        self.user_location = Some(loc);
        self
    }

    /// Set search context size: `"low"`, `"medium"`, or `"high"` (OpenAI only).
    pub fn with_search_context_size(mut self, size: impl Into<String>) -> Self {
        self.search_context_size = Some(size.into());
        self
    }
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self::new()
    }
}
