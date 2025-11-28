# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## AI Guidance

- Ignore GEMINI.md and GEMINI-\*.md files
- To save main context space, for code searches, inspections, troubleshooting or analysis, use code-searcher subagent where appropriate - giving the subagent full context background for the task(s) you assign it.
- After receiving tool results, carefully reflect on their quality and determine optimal next steps before proceeding. Use your thinking to plan and iterate based on this new information, and then take the best next action.
- For maximum efficiency, whenever you need to perform multiple independent operations, invoke all relevant tools simultaneously rather than sequentially.
- Before you finish, please verify your solution
- Do what has been asked; nothing more, nothing less.
- NEVER create files unless they're absolutely necessary for achieving your goal.
- ALWAYS prefer editing an existing file to creating a new one.
- NEVER proactively create documentation files (\*.md) or README files. Only create documentation files if explicitly requested by the User.
- When you update or modify core context files, also update markdown documentation and memory bank
- When asked to commit changes, exclude CLAUDE.md and CLAUDE-\*.md referenced memory bank system files from any commits. Never delete these files.
- **NEVER run e2e tests (`make test-e2e`) unless explicitly asked by the user.** E2E tests are slow and resource-intensive. Only run unit tests (`cargo test`) for verification unless instructed otherwise.

## Documentation Standards

- **Mermaid for diagrams** - All diagrams in Markdown documentation MUST use Mermaid syntax. Never use ASCII art, images, or other diagram formats.

## Code Comments

Follow these rules for comments:

- **Minimal comments** - Code should be self-documenting. Only add comments when the code cannot explain itself.
- **No process comments** - Never describe changes, migrations, or history (e.g., "moved from X", "was previously Y", "now handled by Z"). Comments describe the current state, not how we got here.
- **Why, not what** - Explain *why* something is done if not obvious, never *what* the code does (the code shows that).
- **No redundant comments** - Don't repeat what the code says: `// Increment counter` before `counter += 1`
- **Doc comments for public API** - Use `///` for public functions, structs, and modules to describe purpose and usage.
- **Keep comments current** - Outdated comments are worse than no comments. Delete rather than leave stale.

**Good:**
```rust
// Foreign keys must be enabled per-connection in SQLite
.foreign_keys(true)

/// Initialize the database with schema migrations.
pub async fn init(path: &Path) -> Result<Self>
```

**Bad:**
```rust
// Now handled by DatabaseManager (previously in OtelManager)
// TODO: Refactor this later
// This function increments the counter
counter += 1;
```

## Memory Bank System

This project uses a structured memory bank system with specialized context files. Always check these files for relevant information before starting work:

### Core Context Files

- **CLAUDE-activeContext.md** - Current session state, goals, and progress (if exists)
- **CLAUDE-patterns.md** - Established code patterns and conventions (if exists)
- **CLAUDE-decisions.md** - Architecture decisions and rationale (if exists)
- **CLAUDE-troubleshooting.md** - Common issues and proven solutions (if exists)
- **CLAUDE-config-variables.md** - Configuration variables reference (if exists)
- **CLAUDE-temp.md** - Temporary scratch pad (only read when referenced)

**Important:** Always reference the active context file first to understand what's currently being worked on and maintain session continuity.

### Memory Bank System Backups

When asked to backup Memory Bank System files, you will copy the core context files above and @.claude settings directory to directory @/path/to/backup-directory. If files already exist in the backup directory, you will overwrite them.

## Claude Code Official Documentation

When working on Claude Code features (hooks, skills, subagents, MCP servers, etc.), use the `claude-docs-consultant` skill to selectively fetch official documentation from docs.claude.com.

## ALWAYS START WITH THESE COMMANDS FOR COMMON TASKS

**Task: "List/summarize all files and directories"**

```bash
fd . -t f           # Lists ALL files recursively (FASTEST)
# OR
rg --files          # Lists files (respects .gitignore)
```

**Task: "Search for content in files"**

```bash
rg "search_term"    # Search everywhere (FASTEST)
```

**Task: "Find files by name"**

```bash
fd "filename"       # Find by name pattern (FASTEST)
```

### Directory/File Exploration

```bash
# FIRST CHOICE - List all files/dirs recursively:
fd . -t f           # All files (fastest)
fd . -t d           # All directories
rg --files          # All files (respects .gitignore)

# For current directory only:
ls -la              # OK for single directory view
```

### BANNED - Never Use These Slow Tools

- ❌ `tree` - NOT INSTALLED, use `fd` instead
- ❌ `find` - use `fd` or `rg --files`
- ❌ `grep` or `grep -r` - use `rg` instead
- ❌ `ls -R` - use `rg --files` or `fd`
- ❌ `cat file | grep` - use `rg pattern file`

### Use These Faster Tools Instead

```bash
# ripgrep (rg) - content search
rg "search_term"                # Search in all files
rg -i "case_insensitive"        # Case-insensitive
rg "pattern" -t py              # Only Python files
rg "pattern" -g "*.md"          # Only Markdown
rg -1 "pattern"                 # Filenames with matches
rg -c "pattern"                 # Count matches per file
rg -n "pattern"                 # Show line numbers
rg -A 3 -B 3 "error"            # Context lines
rg " (TODO| FIXME | HACK)"      # Multiple patterns

# ripgrep (rg) - file listing
rg --files                      # List files (respects •gitignore)
rg --files | rg "pattern"       # Find files by name
rg --files -t md                # Only Markdown files

# fd - file finding
fd -e js                        # All •js files (fast find)
fd -x command {}                # Exec per-file
fd -e md -x ls -la {}           # Example with ls

# jq - JSON processing
jq. data.json                   # Pretty-print
jq -r .name file.json           # Extract field
jq '.id = 0' x.json             # Modify field
```

### Search Strategy

1. Start broad, then narrow: `rg "partial" | rg "specific"`
2. Filter by type early: `rg -t python "def function_name"`
3. Batch patterns: `rg "(pattern1|pattern2|pattern3)"`
4. Limit scope: `rg "pattern" src/`

### INSTANT DECISION TREE

```
User asks to "list/show/summarize/explore files"?
  → USE: fd . -t f  (fastest, shows all files)
  → OR: rg --files  (respects .gitignore)

User asks to "search/grep/find text content"?
  → USE: rg "pattern"  (NOT grep!)

User asks to "find file/directory by name"?
  → USE: fd "name"  (NOT find!)

User asks for "directory structure/tree"?
  → USE: fd . -t d  (directories) + fd . -t f  (files)
  → NEVER: tree (not installed!)

Need just current directory?
  → USE: ls -la  (OK for single dir)
```

## Project Overview

SideSeat is an AI Development Toolkit with a Rust backend server and embedded frontend.

### Project Structure

```
server/
├── src/
│   ├── main.rs           # CLI entry point (clap)
│   ├── lib.rs            # Library root, exports run()
│   ├── error.rs          # Error types (thiserror)
│   ├── api/              # API route handlers
│   │   ├── mod.rs
│   │   ├── routes.rs
│   │   ├── auth.rs       # Auth endpoints
│   │   ├── health.rs     # Health check endpoint
│   │   └── otel/         # OTel query API endpoints
│   ├── auth/             # Authentication module
│   │   ├── mod.rs
│   │   ├── manager.rs    # AuthManager - JWT + bootstrap tokens
│   │   ├── jwt.rs        # JWT creation/validation
│   │   └── middleware.rs # Auth middleware
│   ├── core/             # Core managers and utilities
│   │   ├── mod.rs
│   │   ├── constants.rs  # App-wide constants, env var names
│   │   ├── config.rs     # ConfigManager - multi-source config
│   │   ├── storage.rs    # StorageManager - platform paths
│   │   ├── secrets.rs    # SecretManager - credential storage
│   │   └── utils.rs      # Terminal utilities
│   ├── otel/             # OpenTelemetry collector
│   │   ├── mod.rs        # OtelManager - central orchestrator
│   │   ├── error.rs      # OTel-specific errors
│   │   ├── health.rs     # Health status types
│   │   ├── ingest/       # OTLP ingestion (HTTP + gRPC)
│   │   ├── normalize/    # Framework detection, field extraction
│   │   ├── storage/      # SQLite index + Parquet bulk storage
│   │   ├── query/        # Query engine (SQLite + DataFusion)
│   │   └── realtime/     # SSE subscriptions
│   └── server/           # HTTP/gRPC server
│       ├── mod.rs        # Server startup
│       ├── banner.rs     # Startup banner
│       ├── embedded.rs   # Frontend asset serving
│       ├── grpc.rs       # gRPC OTLP server
│       ├── handlers.rs   # 404 handler
│       └── middleware.rs # CORS middleware
docs/                     # Astro documentation site
ui/                       # Frontend (embedded into server)
```

### Core Managers

The `server/src/core/` module contains three managers that work together:

#### StorageManager

Platform-aware storage directory management. Initialize first.

```rust
let storage = StorageManager::init().await?;
// Paths: config_dir(), data_dir(), cache_dir(), logs_dir(), temp_dir()
// User config: user_config_dir() -> ~/.sideseat/ (not auto-created)
// Subdirs: data_subdir(DataSubdir::Database), data_subdir(DataSubdir::Uploads)
```

#### ConfigManager

Multi-source configuration with priority: CLI > ENV > workdir > user_config.

```rust
let config_manager = ConfigManager::init(&storage, &cli_config)?;
let config = config_manager.config();
// config.server.host, config.server.port
// config.logging.level, config.logging.format
```

#### SecretManager

Cross-platform credential storage using OS-native backends (macOS Keychain, Windows Credential Manager, Linux Secret Service). All secrets are stored in a single keychain entry (vault) to minimize permission prompts.

```rust
let secrets = SecretManager::init().await?;
secrets.set_api_key("OPENAI_API_KEY", "sk-xxx", Some("openai")).await?;
let value = secrets.get_value("OPENAI_API_KEY").await?;
```

**Important:**
- All secrets (including JWT signing keys) MUST be stored via SecretManager, never in plain files
- On macOS, the keychain prompts once on first access - click "Always Allow" to grant permanent access to all secrets
- The vault design ensures a single keychain entry for all secrets, so one permission grants access to everything

#### OtelManager

OpenTelemetry collector for AI agent observability. Handles OTLP ingestion, storage, and real-time streaming.

```rust
let otel = OtelManager::init(&storage, config.otel.clone()).await?;
// Access components:
// otel.sender() - Channel for ingesting spans
// otel.query_engine - SQLite queries
// otel.storage - Parquet + SQLite storage
// otel.sse - Real-time SSE subscriptions
```

**Architecture:**
- **Ingestion**: HTTP (`/otel/v1/traces`) and gRPC (port 4317) OTLP endpoints
- **Storage**: SQLite for indexing + Parquet for bulk span data
- **Query**: SQLite for indexed queries, DataFusion for analytics
- **Real-time**: SSE subscriptions with filtered events

### Configuration

**Priority (highest to lowest):**

1. CLI args: `--host`, `--port` (or `-H`, `-p`)
2. Environment: `SIDESEAT_HOST`, `SIDESEAT_PORT`, `SIDESEAT_LOG`
3. Workdir config: `./sideseat.json`
4. User config: `~/.sideseat/config.json`

**Config file format:**

```json
{
  "server": { "host": "127.0.0.1", "port": 5001 },
  "logging": { "level": "info", "format": "compact" },
  "auth": { "enabled": true },
  "otel": {
    "enabled": true,
    "grpc_enabled": true,
    "grpc_port": 4317,
    "retention_max_gb": 20
  }
}
```

**OTel Configuration Options:**
- `otel.enabled` - Enable/disable OTel collector (default: true)
- `otel.grpc_enabled` - Enable gRPC OTLP endpoint (default: true)
- `otel.grpc_port` - gRPC port (default: 4317)
- `otel.retention_days` - Optional max age for traces
- `otel.retention_max_gb` - Max storage size in GB (default: 20)

### Frontend (shadcn/ui)

The frontend uses [shadcn/ui](https://ui.shadcn.com) components. Reference: https://ui.shadcn.com/llms.txt

#### Form Fields

**Always use the Field component for form inputs** (`@/components/ui/field`):

**Field components:**

- `Field` - Wrapper with `data-invalid` for error state
- `FieldLabel` - Label with `htmlFor` attribute
- `FieldDescription` - Helper text below input
- `FieldError` - Error message (only render when error exists)
- `FieldGroup` - Groups multiple fields
- `FieldSet` / `FieldLegend` - Semantic fieldset grouping

### API Endpoints

**Public Endpoints (no auth):**
- `GET /api/v1/health` - Health check with OTel status
- `POST /api/v1/auth/login` - Exchange bootstrap token for JWT
- `GET /api/v1/auth/status` - Check auth status
- `POST /api/v1/auth/logout` - Clear session

**OTel Query Endpoints (no auth):**
- `GET /api/v1/traces` - List traces with filters
- `GET /api/v1/traces/{trace_id}` - Get trace details
- `DELETE /api/v1/traces/{trace_id}` - Soft delete trace
- `GET /api/v1/traces/filters` - Get available filter options
- `GET /api/v1/traces/sse` - Real-time trace updates (SSE)
- `GET /api/v1/spans` - Query spans with filters
- `GET /api/v1/sessions` - List sessions with filters
- `GET /api/v1/sessions/{session_id}` - Get session details
- `DELETE /api/v1/sessions/{session_id}` - Soft delete session
- `GET /api/v1/sessions/{session_id}/traces` - Get traces for a session

**OTel Collector Endpoints (no auth, OTLP standard):**
- `POST /otel/v1/traces` - OTLP HTTP trace ingestion
- `gRPC :4317` - OTLP gRPC trace ingestion

#### Pagination

All list endpoints use **cursor-based pagination** with consistent response format:

**Request params:**
- `cursor` - Opaque cursor string from previous response
- `limit` - Max items per page (default: 50, max: 100)

**Response format:**
```json
{
  "traces": [...],
  "next_cursor": "eyJ0aW1lc3RhbXAiOjE3MzI...",
  "has_more": true
}
```

**Example:**
```bash
# First page
curl "/api/v1/traces?limit=20"

# Next page (use next_cursor from previous response)
curl "/api/v1/traces?limit=20&cursor=eyJ0aW1lc3RhbXAiOjE3MzI..."
```

### Development Commands

```bash
make dev        # Start dev server (hot reload)
make build      # Production build
make test       # Run all tests
make coverage   # Run test coverage (requires cargo-llvm-cov)
make lint       # Run linters
make fmt        # Format code
```
