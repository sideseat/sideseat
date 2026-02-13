import { useState, useMemo } from "react";
import { Check, ChevronsUpDown, Copy, ExternalLink, Search } from "lucide-react";
import { toast } from "sonner";
import { useQueryParam, StringParam, withDefault } from "use-query-params";

import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useProjects } from "@/api/projects/hooks/queries";
import { cn } from "@/lib/utils";

type Tool = {
  name: string;
  description: string;
};

const TOOLS: Tool[] = [
  {
    name: "list_traces",
    description: "List recent runs with summaries, tokens, costs, and error status",
  },
  { name: "list_sessions", description: "List multi-turn sessions grouping related runs" },
  { name: "list_spans", description: "Search operations by type, model, framework, or status" },
  { name: "get_trace", description: "Get execution structure: span tree with timing and models" },
  {
    name: "get_messages",
    description: "Get normalized conversation with roles and content blocks",
  },
  { name: "get_raw_span", description: "Get raw OTLP span data for debugging" },
  {
    name: "get_stats",
    description: "Cost and token analytics by model, framework, and time period",
  },
];

type ClientConfig = {
  id: string;
  name: string;
  cli?: string;
  configFile: string;
  configContent: string;
  deepLink?: { label: string; url: string };
};

function getMcpEndpoint(projectId: string) {
  const hostname = window.location.hostname;
  const port = window.location.port || "5388";
  return `http://${hostname}:${port}/api/v1/projects/${projectId}/mcp`;
}

function ProjectSelector({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const { data, isLoading } = useProjects();

  const projects = useMemo(() => {
    const list = data?.data ?? [];
    return [...list].sort((a, b) => {
      if (a.id === "default") return -1;
      if (b.id === "default") return 1;
      return a.name.localeCompare(b.name);
    });
  }, [data?.data]);

  const filteredProjects = useMemo(() => {
    if (!search.trim()) return projects;
    const lowerSearch = search.toLowerCase();
    return projects.filter(
      (p) => p.name.toLowerCase().includes(lowerSearch) || p.id.toLowerCase().includes(lowerSearch),
    );
  }, [projects, search]);

  const selectedProject = useMemo(() => projects.find((p) => p.id === value), [projects, value]);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="h-10 w-full justify-between font-normal sm:w-80"
        >
          {isLoading ? (
            <span className="text-muted-foreground">Loading...</span>
          ) : selectedProject ? (
            <span className="truncate">{selectedProject.name}</span>
          ) : (
            <span className="text-muted-foreground">Select project...</span>
          )}
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[--radix-popover-trigger-width] p-0 sm:w-80" align="start">
        <div className="p-2">
          <div className="flex items-center rounded-md border px-3 py-2 ring-offset-background focus-within:ring-2 focus-within:ring-ring">
            <Search className="mr-2 h-4 w-4 shrink-0 text-muted-foreground" />
            <input
              type="text"
              placeholder="Search projects..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="w-full bg-transparent text-sm placeholder:text-muted-foreground border-0 outline-none focus:outline-none! focus:ring-0!"
            />
          </div>
        </div>
        <div className="max-h-64 overflow-y-auto p-1">
          {filteredProjects.length === 0 ? (
            <p className="py-6 text-center text-sm text-muted-foreground">No projects found.</p>
          ) : (
            filteredProjects.map((project) => {
              const isSelected = value === project.id;
              return (
                <button
                  key={project.id}
                  onClick={() => {
                    onChange(project.id);
                    setOpen(false);
                    setSearch("");
                  }}
                  className={cn(
                    "flex w-full items-center justify-between rounded-md px-3 py-2 text-sm outline-none transition-colors",
                    "hover:bg-accent",
                    isSelected && "bg-accent",
                  )}
                >
                  <div className="flex min-w-0 flex-col items-start">
                    <span className={cn("truncate", isSelected && "font-medium")}>
                      {project.name}
                    </span>
                    <span className="truncate text-xs text-muted-foreground">{project.id}</span>
                  </div>
                  {isSelected && <Check className="ml-2 h-4 w-4 shrink-0" />}
                </button>
              );
            })
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      toast.success(`${label} copied`);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error("Failed to copy");
    }
  };

  return (
    <Button
      variant="ghost"
      size="icon"
      className="absolute right-2 top-2 h-7 w-7 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
      onClick={handleCopy}
      aria-label={`Copy ${label}`}
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </Button>
  );
}

function CodeBlock({ code, label }: { code: string; label: string }) {
  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-lg border border-zinc-800 bg-zinc-950 p-3 pr-12 font-mono text-xs text-zinc-100 sm:p-4 sm:text-sm">
        <code>{code}</code>
      </pre>
      <CopyButton text={code} label={label} />
    </div>
  );
}

function ClientCard({ client }: { client: ClientConfig }) {
  return (
    <div className="space-y-3 rounded-lg border bg-muted/30 p-3 sm:p-4">
      <div className="flex items-center justify-between">
        <p className="text-sm font-medium">{client.name}</p>
        {client.deepLink && (
          <Button variant="outline" size="sm" className="h-7 gap-1.5 text-xs" asChild>
            <a href={client.deepLink.url}>
              <ExternalLink className="h-3 w-3" />
              {client.deepLink.label}
            </a>
          </Button>
        )}
      </div>
      {client.cli && (
        <div className="space-y-1.5">
          <p className="text-xs text-muted-foreground">CLI</p>
          <CodeBlock code={client.cli} label={`${client.name} CLI command`} />
        </div>
      )}
      <div className="space-y-1.5">
        <p className="text-xs text-muted-foreground">{client.configFile}</p>
        <CodeBlock code={client.configContent} label={`${client.name} config`} />
      </div>
    </div>
  );
}

export default function McpPage() {
  const [projectId, setProjectId] = useQueryParam("project", withDefault(StringParam, "default"));
  const endpoint = useMemo(() => getMcpEndpoint(projectId), [projectId]);

  const clients: ClientConfig[] = useMemo(
    () => [
      {
        id: "kiro",
        name: "Kiro / Kiro CLI",
        cli: `kiro-cli mcp add --name sideseat --url ${endpoint}`,
        configFile: ".kiro/settings/mcp.json",
        configContent: JSON.stringify(
          {
            mcpServers: {
              sideseat: { url: endpoint },
            },
          },
          null,
          2,
        ),
        deepLink: {
          label: "Install in Kiro",
          url: `kiro://kiro.mcp/add?name=sideseat&config=${encodeURIComponent(JSON.stringify({ url: endpoint }))}`,
        },
      },
      {
        id: "claude-code",
        name: "Claude Code",
        cli: `claude mcp add --transport http sideseat ${endpoint}`,
        configFile: ".mcp.json",
        configContent: JSON.stringify(
          {
            mcpServers: {
              sideseat: { type: "streamable-http", url: endpoint },
            },
          },
          null,
          2,
        ),
      },
      {
        id: "codex",
        name: "Codex",
        cli: `codex mcp add --transport http sideseat ${endpoint}`,
        configFile: "~/.codex/config.toml",
        configContent: `[mcp_servers.sideseat]\nurl = "${endpoint}"`,
      },
      {
        id: "cursor",
        name: "Cursor",
        configFile: ".cursor/mcp.json",
        configContent: JSON.stringify(
          {
            mcpServers: {
              sideseat: { type: "streamable-http", url: endpoint },
            },
          },
          null,
          2,
        ),
        deepLink: {
          label: "Install in Cursor",
          url: `cursor://anysphere.cursor-deeplink/mcp/install?name=sideseat&config=${btoa(JSON.stringify({ type: "streamable-http", url: endpoint }))}`,
        },
      },
    ],
    [endpoint],
  );

  return (
    <div className="space-y-6 sm:space-y-8">
      {/* Header */}
      <div>
        <h2 className="text-xl font-semibold tracking-tight">MCP Server</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Connect AI coding agents to your observability data for prompt optimization, debugging,
          and cost analysis.
        </p>
      </div>

      {/* Project Selector */}
      <section className="space-y-3 sm:space-y-4">
        <div>
          <h3 className="text-sm font-medium">Project</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Select the project to generate MCP configuration for.
          </p>
        </div>
        <ProjectSelector value={projectId} onChange={setProjectId} />
      </section>

      {/* Endpoint */}
      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-medium">Endpoint</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Streamable HTTP endpoint for MCP clients.
          </p>
        </div>
        <CodeBlock code={endpoint} label="MCP endpoint" />
      </section>

      {/* Client setup */}
      <section className="space-y-3 sm:space-y-4">
        <div>
          <h3 className="text-sm font-medium">Connect your tool</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Use the CLI command for a one-liner setup, or add the config file to your project.
          </p>
        </div>
        <div className="space-y-4">
          {clients.map((client) => (
            <ClientCard key={client.id} client={client} />
          ))}
        </div>
      </section>

      {/* Available tools */}
      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-medium">Available tools</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            7 tools for discovering, inspecting, and analyzing agent runs.
          </p>
        </div>
        <div className="rounded-lg border">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b bg-muted/50">
                <th className="px-3 py-2 text-left font-medium">Tool</th>
                <th className="px-3 py-2 text-left font-medium">Description</th>
              </tr>
            </thead>
            <tbody>
              {TOOLS.map((tool) => (
                <tr key={tool.name} className="border-b last:border-0">
                  <td className="px-3 py-2 font-mono text-xs">{tool.name}</td>
                  <td className="px-3 py-2 text-muted-foreground">{tool.description}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {/* Verify */}
      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-medium">Verify connection</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Test with the MCP Inspector to confirm tools are available.
          </p>
        </div>
        <CodeBlock
          code={`npx @modelcontextprotocol/inspector ${endpoint}`}
          label="MCP Inspector command"
        />
      </section>
    </div>
  );
}
