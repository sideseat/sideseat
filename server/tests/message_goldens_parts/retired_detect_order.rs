// The detection order as it stood before `supersedes` stopped executing: each clause's rank and the edges it
// inherited, frozen from the assets of that commit (`18e106af`).
//
// The oracle half of that migration. The retired resolver ran over these numbers and edges against the *current*
// predicates (which the migration did not change), so a span labelled differently by the two plans is a
// difference of precedence alone - and every such difference is one the migration states.

/// `(clause id, retired legacy_rank, the supersedes edges it executed)`, in declaration order.
const RETIRED_ORDER: &[(&str, i32, &[&str])] = &[
    ("ag2.detect", 110, &["openinference.detect"]),
    ("agent-framework.detect", 250, &[]),
    ("agentscope.detect", 90, &["openinference.detect"]),
    ("agno.detect", 70, &["openinference.detect"]),
    ("autogen.detect", 10, &[]),
    ("azure-ai-foundry.detect", 170, &[]),
    (
        "azure-openai.detect",
        160,
        &["azure-ai-foundry.detect", "openinference.detect"],
    ),
    (
        "azure-openai.detect.openinference",
        139,
        &["azure-ai-foundry.detect", "openinference.detect"],
    ),
    ("bedrock.detect", 260, &[]),
    ("browser-use.detect", 130, &[]),
    ("browser-use.detect.laminar_agent_run", 131, &[]),
    ("claude-agent-sdk.detect", 270, &[]),
    ("codex.detect", 285, &[]),
    ("crewai.detect", 30, &[]),
    ("genkit.detect", 217, &[]),
    ("google-adk.detect", 20, &[]),
    ("haystack.detect", 120, &["openinference.detect"]),
    ("langchain.detect", 50, &["openinference.detect"]),
    ("langflow.detect", 100, &["openinference.detect"]),
    ("langfuse.detect", 215, &[]),
    ("langgraph.detect", 40, &["langchain.detect"]),
    ("livekit.detect", 230, &[]),
    ("llamaindex.detect", 60, &["openinference.detect"]),
    ("logfire.detect", 200, &[]),
    ("mlflow.detect", 210, &[]),
    ("openai-agents.detect", 240, &["logfire.detect"]),
    ("openinference.detect", 140, &[]),
    ("semantic-kernel.detect", 150, &[]),
    ("smolagents.detect", 80, &["openinference.detect"]),
    ("strands.detect", 280, &[]),
    ("strands.detect.self_identified", 135, &[]),
    ("traceloop.detect", 220, &[]),
    ("vercel-ai.detect", 190, &[]),
    ("vertex-ai.detect", 180, &["logfire.detect"]),
    (
        "vertex-ai.detect.legacy-openllmetry",
        181,
        &["logfire.detect"],
    ),
];

/// `(current clause id, the retired clause it was split out of)`. A retired clause matched where any current clause
/// split from it matches, since a split divides one disjunction of signals between two clauses.
const CLAUSE_ORIGIN: &[(&str, &str)] = &[
    (
        "openai-agents.detect.service_name_under_logfire",
        "openai-agents.detect",
    ),
    ("openai-agents.detect.service_name", "openai-agents.detect"),
];
