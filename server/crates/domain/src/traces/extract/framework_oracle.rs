//! Test-only framework vocabulary for the detection oracle.
//!
//! It lived in `types::enums` behind `#[cfg(test)]`, which stopped working the moment the DTOs became their own
//! crate: a `cfg(test)` item does not exist for a dependent, so the server's tests could not see it. Making it
//! unconditionally public was the other option and is worse - it would put a list of framework names in the
//! library's API, which is exactly what `no_production_module_names_a_framework` exists to prevent, and what the
//! declaration below already says about itself.
//!
//! So it moved to where its only users are. That is also the honest home: it is the equivalence oracle each
//! retired detection table is compared against, not a shape any port speaks in.

use serde::{Deserialize, Serialize};

/// AI/ML framework identifiers - the *oracle's* vocabulary, not production's.
///
/// Detection produces a label from the assets under `server/assets/rules/`, so nothing in the running server
/// consults this list. It survives only because the equivalence oracle that proves the assets reproduce
/// the table needs the names the table used, and it is `#[cfg(test)]` for the reason the mandate exists:
/// an enum *is* the list of frameworks, so a variant in production code would mean adding a framework is
/// a build rather than an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
// `AgentFramework` is Microsoft's product name, so clippy's "variant name ends with the enum's name" cannot be
// satisfied by renaming: the oracle's spellings have to match what the assets declare, which is the whole point
// of comparing against it. The lint did not fire while this enum lived behind `#[cfg(test)]` in another crate.
#[allow(clippy::enum_variant_names)]
pub enum Framework {
    StrandsAgents,
    LangChain,
    LangGraph,
    LlamaIndex,
    OpenInference,
    AutoGen,
    CrewAI,
    SemanticKernel,
    AzureOpenAI,
    AzureAIFoundry,
    GoogleAdk,
    VertexAI,
    VercelAISdk,
    Logfire,
    MLFlow,
    TraceLoop,
    LiveKit,
    OpenAIAgents,
    AWSBedrock,
    AgentFramework,
    ClaudeAgentSdk,
    Agno,
    Smolagents,
    AgentScope,
    Langflow,
    Ag2,
    Haystack,
    BrowserUse,
    #[default]
    Unknown,
}

impl Framework {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StrandsAgents => "StrandsAgents",
            Self::LangChain => "LangChain",
            Self::LangGraph => "LangGraph",
            Self::LlamaIndex => "LlamaIndex",
            Self::OpenInference => "OpenInference",
            Self::AutoGen => "AutoGen",
            Self::CrewAI => "CrewAI",
            Self::SemanticKernel => "SemanticKernel",
            Self::AzureOpenAI => "AzureOpenAI",
            Self::AzureAIFoundry => "AzureAIFoundry",
            Self::GoogleAdk => "GoogleADK",
            Self::VertexAI => "VertexAI",
            Self::VercelAISdk => "VercelAISDK",
            Self::Logfire => "Logfire",
            Self::MLFlow => "MLflow",
            Self::TraceLoop => "TraceLoop",
            Self::LiveKit => "LiveKit",
            Self::OpenAIAgents => "OpenAIAgents",
            Self::AWSBedrock => "AWSBedrock",
            Self::AgentFramework => "AgentFramework",
            Self::ClaudeAgentSdk => "ClaudeAgentSDK",
            Self::Agno => "Agno",
            Self::Smolagents => "Smolagents",
            Self::AgentScope => "AgentScope",
            Self::Langflow => "Langflow",
            Self::Ag2 => "AG2",
            Self::Haystack => "Haystack",
            Self::BrowserUse => "BrowserUse",
            Self::Unknown => "Unknown",
        }
    }

    /// The framework an SDK *declared* it was configured for, from the slug it writes into
    /// `sideseat.framework`.
    ///
    /// The slugs are the SDKs' own `Frameworks` values, which is why they are lower-case and hyphenated
    /// rather than [`Self::as_str`]'s display form: this parses what a client sends, and the two vocabularies
    /// are allowed to differ because one is a wire value and the other is a label.
    ///
    /// A declaration is only ever a **fallback** - see `detect_framework`. Provider slugs (`bedrock`,
    /// `openai`, …) return `None` on purpose: they say which client library was instrumented, not which
    /// agent framework produced the span, and the provider is already recorded separately.
    pub fn from_sdk_slug(slug: &str) -> Option<Self> {
        Some(match slug.trim() {
            "strands" => Self::StrandsAgents,
            "vercel-ai" => Self::VercelAISdk,
            "langchain" => Self::LangChain,
            "langgraph" => Self::LangGraph,
            "llama-index" => Self::LlamaIndex,
            "crewai" => Self::CrewAI,
            "autogen" => Self::AutoGen,
            "ag2" => Self::Ag2,
            "openai-agents" => Self::OpenAIAgents,
            "google-adk" => Self::GoogleAdk,
            "agent-framework" => Self::AgentFramework,
            "claude-agent-sdk" => Self::ClaudeAgentSdk,
            "agno" => Self::Agno,
            "smolagents" => Self::Smolagents,
            "agentscope" => Self::AgentScope,
            "langflow" => Self::Langflow,
            "haystack" => Self::Haystack,
            "browser-use" => Self::BrowserUse,
            "semantic-kernel" => Self::SemanticKernel,
            "azure-openai" => Self::AzureOpenAI,
            "azure-ai-foundry" => Self::AzureAIFoundry,
            "vertex-ai" => Self::VertexAI,
            "logfire" => Self::Logfire,
            "mlflow" => Self::MLFlow,
            "traceloop" => Self::TraceLoop,
            "livekit" => Self::LiveKit,
            // `pydantic-ai` has no Framework of its own: its spans are OpenInference-shaped and the
            // extractor reads them as such, so claiming a distinct framework would contradict what the
            // detection rules say about the very same span.
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moved here with the enum. It asserts the oracle's own spellings, which are what the retired detection
    /// tables are compared against - so it belongs beside them rather than in the DTO crate.
    #[test]
    fn test_framework_as_str() {
        assert_eq!(Framework::StrandsAgents.as_str(), "StrandsAgents");
        assert_eq!(Framework::LangChain.as_str(), "LangChain");
        assert_eq!(Framework::ClaudeAgentSdk.as_str(), "ClaudeAgentSDK");
        assert_eq!(Framework::Unknown.as_str(), "Unknown");
    }
}
