import { useState } from "react";
import { Navigate, useParams } from "react-router";
import { CheckCircle2, Plug, Plus, Settings, Trash2, XCircle } from "lucide-react";

import { type Credential, useCredentials, useTestCredential } from "@/api/credentials";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { AddCredentialDialog } from "./add-credential-dialog";
import { DeleteCredentialDialog } from "./delete-credential-dialog";
import { ManageCredentialDialog } from "./manage-credential-dialog";
import { getProvider } from "./provider-catalog";

const sortByName = (a: Credential, b: Credential) => a.display_name.localeCompare(b.display_name);

type TestState = "idle" | "loading" | "success" | "error";

interface TestInfo {
  state: TestState;
  error?: string;
  latencyMs?: number;
  modelHint?: string;
}

function ProviderBadge({ providerKey }: { providerKey: string }) {
  const provider = getProvider(providerKey);
  if (!provider) {
    return (
      <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border bg-muted text-xs font-bold text-muted-foreground">
        {providerKey.slice(0, 2).toUpperCase()}
      </div>
    );
  }
  return (
    <div
      className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-xs font-bold text-white"
      style={{ backgroundColor: provider.accentColor }}
    >
      {provider.abbrev}
    </div>
  );
}

function TestButton({
  testInfo,
  onTest,
}: {
  testInfo: TestInfo;
  onTest: () => void;
}) {
  if (testInfo.state === "loading") {
    return (
      <Button variant="outline" size="sm" disabled>
        <Spinner className="mr-1.5 h-3.5 w-3.5" />
        Testing...
      </Button>
    );
  }
  if (testInfo.state === "success") {
    return (
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="outline" size="sm" className="text-green-600 border-green-600/50" onClick={onTest}>
              <CheckCircle2 className="mr-1.5 h-3.5 w-3.5" />
              Connected
              {testInfo.latencyMs !== undefined && (
                <span className="ml-1 text-xs text-muted-foreground">({testInfo.latencyMs}ms)</span>
              )}
            </Button>
          </TooltipTrigger>
          {testInfo.modelHint && (
            <TooltipContent>
              <p className="text-xs">Model: {testInfo.modelHint}</p>
            </TooltipContent>
          )}
        </Tooltip>
      </TooltipProvider>
    );
  }
  if (testInfo.state === "error") {
    return (
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              className="text-destructive border-destructive/50"
              onClick={onTest}
            >
              <XCircle className="mr-1.5 h-3.5 w-3.5" />
              Failed
            </Button>
          </TooltipTrigger>
          {testInfo.error && (
            <TooltipContent className="max-w-64">
              <p className="text-xs">{testInfo.error}</p>
            </TooltipContent>
          )}
        </Tooltip>
      </TooltipProvider>
    );
  }
  return (
    <Button variant="outline" size="sm" onClick={onTest}>
      Test
    </Button>
  );
}

interface CredentialCardProps {
  credential: Credential;
  orgId: string;
  onDelete: () => void;
  onManage: () => void;
}

function CredentialCard({ credential, orgId, onDelete, onManage }: CredentialCardProps) {
  const [testInfo, setTestInfo] = useState<TestInfo>({ state: "idle" });
  const { mutate: testCred } = useTestCredential();
  const provider = getProvider(credential.provider_key);

  const handleTest = () => {
    setTestInfo({ state: "loading" });
    testCred(
      { orgId, id: credential.id },
      {
        onSuccess: (result) => {
          setTestInfo({
            state: result.success ? "success" : "error",
            error: result.error,
            latencyMs: result.latency_ms,
            modelHint: result.model_hint,
          });
        },
        onError: (e) => {
          setTestInfo({
            state: "error",
            error: e instanceof Error ? e.message : "Test failed",
          });
        },
      },
    );
  };

  const envVarName =
    credential.source === "env" && credential.env_var_name ? credential.env_var_name : null;

  return (
    <div
      className={cn(
        "group relative overflow-hidden rounded-xl border bg-card transition-all duration-200",
        "border-border hover:border-primary/50 hover:shadow-md",
      )}
    >
      <div className="p-4">
        {/* Header row */}
        <div className="flex items-start gap-3">
          <ProviderBadge providerKey={credential.provider_key} />

          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 flex-wrap">
              <h3 className="font-semibold text-foreground leading-tight truncate">
                {credential.display_name}
              </h3>
              {envVarName && (
                <code className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-mono text-muted-foreground">
                  {envVarName}
                </code>
              )}
            </div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {provider?.displayName ?? credential.provider_key}
            </p>
            {credential.key_preview && (
              <code className="mt-1 inline-block rounded bg-muted px-2 py-0.5 text-xs text-muted-foreground font-mono">
                {credential.key_preview}••••••••
              </code>
            )}
            {credential.endpoint_url && (
              <p className="mt-1 text-xs text-muted-foreground truncate">
                {credential.endpoint_url}
              </p>
            )}
          </div>

          {/* Status badges */}
          <div className="flex shrink-0 flex-col items-end gap-1">
            {credential.source === "env" && (
              <Badge variant="secondary" className="text-[10px] px-1.5">
                ENV
              </Badge>
            )}
          </div>
        </div>

        {/* Actions */}
        <div className="mt-4 flex items-center gap-2">
          <TestButton testInfo={testInfo} onTest={handleTest} />
          {!credential.read_only && (
            <>
              <Button variant="ghost" size="sm" onClick={onManage}>
                <Settings className="mr-1.5 h-3.5 w-3.5" />
                Manage
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto text-muted-foreground hover:text-destructive hover:bg-destructive/10"
                onClick={onDelete}
                aria-label={`Delete credential ${credential.display_name}`}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export default function CredentialsPage() {
  const { orgId } = useParams<{ orgId: string }>();

  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Credential | null>(null);
  const [manageTarget, setManageTarget] = useState<Credential | null>(null);

  const { data, isLoading, error, refetch } = useCredentials(orgId!);

  if (!orgId) return <Navigate to="/" replace />;

  const credentials = data ?? [];
  const envCreds = credentials.filter((c) => c.source === "env").sort(sortByName);
  const storedCreds = credentials.filter((c) => c.source !== "env").sort(sortByName);

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">Model Providers</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage API credentials for LLM/AI providers
          </p>
        </div>
        <Button onClick={() => setAddOpen(true)}>
          <Plus className="mr-2 h-4 w-4" />
          Add Provider
        </Button>
      </div>

      {/* Loading */}
      {isLoading && (
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-36 w-full" />
          ))}
        </div>
      )}

      {/* Error */}
      {error && (
        <div className="text-center py-8">
          <p className="text-destructive">Failed to load credentials</p>
          <Button variant="outline" onClick={() => refetch()} className="mt-4">
            Retry
          </Button>
        </div>
      )}

      {/* Empty state */}
      {!isLoading && !error && credentials.length === 0 && (
        <div className="rounded-2xl border border-dashed border-border/70 bg-muted/40 p-6 text-center">
          <Plug className="mx-auto h-10 w-10 text-muted-foreground/50 mb-3" />
          <p className="font-medium text-sm">No providers configured</p>
          <p className="mt-1 text-sm text-muted-foreground">
            Add your first LLM provider credential to get started.
          </p>
          <Button onClick={() => setAddOpen(true)} className="mt-4">
            Add Provider
          </Button>
        </div>
      )}

      {/* Env-detected credentials */}
      {!isLoading && !error && envCreds.length > 0 && (
        <div className="space-y-3">
          <h3 className="text-sm font-medium text-muted-foreground">
            Detected from environment
          </h3>
          <div className="grid gap-4 md:grid-cols-2">
            {envCreds.map((cred) => (
              <CredentialCard
                key={cred.id}
                credential={cred}
                orgId={orgId}
                onDelete={() => setDeleteTarget(cred)}
                onManage={() => setManageTarget(cred)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Stored credentials */}
      {!isLoading && !error && storedCreds.length > 0 && (
        <div className="space-y-3">
          {envCreds.length > 0 && (
            <h3 className="text-sm font-medium text-muted-foreground">
              Configured
            </h3>
          )}
          <div className="grid gap-4 md:grid-cols-2">
            {storedCreds.map((cred) => (
              <CredentialCard
                key={cred.id}
                credential={cred}
                orgId={orgId}
                onDelete={() => setDeleteTarget(cred)}
                onManage={() => setManageTarget(cred)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Dialogs */}
      <AddCredentialDialog open={addOpen} onOpenChange={setAddOpen} orgId={orgId} />
      <DeleteCredentialDialog
        credential={deleteTarget}
        orgId={orgId}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      />
      <ManageCredentialDialog
        credential={manageTarget}
        orgId={orgId}
        onClose={() => setManageTarget(null)}
      />
    </div>
  );
}
