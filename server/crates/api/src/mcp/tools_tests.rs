use super::*;

/// A span id alone is not a question with one answer, so the tool must decline it.
#[test]
fn a_span_id_without_its_trace_is_refused() {
    assert!(
        span_lacks_its_trace(Some("abc123"), None),
        "a bare span id can match spans in several traces and must be refused"
    );
    assert!(
        !span_lacks_its_trace(Some("abc123"), Some("trace-1")),
        "a span id with its trace identifies one span"
    );
    // Nothing to refuse when no span was asked for.
    assert!(!span_lacks_its_trace(None, None));
    assert!(!span_lacks_its_trace(None, Some("trace-1")));
}

/// A session id does not scope a span lookup, however much it looks as though it should.
///
/// `get_messages` gives `span_id` precedence and never applies `session_id` in that branch, so a
/// span id paired with a session id is still a span id with no trace - and the first version of
/// this guard accepted the pair, with a test that said so.
#[test]
fn a_session_id_does_not_stand_in_for_the_trace() {
    assert!(
        span_lacks_its_trace(Some("abc123"), None),
        "span + session must be refused: the query ignores the session when a span is given"
    );
}

/// Every name the generic guide advertises is a name the guide can actually serve.
///
/// It listed fifteen frameworks from a hand-written string while `get_framework` accepted every entry in
/// the table, so a caller whose framework *was* supported read that it was not. Derived from the table
/// now, and checked in both directions.
#[test]
fn the_advertised_framework_names_are_the_ones_the_guide_serves() {
    let names = supported_framework_names();
    assert!(names.len() > 20, "the table was not read: {names:?}");
    for name in &names {
        assert!(
            get_framework(name).is_some(),
            "the guide advertises `{name}` but cannot resolve it"
        );
    }
    for fw in FRAMEWORKS {
        let name = fw.display.to_lowercase().replace(' ', "-");
        assert!(
            names.contains(&name),
            "{} has an entry but is not advertised",
            fw.display
        );
    }
}

/// A framework's direct-OTLP instrumentation reads the same here as in the telemetry UI.
///
/// Compare per instrumentor because the two surfaces deliberately cover different framework sets.
/// Shared calls must still agree exactly so either copied setup behaves the same.
#[test]
fn a_shared_instrumentor_line_reads_the_same_in_the_telemetry_ui() {
    let ui = include_str!("../../../../../web/src/pages/configuration/telemetry-frameworks.ts");

    /// The instrumentor's name and the whole call, from any line that instruments one.
    fn calls(source: &str) -> std::collections::HashMap<String, String> {
        let mut found = std::collections::HashMap::new();
        for line in source.lines() {
            // Both, in any order: a TS line ends `...)`,` and a Rust one `...)",`.
            let line = line.trim().trim_end_matches(['`', ',', '"', ';']);
            let Some(open) = line.find("Instrumentor().instrument(") else {
                continue;
            };
            // The name is the identifier ending at `Instrumentor`, back to the last non-word char.
            let head = &line[..open + "Instrumentor".len()];
            let start = head
                .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|i| i + 1)
                .unwrap_or(0);
            let name = head[start..].to_string();
            let call = line[start..].trim_end_matches("\\n").to_string();
            found.insert(name, call);
        }
        found
    }

    let mcp: std::collections::HashMap<String, String> = FRAMEWORKS
        .iter()
        .flat_map(|fw| calls(fw.no_sdk_extra_setup))
        .collect();
    let ui_calls = calls(ui);
    assert!(
        mcp.len() > 3 && ui_calls.len() > 3,
        "nothing was parsed, so this test proves nothing: mcp={:?} ui={:?}",
        mcp.keys(),
        ui_calls.keys()
    );

    for (name, mcp_call) in &mcp {
        if let Some(ui_call) = ui_calls.get(name) {
            assert_eq!(
                mcp_call, ui_call,
                "{name} is instrumented differently in the MCP setup guide and the telemetry UI; \
                     one of them is telling users the wrong thing"
            );
        }
    }
}

/// The no-SDK template concatenates extra setup and the snippet, so no executable line may appear in
/// both fragments.
#[test]
fn the_extra_setup_never_repeats_a_line_of_the_snippet() {
    for fw in FRAMEWORKS {
        if fw.no_sdk_extra_setup.is_empty() {
            continue;
        }
        let snippet_lines: std::collections::HashSet<&str> = fw
            .sdk_snippet
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
            .collect();
        for line in fw.no_sdk_extra_setup.lines().map(str::trim) {
            if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
                continue;
            }
            assert!(
                !snippet_lines.contains(line),
                "{}: `{line}` is in both no_sdk_extra_setup and sdk_snippet, and the guide \
                     concatenates them - the generated code declares it twice",
                fw.display
            );
        }
    }
}

#[test]
fn test_python_snippets_have_no_top_level_await() {
    // A bare top-level `await` is a SyntaxError when the snippet is saved and run as a
    // script, which is exactly how a user consumes this guide.
    for name in [
        "strands",
        "langgraph",
        "crewai",
        "autogen",
        "agno",
        "smolagents",
        "ag2",
        "agentscope",
        "langflow",
        "haystack",
        "browser-use",
        "bedrock",
        "anthropic",
        "openai",
        "google-adk",
        "openai-agents",
        "pydantic-ai",
    ] {
        let guide = build_setup_guide_template("http://localhost:5388/otel/default", Some(name));
        for line in guide.lines() {
            let t = line.trim_start();
            let indented = line.len() != t.len();
            if (t.starts_with("await ") || t.contains("= await ")) && !indented {
                panic!("{name}: top-level await is not runnable as a script:\n  {line}");
            }
        }
    }
}

#[test]
fn test_typescript_frameworks_emit_npm_instructions() {
    // Both setup paths must stay in the framework's ecosystem.
    for name in [
        "vercel-ai",
        "strands-typescript",
        "claude-agent-sdk-typescript",
    ] {
        let guide = build_setup_guide_template("http://localhost:5388/otel/default", Some(name));
        assert!(
            guide.contains("npm install @sideseat/sdk"),
            "{name} should emit an npm SDK install, got:\n{guide}"
        );
        assert!(
            guide.contains("```typescript"),
            "{name} should emit TypeScript snippets, got:\n{guide}"
        );
        assert!(
            !guide.contains("pip install"),
            "{name} must not emit pip instructions, got:\n{guide}"
        );
        assert!(
            guide.contains("## Without SDK (direct OTLP)")
                && guide.contains("@opentelemetry/sdk-node"),
            "{name} needs a no-SDK path too, got:\n{guide}"
        );
        // Every symbol the snippet calls must be imported in the same snippet.
        for symbol in ["registerTelemetry", "generateText", "Agent", "query"] {
            if guide.contains(&format!("{symbol}(")) {
                assert!(
                    guide.contains(&format!("import {{ {symbol}"))
                        || guide.contains(&format!(", {symbol} }}"))
                        || guide.contains(&format!("{symbol}, ")),
                    "{name} uses {symbol} without importing it, got:\n{guide}"
                );
            }
        }
    }
}

#[test]
fn test_python_frameworks_still_emit_pip_instructions() {
    for name in ["strands", "langgraph", "crewai"] {
        let guide = build_setup_guide_template("http://localhost:5388/otel/default", Some(name));
        assert!(guide.contains("pip install"), "{name} lost its pip install");
        assert!(
            guide.contains("```python"),
            "{name} lost its Python snippets"
        );
        assert!(!guide.contains("npm install"), "{name} must not emit npm");
    }
}

/// Every Python `sdk_snippet` is self-contained: the guide template supplies only SideSeat/OTel
/// imports, so every other name must be bound by the snippet.
///
/// Checked structurally rather than by running Python, so it needs no interpreter: every
/// capitalised name the snippet calls must be bound by an import or an assignment in the
/// same snippet.
/// Names bound by an import, assignment or def inside the snippet itself.
fn bound_names(snippet: &str) -> Vec<String> {
    let mut bound: Vec<String> = Vec::new();
    for line in snippet.lines() {
        let t = line.trim();
        if let Some(rest) = t
            .strip_prefix("from ")
            .and_then(|r| r.split(" import ").nth(1))
        {
            bound.extend(rest.split(',').map(|n| n.trim().to_string()));
        } else if let Some(rest) = t.strip_prefix("import ") {
            bound.extend(rest.split(',').map(|n| n.trim().to_string()));
        } else if let Some(rest) = t.strip_prefix("async def ").or(t.strip_prefix("def ")) {
            if let Some((name, _)) = rest.split_once('(') {
                bound.push(name.trim().to_string());
            }
        } else if let Some(rest) = t.strip_prefix("async for ").or(t.strip_prefix("for ")) {
            if let Some((name, _)) = rest.split_once(" in ") {
                bound.extend(name.split(',').map(|n| n.trim().to_string()));
            }
        } else if let Some((lhs, _)) = t.split_once('=') {
            let lhs = lhs.trim();
            if !lhs.is_empty() && !lhs.contains(' ') && !lhs.contains('(') {
                bound.push(lhs.to_string());
            }
        }
    }
    bound
}

#[test]
fn test_python_snippets_define_every_name_they_use() {
    for fw in FRAMEWORKS {
        if fw.lang != Lang::Python {
            continue;
        }
        let snippet = fw.sdk_snippet;
        let bound = bound_names(snippet);
        // Any Capitalised identifier immediately followed by `(` is a constructor call.
        let mut used: Vec<String> = Vec::new();
        let bytes: Vec<char> = snippet.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_uppercase()
                && (i == 0
                    || !(bytes[i - 1].is_alphanumeric()
                        || bytes[i - 1] == '_'
                        || bytes[i - 1] == '.'))
            {
                let start = i;
                while i < bytes.len() && (bytes[i].is_alphanumeric() || bytes[i] == '_') {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == '(' {
                    used.push(bytes[start..i].iter().collect());
                }
            } else {
                i += 1;
            }
        }
        for name in used {
            assert!(
                bound.contains(&name),
                "{}: snippet calls `{}` but never imports or defines it:\n{}",
                fw.display,
                name,
                snippet
            );
        }
    }
}

/// The variables a Claude Agent SDK integration cannot work without.
///
/// The Claude Code CLI produces telemetry only when told to, by environment variable, and
/// omitting any one of these yields either no spans or spans with no message content. The
/// list is asserted against every place we hand a user this configuration - see
/// [`claude_configuration_agrees_everywhere_it_is_duplicated`].
const CLAUDE_REQUIRED_ENV: [&str; 9] = [
    "CLAUDE_CODE_ENABLE_TELEMETRY",
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA",
    "ENABLE_BETA_TRACING_DETAILED",
    "BETA_TRACING_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
    "OTEL_TRACES_EXPORTER",
    "OTEL_LOG_USER_PROMPTS",
    "OTEL_LOG_TOOL_DETAILS",
];

/// Every placeholder must be substituted, and the Claude Agent SDK guides must carry the
/// exporter configuration in both languages.
///
/// The CLI subprocess requires the endpoint, both beta tiers and the content flags to emit useful
/// message telemetry.
#[test]
fn test_claude_guides_carry_the_exporter_configuration() {
    for name in ["claude-agent-sdk", "claude-agent-sdk-typescript"] {
        let guide = build_setup_guide("default", Some(name));
        assert!(
            !guide.contains("__OTLP_"),
            "{name}: an OTLP placeholder was not substituted:\n{guide}"
        );
        for required in CLAUDE_REQUIRED_ENV {
            assert!(
                guide.contains(required),
                "{name}: guide omits {required}, so the CLI would emit nothing useful"
            );
        }
    }
}

/// Every maintained copy of the Claude telemetry configuration carries the required variables.
///
/// The copies span executable examples, a script, docs, the UI and this guide, so the test names each
/// surface explicitly and checks it against one required-variable list.
#[test]
fn claude_configuration_agrees_everywhere_it_is_duplicated() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .expect("repo root");

    let copies = [
        "docs/src/content/docs/docs/integrations/frameworks/claude-agent-sdk.mdx",
        "docs/src/content/docs/docs/index.mdx",
        "web/src/pages/configuration/telemetry-frameworks.ts",
        "examples/python/claude-agent-sdk/native.py",
        "scripts/run-claude.sh",
    ];

    for relative in copies {
        let path = repo.join(relative);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{relative}: {e} - was this copy moved or deleted?"));
        for required in CLAUDE_REQUIRED_ENV {
            assert!(
                text.contains(required),
                "{relative} omits {required}, so it disagrees with the MCP setup guide - \
                     the CLI would emit nothing useful for anyone following it"
            );
        }
    }
}

/// TypeScript snippets must declare every identifier they use, same as the Python ones.
#[test]
fn test_typescript_snippets_declare_every_identifier() {
    for fw in FRAMEWORKS {
        if fw.lang != Lang::TypeScript {
            continue;
        }
        let snippet = fw.sdk_snippet;
        let mut bound: Vec<String> = Vec::new();
        for line in snippet.lines() {
            let t = line.trim();
            // `import { a, b } from '...'`
            if let Some(rest) = t.strip_prefix("import {").and_then(|r| r.split('}').next()) {
                bound.extend(rest.split(',').map(|n| n.trim().to_string()));
            }
            // `const x = ...` / `let x = ...`
            for kw in ["const ", "let ", "var "] {
                if let Some(rest) = t.strip_prefix(kw) {
                    let name = rest
                        .split(['=', ':', ' ', '('])
                        .next()
                        .unwrap_or("")
                        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                    if !name.is_empty() {
                        bound.push(name.to_string());
                    }
                    // Destructuring: `const { text } = ...`
                    if rest.trim_start().starts_with('{')
                        && let Some(inner) =
                            rest.split('{').nth(1).and_then(|r| r.split('}').next())
                    {
                        bound.extend(inner.split(',').map(|n| n.trim().to_string()));
                    }
                }
            }
            // `for await (const msg of ...)`
            if let Some(rest) = t.split("const ").nth(1)
                && t.contains(" of ")
                && let Some(name) = rest.split_whitespace().next()
            {
                bound.push(name.to_string());
            }
        }

        // Identifiers the snippet USES, from three shapes:
        //   `{ model }`      shorthand object property
        //   `f(options)`     bare identifier argument
        //   `agent.invoke()` member access
        //
        // A hardcoded list of six names only guarded the cases that had already broken;
        // deleting a `const agent = ...` line passed because `agent` appeared only as member
        // access, which nothing looked at.
        const RUNTIME_GLOBALS: &[&str] = &["process", "console", "JSON", "Math"];
        // Declared by the guide template that wraps every snippet, not by the snippet:
        // `import { init, Frameworks } from '@sideseat/sdk'` on the SDK path and the NodeSDK
        // block on the direct-OTLP path. A snippet using these is correct.
        const TEMPLATE_PROVIDED: &[&str] =
            &["init", "sdk", "query", "generateText", "registerTelemetry"];
        const KEYWORDS: &[&str] = &[
            "const",
            "let",
            "var",
            "await",
            "for",
            "of",
            "new",
            "import",
            "from",
            "async",
            "return",
            "true",
            "false",
            "null",
            "undefined",
        ];
        let plausible = |t: &str| {
            t.len() > 1
                && t.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && !KEYWORDS.contains(&t)
                && !RUNTIME_GLOBALS.contains(&t)
                && !TEMPLATE_PROVIDED.contains(&t)
        };

        // String literals are stripped first: a model id like 'us.anthropic.claude-...' would
        // otherwise be read as member access on an undeclared `us`.
        let mut code = String::with_capacity(snippet.len());
        let mut quote: Option<char> = None;
        for ch in snippet.chars() {
            match quote {
                Some(q) => {
                    if ch == q {
                        quote = None;
                    }
                }
                None => {
                    if ch == '\'' || ch == '"' || ch == '`' {
                        quote = Some(ch);
                    } else {
                        code.push(ch);
                    }
                }
            }
        }
        let snippet_code = code.as_str();

        let mut used: Vec<String> = Vec::new();
        for seg in snippet_code.split(['{', '}', '(', ')', ',', ';']) {
            let t = seg.trim();
            if plausible(t) {
                used.push(t.to_string());
            }
        }
        // Direct callees: `model: bedrock(...)` is a use of `bedrock`, but it matched neither
        // the shorthand-property nor the member-access shape, so deleting its import passed.
        for (idx, _) in snippet_code.match_indices('(') {
            let head: String = snippet_code[..idx]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<Vec<char>>()
                .into_iter()
                .rev()
                .collect();
            // Skip a method call: in `console.log(...)` the callee is `log`, which is not an
            // identifier the snippet must declare - the member-access scan below covers
            // `console` instead.
            let is_method = snippet_code[..idx - head.len()].ends_with('.');
            if !is_method && plausible(&head) {
                used.push(head);
            }
        }
        for tok in snippet_code.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')) {
            if let Some((head, rest)) = tok.split_once('.')
                && !rest.is_empty()
                && plausible(head)
            {
                used.push(head.to_string());
            }
        }
        used.sort();
        used.dedup();

        for token in &used {
            assert!(
                bound.iter().any(|b| b == token),
                "{}: TypeScript snippet uses `{token}` without declaring it:\n{}",
                fw.display,
                snippet
            );
        }
    }
}

/// Lowercase names passed as keyword arguments must be bound in the snippet too.
///
/// This complements the constructor-name check above by covering lowercase placeholders such as
/// `model=model`, `llm=llm`, `llm_config=llm_config` and `retriever`.
#[test]
fn test_python_snippets_have_no_unbound_placeholders() {
    for fw in FRAMEWORKS {
        if fw.lang != Lang::Python {
            continue;
        }
        let bound = bound_names(fw.sdk_snippet);
        for candidate in [
            "model",
            "llm",
            "llm_config",
            "retriever",
            "options",
            "tools",
        ] {
            let used = fw.sdk_snippet.contains(&format!("={candidate})"))
                || fw.sdk_snippet.contains(&format!("={candidate},"));
            if used {
                assert!(
                    bound.iter().any(|b| b == candidate),
                    "{}: snippet passes `{}` but never binds it:\n{}",
                    fw.display,
                    candidate,
                    fw.sdk_snippet
                );
            }
        }
        assert!(
            !fw.sdk_snippet.contains("[...]"),
            "{}: snippet has an elided `[...]` argument, which is not runnable:\n{}",
            fw.display,
            fw.sdk_snippet
        );
    }
}

#[test]
fn test_documented_framework_values_all_resolve() {
    // docs/src/content/docs/docs/mcp.mdx publishes this list to users. A value that
    // does not resolve makes setup_guide silently fall back to the generic guide.
    for name in [
        "strands",
        "langchain",
        "langgraph",
        "crewai",
        "autogen",
        "openai-agents",
        "pydantic-ai",
        "google-adk",
        "agent-framework",
        "claude-agent-sdk",
        "bedrock",
        "openai",
        "anthropic",
        "google-genai",
        "vertex-ai",
        "agno",
        "smolagents",
        "ag2",
        "agentscope",
        "langflow",
        "haystack",
        "browser-use",
        "azure-openai",
        // TypeScript
        "vercel-ai",
        "strands-typescript",
        "claude-agent-sdk-typescript",
    ] {
        assert!(
            get_framework(name).is_some(),
            "documented framework value {name:?} does not resolve"
        );
    }
}

#[test]
fn test_setup_guide_carries_required_sdk_extra() {
    // A framework whose instrumentation lives behind an optional extra must have it on
    // the SDK install line. Without the extra the import fails, instrument() only logs a
    // warning, and the user gets a running app that emits no spans at all.
    for (name, extra) in [
        ("langgraph", "langgraph"),
        ("crewai", "crewai"),
        ("autogen", "autogen"),
        ("bedrock", "aws"),
        ("anthropic", "anthropic"),
        ("vertex-ai", "vertex-ai"),
        ("azure-openai", "azure-openai"),
        ("agentscope", "agentscope"),
    ] {
        let guide = build_setup_guide_template("http://localhost:5388/otel/default", Some(name));
        assert!(
            guide.contains(&format!("pip install \"sideseat[{extra}]\"")),
            "{name} must install sideseat[{extra}], got:\n{guide}"
        );
    }
    // Frameworks that need no extra keep the bare package.
    for name in ["strands", "google-adk", "claude-agent-sdk"] {
        let guide = build_setup_guide_template("http://localhost:5388/otel/default", Some(name));
        assert!(
            guide.contains("pip install sideseat "),
            "{name} should install plain sideseat, got:\n{guide}"
        );
    }
}

#[test]
fn test_setup_guide_uses_current_vertex_ai_google_genai_client() {
    let guide = build_setup_guide_template("http://localhost:5388/otel/default", Some("vertex-ai"));

    assert!(guide.contains("pip install \"sideseat[vertex-ai]\" google-genai"));
    assert!(guide.contains("Frameworks.VertexAI"));
    assert!(guide.contains("genai.Client(enterprise=True"));
    assert!(!guide.contains("genai.Client(vertexai=True"));
    assert!(guide.contains("logfire.instrument_google_genai()"));
    assert!(!guide.contains("vertexai.generative_models"));
    assert!(!guide.contains("VertexAIInstrumentor"));
}

#[test]
fn test_setup_guide_uses_current_azure_openai_v1_instrumentation() {
    let guide = build_setup_guide("demo", Some("azure-openai"));
    assert!(guide.contains("Frameworks.AzureOpenAI"));
    assert!(guide.contains("openai.azure.com/openai/v1/"));
    assert!(guide.contains("OpenAIInstrumentor().instrument(tracer_provider=provider)"));
    assert!(!guide.contains("from openai import AzureOpenAI"));
    assert!(!guide.contains("Frameworks.OpenAI"));
}

#[test]
fn test_setup_guide_resolves_claude_agent_sdk() {
    let guide = build_setup_guide("demo", Some("claude-agent-sdk"));
    assert!(guide.contains("Frameworks.ClaudeAgentSDK"));
    assert!(guide.contains("CLAUDE_CODE_ENHANCED_TELEMETRY_BETA"));
    // Snippets are inserted as values, so the endpoint placeholder must be
    // substituted after formatting or it leaks into the output verbatim.
    assert!(!guide.contains("__OTLP_ENDPOINT__"));
    assert!(guide.contains("http://localhost:5388/otel/demo/v1/traces"));
}

#[test]
fn test_setup_guide_uses_current_agentscope_api_and_middleware() {
    let guide = build_setup_guide("demo", Some("agentscope"));
    assert!(guide.contains("pip install \"sideseat[agentscope]\""));
    assert!(guide.contains("from agentscope.credential import OpenAICredential"));
    assert!(guide.contains("from agentscope.message import UserMsg"));
    assert!(guide.contains("middlewares=[TracingMiddleware()]"));
    assert!(!guide.contains("from agentscope.message import Msg, TextBlock"));
}

#[test]
fn test_setup_guide_matches_framework_aliases() {
    // get_framework() matches on the kebab-cased display name, the lowercased
    // sdk_variant (with and without hyphens), and the pip package.
    for name in [
        "claude-agent-sdk",
        "claudeagentsdk",
        "Claude-Agent-SDK",
        "CLAUDE-AGENT-SDK",
    ] {
        let guide = build_setup_guide("demo", Some(name));
        assert!(
            guide.contains("Frameworks.ClaudeAgentSDK"),
            "'{name}' should resolve to the Claude Agent SDK entry"
        );
    }
}

#[test]
fn optional_timestamp_parses_a_supplied_iso8601_value() {
    let result =
        parse_optional_ts("from_timestamp", Some("2025-01-15T12:00:00Z".to_string())).unwrap();
    assert_eq!(result.unwrap().timestamp(), 1736942400);
}

#[test]
fn optional_timestamp_preserves_an_absent_filter() {
    assert_eq!(parse_optional_ts("from_timestamp", None).unwrap(), None);
}

#[test]
fn optional_timestamp_rejects_an_invalid_supplied_filter() {
    assert!(parse_optional_ts("from_timestamp", Some("not-a-date".to_string())).is_err());
}

#[test]
fn time_window_rejects_reversed_bounds() {
    assert!(
        parse_time_window(
            Some("2025-01-16T12:00:00Z".to_string()),
            Some("2025-01-15T12:00:00Z".to_string())
        )
        .is_err()
    );
}

#[test]
fn time_window_accepts_equal_or_open_bounds() {
    let timestamp = "2025-01-15T12:00:00Z".to_string();
    assert!(parse_time_window(Some(timestamp.clone()), Some(timestamp)).is_ok());
    assert!(parse_time_window(None, Some("2025-01-15T12:00:00Z".to_string())).is_ok());
    assert!(parse_time_window(Some("2025-01-15T12:00:00Z".to_string()), None).is_ok());
}

#[test]
fn required_timestamp_parses_iso8601() {
    assert!(parse_ts("from_timestamp", "2025-01-15T12:00:00Z").is_ok());
}

#[test]
fn required_timestamp_rejects_invalid_input() {
    assert!(parse_ts("from_timestamp", "garbage").is_err());
}

#[test]
fn test_ok_json_serializes() {
    let val = serde_json::json!({"key": "value"});
    let result = ok_json(&val);
    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert!(!call_result.content.is_empty());
}

#[test]
fn test_clamp_page() {
    assert_eq!(clamp_page(None), 1);
    assert_eq!(clamp_page(Some(0)), 1);
    assert_eq!(clamp_page(Some(1)), 1);
    assert_eq!(clamp_page(Some(5)), 5);
    assert_eq!(clamp_page(Some(u32::MAX)), MAX_PAGE);
}

#[test]
fn test_clamp_limit() {
    assert_eq!(clamp_limit(None), 20);
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(1)), 1);
    assert_eq!(clamp_limit(Some(50)), 50);
    assert_eq!(clamp_limit(Some(1000)), MAX_PAGE_LIMIT);
}
