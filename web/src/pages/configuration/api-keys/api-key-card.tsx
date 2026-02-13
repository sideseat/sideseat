import type { MouseEvent } from "react";
import { Key, Trash2, Clock, Calendar, AlertTriangle } from "lucide-react";

import type { ApiKey } from "@/api/api-keys";
import { SCOPE_BADGE_VARIANT } from "@/api/api-keys";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

interface ApiKeyCardProps {
  apiKey: ApiKey;
  onDelete: () => void;
}

function formatRelativeTime(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffSecs = Math.floor(diffMs / 1000);
  const diffMins = Math.floor(diffSecs / 60);
  const diffHours = Math.floor(diffMins / 60);
  const diffDays = Math.floor(diffHours / 24);

  if (diffSecs < 60) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 30) return `${diffDays}d ago`;

  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: date.getFullYear() !== now.getFullYear() ? "numeric" : undefined,
  });
}

function formatDate(dateStr: string): string {
  const date = new Date(dateStr);
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function isExpired(expiresAt: string | null): boolean {
  if (!expiresAt) return false;
  return new Date(expiresAt) < new Date();
}

export function ApiKeyCard({ apiKey, onDelete }: ApiKeyCardProps) {
  const handleDelete = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    onDelete();
  };

  const expired = isExpired(apiKey.expires_at);

  return (
    <div
      className={cn(
        "group relative overflow-hidden rounded-xl border bg-card transition-all duration-200",
        expired
          ? "border-destructive/30 bg-destructive/5"
          : "border-border hover:border-primary/50 hover:shadow-md",
      )}
    >
      <div className="p-4">
        {/* Header row */}
        <div className="flex items-start justify-between gap-3">
          <div className="flex items-center gap-3 min-w-0">
            {/* Key icon */}
            <div
              className={cn(
                "flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border",
                expired
                  ? "border-destructive/20 bg-destructive/10"
                  : "border-primary/20 bg-primary/10",
              )}
            >
              <Key className={cn("h-5 w-5", expired ? "text-destructive" : "text-primary")} />
            </div>

            <div className="min-w-0 flex-1">
              <h3 className="font-semibold text-foreground truncate leading-tight">
                {apiKey.name}
              </h3>
              {/* Key prefix with muted styling */}
              <code className="mt-1 inline-block rounded bg-muted px-2 py-0.5 text-xs text-muted-foreground font-mono">
                {apiKey.key_prefix}...
              </code>
            </div>
          </div>

          {/* Actions */}
          <div className="flex items-center gap-2 shrink-0">
            {/* Scope badge */}
            {expired ? (
              <Badge variant="destructive" className="gap-1">
                <AlertTriangle className="h-3 w-3" />
                Expired
              </Badge>
            ) : (
              <Badge variant={SCOPE_BADGE_VARIANT[apiKey.scope]}>{apiKey.scope}</Badge>
            )}

            {/* Delete button */}
            <button
              type="button"
              onClick={handleDelete}
              className={cn(
                "rounded-lg p-2 transition-all duration-150",
                "text-muted-foreground/60 hover:text-destructive",
                "hover:bg-destructive/10 focus-visible:bg-destructive/10",
                "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-destructive/50",
              )}
              aria-label={`Delete API key ${apiKey.name}`}
            >
              <Trash2 className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Metadata footer */}
        <div className="mt-4 flex items-center gap-4 text-xs text-muted-foreground">
          <div className="flex items-center gap-1.5">
            <Clock className="h-3.5 w-3.5" />
            <span>
              {apiKey.last_used_at ? (
                <>Used {formatRelativeTime(apiKey.last_used_at)}</>
              ) : (
                <span className="italic">Never used</span>
              )}
            </span>
          </div>
          <div className="flex items-center gap-1.5">
            <Calendar className="h-3.5 w-3.5" />
            <span>Created {formatDate(apiKey.created_at)}</span>
          </div>
          {apiKey.expires_at && (
            <div className={cn("flex items-center gap-1.5", expired && "text-destructive")}>
              <AlertTriangle className="h-3.5 w-3.5" />
              <span>
                {expired ? "Expired" : "Expires"} {formatDate(apiKey.expires_at)}
              </span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
