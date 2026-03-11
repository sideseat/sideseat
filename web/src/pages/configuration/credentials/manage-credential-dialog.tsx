import { useEffect, useState } from "react";
import { Trash2, Plus, CheckCircle, XCircle, Loader2 } from "lucide-react";

import type { Credential, TestResult } from "@/api/credentials";
import {
  useUpdateCredential,
  useCredentialPermissions,
  useCreateCredentialPermission,
  useDeleteCredentialPermission,
} from "@/api/credentials";
import { useProjects } from "@/api/projects";
import { useCredentialsClient } from "@/lib/app-context";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

import { getProvider, type CredentialField, type ProviderEntry } from "./provider-catalog";

function getFieldDef(
  provider: ProviderEntry | undefined,
  key: string,
): CredentialField | undefined {
  if (!provider) return undefined;
  const allFields = [
    ...(provider.fields ?? []),
    ...(provider.authModes?.flatMap((m) => m.fields) ?? []),
  ];
  return allFields.find((f) => f.name === key);
}

interface ManageCredentialDialogProps {
  credential: Credential | null;
  orgId: string;
  onClose: () => void;
}

function ProviderBadge({ providerKey }: { providerKey: string }) {
  const p = getProvider(providerKey);
  if (!p) {
    return (
      <span className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border bg-muted text-xs font-bold text-muted-foreground">
        {providerKey.slice(0, 2).toUpperCase()}
      </span>
    );
  }
  return (
    <span
      className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-sm font-bold text-white"
      style={{ backgroundColor: p.accentColor }}
    >
      {p.abbrev}
    </span>
  );
}

export function ManageCredentialDialog({
  credential,
  orgId,
  onClose,
}: ManageCredentialDialogProps) {
  const credClient = useCredentialsClient();
  const [isTesting, setIsTesting] = useState(false);
  const [testResult, setTestResult] = useState<TestResult | null>(null);
  const [activeTab, setActiveTab] = useState("general");
  const [displayName, setDisplayName] = useState(credential?.display_name ?? "");
  const [endpointUrl, setEndpointUrl] = useState(credential?.endpoint_url ?? "");
  const { mutate: saveMutate, isPending: isSaving } = useUpdateCredential();
  const provider = credential ? getProvider(credential.provider_key) : undefined;

  useEffect(() => {
    if (credential) {
      setDisplayName(credential.display_name);
      setEndpointUrl(credential.endpoint_url ?? "");
      setActiveTab("general");
      setTestResult(null);
      setIsTesting(false);
    }
  }, [credential?.id]);

  const handleSave = () => {
    if (!credential) return;
    saveMutate(
      {
        orgId,
        id: credential.id,
        req: {
          display_name: displayName.trim() || undefined,
          endpoint_url: endpointUrl.trim() ? endpointUrl.trim() : null,
        },
      },
      { onSuccess: onClose },
    );
  };

  const handleTest = async () => {
    if (!credential) return;
    setTestResult(null);
    setIsTesting(true);
    try {
      const r = await credClient.test(orgId, credential.id);
      setTestResult(r);
    } catch (e) {
      setTestResult({
        success: false,
        latency_ms: 0,
        error: e instanceof Error ? e.message : "Test failed",
      });
    } finally {
      setIsTesting(false);
    }
  };

  const handleOpenChange = (open: boolean) => {
    if (!open) {
      setTestResult(null);
      setIsTesting(false);
      onClose();
    }
  };

  return (
    <Dialog open={!!credential} onOpenChange={handleOpenChange}>
      <DialogContent
        className="flex w-full flex-col sm:max-w-3xl"
        style={{ height: "min(90vh, 760px)" }}
      >
        <DialogHeader className="shrink-0">
          <DialogTitle className="flex items-center gap-2">
            {credential && <ProviderBadge providerKey={credential.provider_key} />}
            {credential?.display_name ?? "Manage Credential"}
          </DialogTitle>
          <DialogDescription>
            {provider?.displayName ?? credential?.provider_key}
            {credential?.key_preview && (
              <span className="ml-2 font-mono text-xs opacity-60">
                {credential.key_preview}••••
              </span>
            )}
          </DialogDescription>
        </DialogHeader>

        {credential && (
          <Tabs value={activeTab} onValueChange={setActiveTab} className="flex min-h-0 flex-1 flex-col">
            <TabsList className="w-fit shrink-0">
              <TabsTrigger value="general">General</TabsTrigger>
              <TabsTrigger value="access">Access</TabsTrigger>
            </TabsList>

            <div className="mt-5 flex min-h-0 flex-1 flex-col px-0.5">
              <TabsContent value="general" className="flex flex-1 flex-col data-[state=inactive]:hidden">
                <GeneralTab
                  credential={credential}
                  provider={provider}
                  displayName={displayName}
                  setDisplayName={setDisplayName}
                  endpointUrl={endpointUrl}
                  setEndpointUrl={setEndpointUrl}
                />
              </TabsContent>
              <TabsContent value="access" className="flex flex-1 flex-col data-[state=inactive]:hidden">
                <AccessTab credential={credential} orgId={orgId} />
              </TabsContent>
            </div>
          </Tabs>
        )}

        <div className="mt-4 shrink-0 space-y-3 border-t pt-4">
          {testResult && (
            <div
              className={cn(
                "flex items-start gap-2 rounded-lg border px-3 py-2.5 text-sm",
                testResult.success
                  ? "border-green-500/30 bg-green-500/10 text-green-700 dark:text-green-400"
                  : "border-destructive/30 bg-destructive/10 text-destructive",
              )}
            >
              {testResult.success ? (
                <CheckCircle className="mt-0.5 h-4 w-4 shrink-0" />
              ) : (
                <XCircle className="mt-0.5 h-4 w-4 shrink-0" />
              )}
              <span>
                {testResult.success
                  ? `Connected${testResult.model_hint ? ` — ${testResult.model_hint}` : ""}${testResult.latency_ms ? ` (${testResult.latency_ms}ms)` : ""}`
                  : (testResult.error ?? "Connection failed")}
              </span>
            </div>
          )}

          <div className="flex items-center gap-2">
            <Button type="button" variant="outline" onClick={handleTest} disabled={isTesting}>
              {isTesting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              Test Connection
            </Button>
            <div className="ml-auto flex items-center gap-2">
              {activeTab === "general" && (
                <Button type="button" onClick={handleSave} disabled={isSaving || !displayName.trim()}>
                  {isSaving && <Spinner className="mr-2 h-4 w-4" />}
                  Save
                </Button>
              )}
              <Button type="button" variant="outline" onClick={onClose}>
                Close
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function GeneralTab({
  credential,
  provider,
  displayName,
  setDisplayName,
  endpointUrl,
  setEndpointUrl,
}: {
  credential: Credential;
  provider: ProviderEntry | undefined;
  displayName: string;
  setDisplayName: (v: string) => void;
  endpointUrl: string;
  setEndpointUrl: (v: string) => void;
}) {
  const hasEndpointField =
    provider?.fields?.some((f) => f.inEndpointUrl) ||
    provider?.authModes?.some((m) => m.fields.some((f) => f.inEndpointUrl));

  return (
    <div className="flex flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto space-y-5 px-0.5 pb-2">
        <Field>
          <FieldLabel>Display Name</FieldLabel>
          <Input
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            maxLength={100}
            autoFocus
          />
        </Field>

        {(hasEndpointField || credential.endpoint_url) && (
          <Field>
            <FieldLabel>Endpoint URL</FieldLabel>
            <Input
              type="url"
              value={endpointUrl}
              onChange={(e) => setEndpointUrl(e.target.value)}
              placeholder={
                provider?.fields?.find((f) => f.inEndpointUrl)?.placeholder ?? "https://..."
              }
            />
            <p className="mt-1 text-xs text-muted-foreground">
              Leave blank to use the default endpoint
            </p>
          </Field>
        )}

        {credential.extra_config &&
          Object.entries(credential.extra_config)
            .filter(([key]) => key !== "auth_mode")
            .map(([key, val]) => {
              const fieldDef = getFieldDef(provider, key);
              const label = fieldDef?.label ?? key.replace(/_/g, " ");
              const displayValue =
                fieldDef?.type === "select"
                  ? (fieldDef.options?.find((o) => o.value === String(val))?.label ?? String(val ?? ""))
                  : String(val ?? "");
              return (
                <Field key={key}>
                  <FieldLabel>{label}</FieldLabel>
                  <Input
                    value={displayValue}
                    readOnly
                    className="cursor-default opacity-60 focus-visible:ring-0"
                  />
                </Field>
              );
            })}
      </div>
    </div>
  );
}

function AccessTab({
  credential,
  orgId,
}: {
  credential: Credential;
  orgId: string;
}) {
  const [selectedProject, setSelectedProject] = useState("");
  const [access, setAccess] = useState<"allow" | "deny">("allow");

  const { data: permissions, isLoading: permsLoading } = useCredentialPermissions(
    orgId,
    credential.id,
  );
  const { data: projectsData } = useProjects({ org_id: orgId });
  const { mutate: createPerm, isPending: creating } = useCreateCredentialPermission();
  const { mutate: deletePerm } = useDeleteCredentialPermission();

  const projects = projectsData?.data ?? [];

  const handleAddPermission = () => {
    if (!selectedProject) return;
    if (permissions?.some((p) => p.project_id === selectedProject)) return;
    createPerm(
      {
        orgId,
        credentialId: credential.id,
        req: { project_id: selectedProject, access },
      },
      { onSuccess: () => setSelectedProject("") },
    );
  };

  return (
    <div className="flex flex-1 flex-col">
      <p className="shrink-0 text-sm text-muted-foreground">
        Control which projects can use this credential. By default, all projects have access.
      </p>

      <div className="mt-4 min-h-0 flex-1 overflow-y-auto px-0.5">
        {permsLoading ? (
          <div className="space-y-2">
            {[1, 2].map((i) => (
              <Skeleton key={i} className="h-10 w-full" />
            ))}
          </div>
        ) : (permissions ?? []).length === 0 ? (
          <div className="flex h-full items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground">
            No rules — all projects have access by default
          </div>
        ) : (
          <div className="space-y-1.5">
            {(permissions ?? []).map((perm) => {
              const project = projects.find((p) => p.id === perm.project_id);
              return (
                <div
                  key={perm.id}
                  className="flex items-center justify-between rounded-md border px-3 py-2 text-sm"
                >
                  <span className="font-medium">
                    {perm.project_id ? (project?.name ?? perm.project_id) : "All projects (default)"}
                  </span>
                  <div className="flex items-center gap-2">
                    <span
                      className={cn(
                        "rounded px-1.5 py-0.5 text-xs font-medium",
                        perm.access === "allow"
                          ? "bg-green-500/10 text-green-600"
                          : "bg-destructive/10 text-destructive",
                      )}
                    >
                      {perm.access}
                    </span>
                    <button
                      type="button"
                      onClick={() =>
                        deletePerm({ orgId, credentialId: credential.id, permId: perm.id })
                      }
                      className="rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="mt-4 shrink-0 flex gap-2">
        <div className="flex-1">
          <Select value={selectedProject} onValueChange={setSelectedProject}>
            <SelectTrigger>
              <SelectValue placeholder="Select project" />
            </SelectTrigger>
            <SelectContent>
              {projects.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <Select value={access} onValueChange={(v) => setAccess(v as "allow" | "deny")}>
          <SelectTrigger className="w-24">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="allow">Allow</SelectItem>
            <SelectItem value="deny">Deny</SelectItem>
          </SelectContent>
        </Select>
        <Button
          type="button"
          variant="outline"
          size="icon"
          onClick={handleAddPermission}
          disabled={!selectedProject || creating}
        >
          {creating ? <Spinner className="h-4 w-4" /> : <Plus className="h-4 w-4" />}
        </Button>
      </div>
    </div>
  );
}
