/* Adapted from engagement-mck/solution/site/src/components/chat/tool-call-card.tsx */
import { Check, ChevronRight, Copy, Wrench } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { PulseDot } from "./pulse-dot";

export const RESULT_INLINE_MAX_CHARS = 4096;

/**
 * Render an icon for a tool. Default-only today; per-tool mappings can be
 * added without changing the call sites.
 */
export function ToolIcon({ className }: { toolName?: string; className?: string }) {
  return <Wrench className={className} />;
}

interface Props {
  toolName: string;
  args: string;
  result: string | null;
  done: boolean;
  /** When provided, renders the row as a clickable pill that opens a sheet. */
  onOpenFull?: () => void;
  scrollTargetId?: string;
}

export function ToolCallCard({
  toolName,
  args,
  result,
  done,
  onOpenFull,
  scrollTargetId,
}: Props) {
  const summary = useMemo(() => summaryFor(toolName, args), [toolName, args]);
  const status = !done || result === null ? "streaming" : "done";

  if (onOpenFull) {
    const streaming = status === "streaming";
    return (
      <button
        type="button"
        id={scrollTargetId}
        onClick={onOpenFull}
        title={summary || toolName}
        className={cn(
          "group inline-flex max-w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-left text-xs shadow-xs transition-colors",
          streaming
            ? "border-primary/40 bg-primary/10 hover:border-primary/60 hover:bg-primary/15"
            : "bg-card hover:bg-muted/40",
        )}
      >
        <span
          className={cn(
            "flex size-5 shrink-0 items-center justify-center rounded",
            streaming ? "bg-primary text-primary-foreground" : "bg-primary/10 text-primary",
          )}
        >
          <ToolIcon toolName={toolName} className="size-3.5" />
        </span>
        <span
          className={cn(
            "shrink-0 font-mono text-[12px] font-medium",
            streaming ? "text-foreground" : "text-foreground/85",
          )}
        >
          {toolName}
        </span>
        {summary ? (
          <span
            className={cn(
              "min-w-0 max-w-[22ch] truncate font-mono text-[12px]",
              streaming ? "text-foreground/80" : "text-muted-foreground",
            )}
          >
            {summary}
          </span>
        ) : null}
        {streaming ? (
          <PulseDot />
        ) : (
          <Check className="size-3 shrink-0 text-green-600" />
        )}
      </button>
    );
  }

  const oversize = result !== null && result.length > RESULT_INLINE_MAX_CHARS;

  return (
    <details
      id={scrollTargetId}
      className="group min-w-0 overflow-hidden rounded-lg border bg-card text-xs shadow-xs transition-colors"
    >
      <summary className="flex cursor-pointer select-none items-center gap-2 px-3 py-2">
        <ChevronRight className="size-3 shrink-0 text-muted-foreground transition-transform duration-200 ease-out group-open:rotate-90" />
        <span className="flex size-5 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
          <ToolIcon toolName={toolName} className="size-3.5" />
        </span>
        <span className="shrink-0 font-mono text-[12px] font-medium text-foreground">
          {toolName}
        </span>
        {summary ? (
          <span className="min-w-0 flex-1 truncate font-mono text-muted-foreground">
            {summary}
          </span>
        ) : (
          <span className="flex-1" />
        )}
        <StatusPill status={status} />
      </summary>
      <div className="space-y-3 border-t bg-muted/30 px-3 py-3">
        <ArgsSection args={args} truncated={!done} />
        {result !== null ? (
          oversize ? <OversizeResultHint bytes={result.length} /> : <ResultSection body={result} />
        ) : null}
      </div>
    </details>
  );
}

function OversizeResultHint({ bytes }: { bytes: number }) {
  return (
    <div className="rounded-md border border-dashed bg-background px-3 py-2">
      <p className="text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
        Result
      </p>
      <p className="text-[11px] text-muted-foreground">
        {formatBytes(bytes)} — too large to render inline.
      </p>
    </div>
  );
}

export function ArgsSection({
  args,
  truncated,
  unbounded,
}: {
  args: string;
  truncated: boolean;
  unbounded?: boolean;
}) {
  return (
    <Section
      label="Arguments"
      body={args || "{}"}
      truncated={truncated}
      mode={looksLikeJson(args) ? "json" : "text"}
      unbounded={unbounded}
    />
  );
}

export function ResultSection({
  body,
  unbounded,
}: {
  body: string;
  unbounded?: boolean;
}) {
  const unwrapped = unwrapJsonString(body);
  const mode: "json" | "text" = looksLikeJson(unwrapped) ? "json" : "text";
  return <Section label="Result" body={unwrapped} mode={mode} unbounded={unbounded} />;
}

function StatusPill({ status }: { status: "streaming" | "done" }) {
  if (status === "streaming") {
    return (
      <Badge variant="default" className="gap-1 text-[10px] font-medium">
        <PulseDot />
        Streaming
      </Badge>
    );
  }
  return (
    <Badge
      variant="outline"
      className="gap-1 border-green-600/30 bg-green-600/10 text-[10px] font-medium text-green-700 dark:text-green-400"
    >
      <Check className="size-3" />
      Done
    </Badge>
  );
}

function Section({
  label,
  body,
  truncated,
  mode = "json",
  unbounded,
}: {
  label: string;
  body: string;
  truncated?: boolean;
  mode?: "json" | "text";
  unbounded?: boolean;
}) {
  const pretty = mode === "json" ? tryPretty(body) : unwrapJsonString(body);
  const [copied, setCopied] = useState(false);
  const onCopy = useCallback(() => {
    navigator.clipboard.writeText(pretty).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  }, [pretty]);

  return (
    <div>
      <div className="mb-1.5 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <p className="text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
            {label}
          </p>
          <span className="rounded-sm border px-1 py-px font-mono text-[9px] uppercase text-muted-foreground/80">
            {mode}
          </span>
        </div>
        <button
          type="button"
          onClick={onCopy}
          className="inline-flex cursor-pointer items-center gap-1 rounded px-1 py-0.5 text-[10px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          aria-label={`Copy ${label}`}
        >
          {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre
        className={cn(
          "overflow-auto rounded-md border bg-background p-2.5 font-mono text-[11px] leading-relaxed",
          unbounded ? "max-h-[70vh]" : "max-h-72",
          mode === "json" ? "whitespace-pre-wrap" : "whitespace-pre",
        )}
      >
        {pretty}
        {truncated ? <span className="text-muted-foreground">{"\n…"}</span> : null}
      </pre>
    </div>
  );
}

function summaryFor(_toolName: string, raw: string): string {
  if (!raw) return "";
  let obj: Record<string, unknown>;
  try {
    obj = JSON.parse(raw) as Record<string, unknown>;
  } catch {
    return "";
  }
  for (const k of ["query", "path", "file", "name", "command", "city", "url", "id"]) {
    const v = obj[k];
    if (typeof v === "string" && v) return v;
    if (typeof v === "number") return String(v);
  }
  return "";
}

function looksLikeJson(raw: string): boolean {
  if (!raw) return false;
  const t = raw.trim();
  const f = t[0];
  const l = t[t.length - 1];
  if (!((f === "{" && l === "}") || (f === "[" && l === "]"))) return false;
  try {
    JSON.parse(t);
    return true;
  } catch {
    return false;
  }
}

function tryPretty(raw: string): string {
  if (!raw) return "";
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

function unwrapJsonString(raw: string): string {
  if (!raw) return "";
  if (raw.startsWith('"') && raw.endsWith('"')) {
    try {
      const parsed = JSON.parse(raw);
      if (typeof parsed === "string") return parsed;
    } catch {
      // fall through
    }
  }
  if (/\\[ntr"\\]/.test(raw)) {
    return raw
      .replace(/\\n/g, "\n")
      .replace(/\\r/g, "\r")
      .replace(/\\t/g, "\t")
      .replace(/\\"/g, '"')
      .replace(/\\\\/g, "\\");
  }
  return raw;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
