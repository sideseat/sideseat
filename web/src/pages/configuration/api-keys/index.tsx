import { useMemo, useState } from "react";
import { Navigate, useParams } from "react-router";
import { Key, Plus } from "lucide-react";

import { useApiKeys, type ApiKey, type CreateApiKeyResponse } from "@/api/api-keys";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

import { ApiKeyCard } from "./api-key-card";
import { ApiKeyCreatedDialog } from "./api-key-created-dialog";
import { CreateApiKeyDialog } from "./create-api-key-dialog";
import { DeleteApiKeyDialog } from "./delete-api-key-dialog";

export default function ApiKeysPage() {
  const { orgId } = useParams<{ orgId: string }>();

  // Dialog states
  const [createOpen, setCreateOpen] = useState(false);
  const [createdKey, setCreatedKey] = useState<CreateApiKeyResponse | null>(null);
  const [deleteKey, setDeleteKey] = useState<ApiKey | null>(null);

  // Data fetching
  const { data, isLoading, error, refetch } = useApiKeys(orgId!);

  // Sort keys by name
  const sortedKeys = useMemo(
    () => [...(data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [data],
  );

  // Create mutation callback - opens "key created" dialog
  const handleKeyCreated = (response: CreateApiKeyResponse) => {
    setCreateOpen(false);
    setCreatedKey(response);
  };

  if (!orgId) return <Navigate to="/" replace />;

  return (
    <div className="space-y-6">
      {/* Header with Create button */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">API Keys</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage API keys for programmatic access
          </p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus className="mr-2 h-4 w-4" />
          Create API Key
        </Button>
      </div>

      {/* Loading state */}
      {isLoading && (
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-32 w-full" />
          ))}
        </div>
      )}

      {/* Error state */}
      {error && (
        <div className="text-center py-8">
          <p className="text-destructive">Failed to load API keys</p>
          <Button variant="outline" onClick={() => refetch()} className="mt-4">
            Retry
          </Button>
        </div>
      )}

      {/* Empty state */}
      {!isLoading && !error && sortedKeys.length === 0 && (
        <div className="rounded-2xl border border-dashed border-border/70 bg-muted/40 p-6 text-center">
          <Key className="mx-auto h-10 w-10 text-muted-foreground/50 mb-3" />
          <p className="text-sm text-muted-foreground">
            No API keys yet. Create one to authenticate API requests.
          </p>
          <Button onClick={() => setCreateOpen(true)} className="mt-4">
            Create API Key
          </Button>
        </div>
      )}

      {/* Key list */}
      {!isLoading && !error && sortedKeys.length > 0 && (
        <div className="grid gap-4 md:grid-cols-2">
          {sortedKeys.map((key) => (
            <ApiKeyCard key={key.id} apiKey={key} onDelete={() => setDeleteKey(key)} />
          ))}
        </div>
      )}

      {/* Dialogs */}
      <CreateApiKeyDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        orgId={orgId}
        onCreated={handleKeyCreated}
      />
      <ApiKeyCreatedDialog createdKey={createdKey} onClose={() => setCreatedKey(null)} />
      <DeleteApiKeyDialog
        apiKey={deleteKey}
        orgId={orgId}
        onOpenChange={(open) => !open && setDeleteKey(null)}
      />
    </div>
  );
}
