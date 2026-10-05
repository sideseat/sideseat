import { AlertCircle } from "lucide-react";
import type { SpanEnvelope } from "@/api/otel/types";

/** A failed span's exception, shown in the thread where that span's step happened. */
export function SpanErrorRow({ envelope }: { envelope: SpanEnvelope }) {
  const title = envelope.exception_type ?? "Error";
  return (
    <div
      data-span-error=""
      className="rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2 @[400px]:px-4"
    >
      <div className="flex items-center gap-2">
        <AlertCircle aria-hidden="true" className="h-4 w-4 shrink-0 text-destructive" />
        <span className="text-sm font-medium text-destructive">{title}</span>
        {envelope.span_name && (
          <span className="min-w-0 truncate font-mono text-xs text-muted-foreground">
            in {envelope.span_name}
          </span>
        )}
      </div>
      {envelope.exception_message && (
        <p className="mt-1 whitespace-pre-wrap break-words text-sm">{envelope.exception_message}</p>
      )}
      {envelope.exception_stacktrace && (
        <details className="mt-2">
          <summary className="cursor-pointer text-xs text-muted-foreground">Stack trace</summary>
          <pre className="mt-1 max-h-80 overflow-auto rounded-md bg-muted/50 p-2 font-mono text-xs whitespace-pre">
            {envelope.exception_stacktrace}
          </pre>
        </details>
      )}
    </div>
  );
}
