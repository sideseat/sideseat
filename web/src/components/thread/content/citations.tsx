import type { Citation } from "@/api/otel/types";
import { JsonContent } from "./json-content";

/** The sources a text cites, beneath it: each by its title or the place it names. */
export function Citations({ citations }: { citations?: Citation[] }) {
  if (!citations?.length) return null;
  return (
    <ul className="mt-2 space-y-1 border-t border-border/30 pt-2 text-xs text-muted-foreground">
      {citations.map((citation, index) => (
        <li key={index}>
          <CitationEntry citation={citation} />
        </li>
      ))}
    </ul>
  );
}

function CitationEntry({ citation }: { citation: Citation }) {
  if (citation.kind === "unknown") {
    return (
      <div>
        <span>Citation in a shape not yet read</span>
        <JsonContent data={citation.raw} />
      </div>
    );
  }
  const label = citation.title ?? citation.source;
  return (
    <span>
      {citation.kind === "url" && citation.source ? (
        <a href={citation.source} target="_blank" rel="noopener noreferrer" className="underline">
          {label}
        </a>
      ) : (
        <span>{label}</span>
      )}
      {citation.cited_text && <q className="ml-1">{citation.cited_text}</q>}
    </span>
  );
}
