import { useState, useMemo } from "react";
import { Link } from "react-router";
import { Check, ChevronsUpDown, Copy, ExternalLink, Search } from "lucide-react";
import { toast } from "sonner";
import { useQueryParam, StringParam, withDefault } from "use-query-params";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useProjects } from "@/api/projects/hooks/queries";
import { cn } from "@/lib/utils";

import { FRAMEWORKS } from "./telemetry-frameworks";

function usePorts() {
  return useMemo(() => {
    const hostname = window.location.hostname;
    const httpPort = window.location.port || "5388";
    const grpcPort = "4317";
    return { hostname, httpPort, grpcPort };
  }, []);
}

function getEndpoint(hostname: string, httpPort: string, projectId: string) {
  return `http://${hostname}:${httpPort}/otel/${projectId}`;
}

function CodeBlock({ code, label, lang }: { code: string; label: string; lang?: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      toast.success(`${label} copied to clipboard`);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  };

  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-lg border border-zinc-800 bg-zinc-950 p-3 pr-12 font-mono text-xs text-zinc-100 sm:p-4 sm:text-sm">
        <code data-lang={lang}>{code}</code>
      </pre>
      <Button
        variant="ghost"
        size="icon"
        className="absolute right-2 top-2 h-7 w-7 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
        onClick={handleCopy}
        aria-label={`Copy ${label}`}
      >
        {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
      </Button>
    </div>
  );
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

/** Provider setup shared by every "without the SDK" snippet.
 *
 * The altCode entries only show the instrumentor call, which references `provider`. Without
 * this block in front of them the snippet raises NameError when pasted. The endpoint comes
 * from OTEL_EXPORTER_OTLP_ENDPOINT, shown just above in the same panel, which both OTel
 * SDKs expand to `<base>/v1/traces` on their own.
 */
function providerSetup(lang: "python" | "javascript"): string {
  if (lang === "javascript") {
    return `import { NodeSDK } from '@opentelemetry/sdk-node';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';

const sdk = new NodeSDK({ traceExporter: new OTLPTraceExporter() });
sdk.start();

// Flush before the process exits, or a short-lived script can lose its spans.
process.on('beforeExit', async () => {
  await sdk.shutdown();
});`;
  }
  return `from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)`;
}

function transformCode(
  code: string,
  lang: "python" | "javascript",
  opts: { useApiKey: boolean; projectId: string },
): string {
  const { useApiKey, projectId } = opts;
  const nonDefaultProject = projectId !== "default";

  if (lang === "javascript") {
    if (!useApiKey && !nonDefaultProject) return code;
    const extraArgs: string[] = [];
    if (nonDefaultProject) extraArgs.push(`projectId: "${projectId}"`);
    if (useApiKey) extraArgs.push("apiKey: process.env.SIDESEAT_API_KEY");
    return code.replace(/init\((\{[^}]*\}|)\)/, (_, existing) => {
      const inner = existing ? existing.slice(1, -1).trim() : "";
      const parts = [inner, ...extraArgs].filter(Boolean);
      return `init({ ${parts.join(", ")} })`;
    });
  }

  if (!code.includes("SideSeat(")) return code;

  const extraArgs: string[] = [];
  if (nonDefaultProject) extraArgs.push(`project_id="${projectId}"`);
  if (useApiKey) extraArgs.push('api_key=os.environ["SIDESEAT_API_KEY"]');

  if (extraArgs.length === 0) return code;

  const withImport = useApiKey && !code.includes("import os\n") ? "import os\n" + code : code;
  return withImport.replace(
    /SideSeat\(framework=([^)]+)\)/,
    `SideSeat(framework=$1, ${extraArgs.join(", ")})`,
  );
}

export default function TelemetryPage() {
  const [selectedFramework, setSelectedFramework] = useState<string>("bedrock");
  const [useApiKey, setUseApiKey] = useState(false);
  const [projectId, setProjectId] = useQueryParam("project", withDefault(StringParam, "default"));
  const { hostname, httpPort } = usePorts();
  const endpoint = getEndpoint(hostname, httpPort, projectId);
  const framework = FRAMEWORKS.find((f) => f.id === selectedFramework) ?? FRAMEWORKS[0];

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h2 className="text-xl font-semibold tracking-tight">Telemetry Setup</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Capture agent runs in your local workbench. Pick a framework and add a few lines of code.
        </p>
      </div>

      {/* Project Selector */}
      <section className="space-y-3 sm:space-y-4">
        <div>
          <h3 className="text-sm font-medium">Project</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Select the project to send telemetry data to.
          </p>
        </div>
        <ProjectSelector value={projectId} onChange={setProjectId} />
        <div className="flex items-center gap-2">
          <Checkbox
            id="use-api-key"
            checked={useApiKey}
            onCheckedChange={(checked) => setUseApiKey(checked === true)}
          />
          <Label htmlFor="use-api-key" className="text-sm font-normal cursor-pointer">
            With API Key
          </Label>
        </div>
      </section>

      {/* Step 1: Framework */}
      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-medium">1. Pick your framework</h3>
        </div>
        <Select value={selectedFramework} onValueChange={setSelectedFramework}>
          <SelectTrigger className="w-full sm:w-80">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectLabel>Providers</SelectLabel>
              {FRAMEWORKS.filter((f) => f.group === "Providers").map((f) => (
                <SelectItem key={f.id} value={f.id}>
                  {f.name}
                </SelectItem>
              ))}
            </SelectGroup>
            <SelectGroup>
              <SelectLabel>Frameworks</SelectLabel>
              {FRAMEWORKS.filter((f) => f.group === "Frameworks").map((f) => (
                <SelectItem key={f.id} value={f.id}>
                  {f.name}
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>
      </section>

      {/* Step 2: Install & Run */}
      <section className="space-y-3 sm:space-y-4">
        <div>
          <h3 className="text-sm font-medium">
            2. Install and run
            <span className="mx-1.5 font-normal text-border">|</span>
            <a
              href={framework.docUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-0.5 text-xs font-normal text-muted-foreground hover:text-foreground transition-colors"
            >
              docs
              <ExternalLink className="h-3 w-3" />
            </a>
          </h3>
          {framework.note && <p className="mt-1 text-xs text-muted-foreground">{framework.note}</p>}
        </div>

        {framework.banner ? (
          <div className="rounded-lg border border-dashed bg-muted/30 p-4">
            <p className="text-sm font-medium">Not supported in TypeScript</p>
            <p className="mt-1.5 text-xs text-muted-foreground">{framework.banner}</p>
          </div>
        ) : framework.altCode ? (
          <>
            {/* Option 1: SideSeat SDK */}
            <div className="space-y-3 rounded-lg border bg-muted/30 p-3 sm:p-4">
              <div>
                <p className="text-xs font-medium">SideSeat SDK (Recommended)</p>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  Automatic setup — one import, zero config.
                </p>
              </div>
              <div className="space-y-1.5">
                <p className="text-xs text-muted-foreground">Install</p>
                <CodeBlock code={framework.install} label="Install command" lang="bash" />
              </div>
              <div className="space-y-1.5">
                <p className="text-xs text-muted-foreground">Code</p>
                <CodeBlock
                  code={transformCode(framework.code(), framework.lang, { useApiKey, projectId })}
                  label="Setup code"
                  lang={framework.lang}
                />
              </div>
            </div>

            {/* Option 2: Without SideSeat SDK */}
            {framework.altInstall && (
              <div className="space-y-3 rounded-lg border bg-muted/30 p-3 sm:p-4">
                <div>
                  <p className="text-xs font-medium">Without SideSeat SDK</p>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    Manual OpenTelemetry setup for full control.
                  </p>
                </div>
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">Set environment variables</p>
                  <CodeBlock
                    code={
                      useApiKey
                        ? `export OTEL_EXPORTER_OTLP_ENDPOINT=${endpoint}\nexport OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $SIDESEAT_API_KEY"`
                        : `export OTEL_EXPORTER_OTLP_ENDPOINT=${endpoint}`
                    }
                    label="Environment variables"
                    lang="bash"
                  />
                </div>
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">Install</p>
                  <CodeBlock code={framework.altInstall} label="Install command" lang="bash" />
                </div>
                {!framework.altSkipProviderSetup && (
                  <div className="space-y-1.5">
                    <p className="text-xs text-muted-foreground">Configure the exporter</p>
                    <CodeBlock
                      code={providerSetup(framework.lang)}
                      label="Provider setup"
                      lang={framework.lang}
                    />
                  </div>
                )}
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">Instrument the framework</p>
                  <CodeBlock
                    code={transformCode(framework.altCode(), framework.lang, {
                      useApiKey,
                      projectId,
                    })}
                    label="Setup code"
                    lang={framework.lang}
                  />
                </div>
              </div>
            )}
          </>
        ) : (
          <>
            <div className="space-y-1.5">
              <p className="text-xs font-medium text-muted-foreground">Install</p>
              <CodeBlock code={framework.install} label="Install command" lang="bash" />
            </div>
            <div className="space-y-1.5">
              <p className="text-xs font-medium text-muted-foreground">Code</p>
              <CodeBlock
                code={transformCode(framework.code(), framework.lang, { useApiKey, projectId })}
                label="Setup code"
                lang={framework.lang}
              />
            </div>
          </>
        )}

        {/* Run */}
        {!framework.banner && (
          <div className="space-y-1.5">
            <p className="text-xs font-medium text-muted-foreground">Run</p>
            <CodeBlock code={framework.run} label="Run command" lang="bash" />
          </div>
        )}
      </section>

      {/* Step 3: See your runs */}
      {!framework.banner && (
        <section className="space-y-3 sm:space-y-4">
          <div>
            <h3 className="text-sm font-medium">3. See your runs</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              SideSeat shows a timeline of prompts, tool calls, and model responses for each agent
              run. Traces appear within seconds.
            </p>
          </div>
          <div className="space-y-2">
            <Link to={`/projects/${projectId}/observability/traces`}>
              <Button variant="outline" size="sm">
                Open workbench
              </Button>
            </Link>
            <div className="mt-2 rounded-lg border border-dashed p-3 text-xs text-muted-foreground">
              <p className="font-medium">Traces not appearing?</p>
              <ul className="mt-1.5 list-inside list-disc space-y-0.5">
                <li>Make sure SideSeat is running</li>
                <li>
                  For short scripts, call <code className="font-mono">shutdown()</code> before exit
                  so spans are flushed
                </li>
                <li>
                  Check the endpoint URL matches <code className="font-mono">{endpoint}</code>
                </li>
              </ul>
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
