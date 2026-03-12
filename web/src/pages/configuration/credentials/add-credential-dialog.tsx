import { useState } from "react";
import { Eye, EyeOff, CheckCircle, XCircle, Loader2, Trash2, Plus } from "lucide-react";

import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useCreateCredential, credentialKeys, type TestResult } from "@/api/credentials";
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
import { Field, FieldLabel, FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Spinner } from "@/components/ui/spinner";
import { Wizard, type WizardStep } from "@/components/wizard";
import { cn } from "@/lib/utils";

import {
  PROVIDER_CATALOG,
  buildCreatePayload,
  getProvider,
  type CredentialField,
  type ProviderEntry,
} from "./provider-catalog";

function getFieldDefaults(fields: CredentialField[]): Record<string, string> {
  const defaults: Record<string, string> = {};
  for (const f of fields) {
    if (f.defaultValue) defaults[f.name] = f.defaultValue;
  }
  return defaults;
}

interface AddCredentialDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
}

type PendingPerm = { project_id: string; access: "allow" | "deny" };

const WIZARD_STEPS: WizardStep[] = [
  { id: "provider", label: "Provider" },
  { id: "configure", label: "Configure" },
  { id: "access", label: "Access" },
];

// ─── Field renderers ─────────────────────────────────────────────────────────

function PasswordInput({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <Input
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="pr-10"
      />
      <button
        type="button"
        onClick={() => setShow((s) => !s)}
        className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
        tabIndex={-1}
      >
        {show ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
      </button>
    </div>
  );
}

function DynamicField({
  field,
  value,
  onChange,
  error,
}: {
  field: CredentialField;
  value: string;
  onChange: (v: string) => void;
  error?: string;
}) {
  return (
    <Field data-invalid={!!error}>
      <FieldLabel>
        {field.label}
        {field.required && <span className="ml-0.5 text-destructive">*</span>}
      </FieldLabel>
      {field.description && (
        <p className="mb-1 text-xs text-muted-foreground">{field.description}</p>
      )}
      {field.type === "api_key" ? (
        <PasswordInput value={value} onChange={onChange} placeholder={field.placeholder} />
      ) : field.type === "select" && field.options ? (
        <Select value={value} onValueChange={onChange}>
          <SelectTrigger>
            <SelectValue placeholder={`Select ${field.label.toLowerCase()}`} />
          </SelectTrigger>
          <SelectContent>
            {field.options.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : field.type === "json_secret" ? (
        <textarea
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={field.placeholder}
          rows={4}
          className="flex w-full resize-none rounded-md border border-input bg-background px-3 py-2 font-mono text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        />
      ) : (
        <Input
          type={field.type === "url" ? "url" : "text"}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={field.placeholder}
        />
      )}
      {error && <FieldError>{error}</FieldError>}
    </Field>
  );
}

// ─── Step 1: Provider picker ──────────────────────────────────────────────────

function ProviderPicker({
  selectedKey,
  onChange,
}: {
  selectedKey: string;
  onChange: (key: string) => void;
}) {
  return (
    <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
      {PROVIDER_CATALOG.map((p) => (
        <button
          key={p.key}
          type="button"
          onClick={() => onChange(selectedKey === p.key ? "" : p.key)}
          className={cn(
            "flex items-center gap-3 rounded-xl border bg-card p-4 text-left transition-all duration-150",
            selectedKey === p.key
              ? "border-primary bg-primary/5 shadow-sm ring-1 ring-primary/30"
              : "border-border hover:border-primary/40 hover:shadow-sm",
          )}
        >
          <div
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl text-sm font-bold text-white shadow-sm"
            style={{ backgroundColor: p.accentColor }}
          >
            {p.abbrev}
          </div>
          <div className="min-w-0 flex-1">
            <p className="truncate font-semibold text-foreground">{p.displayName}</p>
            <p className="mt-0.5 truncate text-xs text-muted-foreground">{p.description}</p>
          </div>
        </button>
      ))}
    </div>
  );
}

// ─── Step 2: Configure ────────────────────────────────────────────────────────

function ConfigureStep({
  provider,
  authModeId,
  onAuthModeChange,
  values,
  setValue,
  errors,
  displayName,
  setDisplayName,
  nameError,
  setNameError,
}: {
  provider: ProviderEntry;
  authModeId: string | null;
  onAuthModeChange: (id: string) => void;
  values: Record<string, string>;
  setValue: (name: string, val: string) => void;
  errors: Record<string, string>;
  displayName: string;
  setDisplayName: (v: string) => void;
  nameError: string;
  setNameError: (v: string) => void;
}) {
  const currentFields =
    authModeId && provider.authModes
      ? (provider.authModes.find((m) => m.id === authModeId)?.fields ?? [])
      : (provider.fields ?? []);

  return (
    <div className="space-y-5">
      {/* Name */}
      <Field data-invalid={!!nameError}>
        <FieldLabel>
          Name<span className="ml-0.5 text-destructive">*</span>
        </FieldLabel>
        <Input
          value={displayName}
          onChange={(e) => {
            setDisplayName(e.target.value);
            if (nameError) setNameError("");
          }}
          placeholder={provider.displayName}
          maxLength={100}
          autoFocus
        />
        {nameError && <FieldError>{nameError}</FieldError>}
      </Field>

      {/* Auth mode selector */}
      {provider.authModes && (
        <Field>
          <FieldLabel>Authentication</FieldLabel>
          <ToggleGroup
            type="single"
            value={authModeId ?? ""}
            onValueChange={(v) => v && onAuthModeChange(v)}
            className="flex flex-wrap justify-start gap-1"
          >
            {provider.authModes.map((mode) => (
              <ToggleGroupItem key={mode.id} value={mode.id} className="px-3 py-1.5 text-sm">
                {mode.label}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </Field>
      )}

      {currentFields.map((field) => (
        <DynamicField
          key={field.name}
          field={field}
          value={values[field.name] ?? ""}
          onChange={(v) => setValue(field.name, v)}
          error={errors[field.name]}
        />
      ))}
    </div>
  );
}

// ─── Step 3: Access ───────────────────────────────────────────────────────────

function AccessStep({
  pendingPerms,
  setPendingPerms,
  projects,
}: {
  pendingPerms: PendingPerm[];
  setPendingPerms: React.Dispatch<React.SetStateAction<PendingPerm[]>>;
  projects: { id: string; name: string }[];
}) {
  const [addProject, setAddProject] = useState("");
  const [addAccess, setAddAccess] = useState<"allow" | "deny">("allow");

  return (
    <div className="flex flex-1 flex-col">
      <p className="shrink-0 text-sm text-muted-foreground">
        Restrict which projects can use this credential. By default all projects have access.
      </p>

      <div className="mt-4 min-h-0 flex-1 overflow-y-auto px-1 pt-1">
        {pendingPerms.length === 0 ? (
          <div className="flex h-full items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground">
            No rules — all projects will have access by default
          </div>
        ) : (
          <div className="space-y-1.5">
            {pendingPerms.map((perm) => {
              const project = projects.find((p) => p.id === perm.project_id);
              return (
                <div
                  key={perm.project_id}
                  className="flex items-center justify-between rounded-md border px-3 py-2 text-sm"
                >
                  <span className="font-medium">{project?.name ?? perm.project_id}</span>
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
                      onClick={() => setPendingPerms((prev) => prev.filter((p) => p.project_id !== perm.project_id))}
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
          <Select value={addProject} onValueChange={setAddProject}>
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
        <Select value={addAccess} onValueChange={(v) => setAddAccess(v as "allow" | "deny")}>
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
          onClick={() => {
            if (!addProject) return;
            if (pendingPerms.some((p) => p.project_id === addProject)) return;
            setPendingPerms((prev) => [...prev, { project_id: addProject, access: addAccess }]);
            setAddProject("");
          }}
          disabled={!addProject}
        >
          <Plus className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}

// ─── Dialog ───────────────────────────────────────────────────────────────────

export function AddCredentialDialog({ open, onOpenChange, orgId }: AddCredentialDialogProps) {
  const { mutate: createMutate, isPending: isCreating } = useCreateCredential();
  const credClient = useCredentialsClient();
  const queryClient = useQueryClient();
  const { data: projectsData } = useProjects({ org_id: orgId });
  const projects = projectsData?.data ?? [];

  const [step, setStep] = useState(0);
  const [selectedKey, setSelectedKey] = useState("");
  const [authModeId, setAuthModeId] = useState<string | null>(null);
  const [displayName, setDisplayName] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [nameError, setNameError] = useState("");
  const [pendingPerms, setPendingPerms] = useState<PendingPerm[]>([]);
  const [testedCredId, setTestedCredId] = useState<string | null>(null);
  // Track how many pendingPerms were applied when the test credential was created,
  // so we can apply any rules added afterward on final submit.
  const [testedPermCount, setTestedPermCount] = useState(0);
  const [isTesting, setIsTesting] = useState(false);
  const [testResult, setTestResult] = useState<TestResult | null>(null);

  const provider: ProviderEntry | undefined = selectedKey ? getProvider(selectedKey) : undefined;

  const currentFields = provider
    ? authModeId && provider.authModes
      ? provider.authModes.find((m) => m.id === authModeId)?.fields ?? []
      : (provider.fields ?? [])
    : [];

  // Deletes the server-side test credential and clears all test state.
  // Must be called whenever any field that affects the credential payload changes,
  // so the next Test always creates a fresh, accurate credential.
  const clearTestedCredential = () => {
    if (testedCredId) {
      credClient.delete(orgId, testedCredId).catch(() => undefined);
      setTestedCredId(null);
      setTestedPermCount(0);
      setTestResult(null);
    }
  };

  const handleProviderChange = (key: string) => {
    clearTestedCredential();
    setSelectedKey(key);
    setErrors({});
    if (!key) return;
    const p = getProvider(key);
    const firstMode = p?.authModes?.[0];
    setAuthModeId(firstMode?.id ?? null);
    const fields = firstMode?.fields ?? p?.fields ?? [];
    setValues(getFieldDefaults(fields));
    if (!displayName.trim() && p) setDisplayName(p.displayName);
    // Auto-advance to configure step
    setStep(1);
  };

  const handleAuthModeChange = (modeId: string) => {
    clearTestedCredential();
    const mode = provider?.authModes?.find((m) => m.id === modeId);
    setAuthModeId(modeId);
    setValues(getFieldDefaults(mode?.fields ?? []));
    setErrors({});
  };

  const setFieldValue = (name: string, val: string) => {
    clearTestedCredential();
    setValues((prev) => ({ ...prev, [name]: val }));
    if (errors[name]) setErrors((prev) => ({ ...prev, [name]: "" }));
  };

  // Wrapped setter: changing the display name also invalidates any in-flight test credential
  // since display_name is persisted on the credential object.
  const handleSetDisplayName = (v: string) => {
    clearTestedCredential();
    setDisplayName(v);
  };

  const validate = (): boolean => {
    const newErrors: Record<string, string> = {};
    let hasNameError = false;
    if (!displayName.trim()) {
      setNameError("Name is required");
      hasNameError = true;
    } else if (displayName.trim().length > 100) {
      setNameError("Name must be 100 characters or less");
      hasNameError = true;
    } else {
      setNameError("");
    }
    for (const field of currentFields) {
      if (field.required && !values[field.name]?.trim()) {
        newErrors[field.name] = `${field.label} is required`;
      }
    }
    setErrors(newErrors);
    return !hasNameError && Object.keys(newErrors).length === 0;
  };

  const buildReq = () => {
    if (!provider) return null;
    const payload = buildCreatePayload(provider, authModeId, values);
    return { display_name: displayName.trim(), provider_key: provider.key, ...payload };
  };

  const applyPendingPerms = async (credId: string) => {
    let failures = 0;
    for (const perm of pendingPerms) {
      try {
        await credClient.createPermission(orgId, credId, {
          project_id: perm.project_id,
          access: perm.access,
        });
      } catch {
        failures++;
      }
    }
    if (failures > 0) {
      toast.warning(
        `Credential saved, but ${failures} permission rule${failures > 1 ? "s" : ""} could not be applied`,
      );
    }
  };

  const ensureTestedCredential = async (): Promise<string> => {
    if (testedCredId) return testedCredId;
    const req = buildReq();
    if (!req) throw new Error("Invalid form");
    const cred = await credClient.create(orgId, req);
    setTestedCredId(cred.id);
    setTestedPermCount(pendingPerms.length);
    await applyPendingPerms(cred.id);
    return cred.id;
  };

  const handleTest = async () => {
    if (!provider || !validate()) {
      setStep(1);
      return;
    }
    setTestResult(null);
    setIsTesting(true);
    try {
      const id = await ensureTestedCredential();
      const result = await credClient.test(orgId, id);
      setTestResult(result);
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

  const handleNext = () => {
    if (step === 1 && !validate()) return;
    setStep((s) => s + 1);
  };

  const handleBack = () => {
    setErrors({});
    setNameError("");
    setStep((s) => s - 1);
  };

  const handleSubmit = async () => {
    if (!provider) return;
    if (!validate()) {
      setStep(1);
      return;
    }
    if (testedCredId) {
      // Credential already exists — sync any changes made since it was created.
      const patches: Promise<unknown>[] = [];
      // displayName could have changed (clearTestedCredential would have cleared testedCredId
      // if the user changed it, so this branch is only reached when displayName is unchanged
      // or the user changed it before testing — either way nothing extra needed here).
      // Apply permissions added after the test credential was created.
      const newPerms = pendingPerms.slice(testedPermCount);
      for (const perm of newPerms) {
        patches.push(
          credClient
            .createPermission(orgId, testedCredId, { project_id: perm.project_id, access: perm.access })
            .catch(() => undefined),
        );
      }
      if (patches.length > 0) await Promise.all(patches);
      handleClose(false);
      return;
    }
    const req = buildReq();
    if (!req) return;
    createMutate(
      { orgId, req },
      {
        onSuccess: async (cred) => {
          await applyPendingPerms(cred.id);
          handleClose(false);
        },
      },
    );
  };

  const handleClose = (deleteTestCred = true) => {
    if (deleteTestCred && testedCredId) {
      credClient.delete(orgId, testedCredId).catch(() => undefined);
    } else if (!deleteTestCred && testedCredId) {
      // Credential was created during Test — invalidate list and notify since createMutate was never called
      queryClient.invalidateQueries({ queryKey: credentialKeys.lists() });
      toast.success("Credential added");
    }
    onOpenChange(false);
    // Delay state reset until after the close animation completes (~150ms)
    // to prevent the provider picker from flashing before the dialog fades out
    setTimeout(() => {
      setStep(0);
      setSelectedKey("");
      setAuthModeId(null);
      setDisplayName("");
      setValues({});
      setErrors({});
      setNameError("");
      setPendingPerms([]);
      setTestedCredId(null);
      setTestedPermCount(0);
      setIsTesting(false);
      setTestResult(null);
    }, 200);
  };

  const stepDescriptions = [
    "Select the LLM provider you want to connect",
    provider
      ? `Configure credentials for ${provider.displayName}`
      : "Configure your API credentials",
    "Control which projects can use this credential",
  ];

  const testButton =
    step >= 1 ? (
      <Button
        type="button"
        variant="outline"
        onClick={handleTest}
        disabled={!provider || !displayName.trim() || isTesting || isCreating}
      >
        {isTesting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
        Test
      </Button>
    ) : undefined;

  const testResultBanner = testResult ? (
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
  ) : undefined;

  return (
    <Dialog open={open} onOpenChange={(o) => !o && handleClose()}>
      <DialogContent className="flex w-full flex-col sm:max-w-3xl" style={{ height: "min(90vh, 760px)" }}>
        <DialogHeader className="shrink-0">
          <DialogTitle>Add Model Provider</DialogTitle>
          <DialogDescription>{stepDescriptions[step]}</DialogDescription>
        </DialogHeader>

        <Wizard
          steps={WIZARD_STEPS}
          currentStep={step}
          onNext={step === WIZARD_STEPS.length - 1 ? handleSubmit : handleNext}
          onBack={step > 0 ? handleBack : undefined}
          onCancel={() => handleClose()}
          canNext={
            step === 0
              ? !!selectedKey
              : step === 1
                ? !!provider && !!displayName.trim()
                : true
          }
          nextLabel={
            step === WIZARD_STEPS.length - 1
              ? testedCredId
                ? "Save"
                : "Add Provider"
              : "Next"
          }
          nextLoading={isCreating && !testedCredId}
          extra={testButton}
          aboveFooter={testResultBanner}
        >
          {/* Step 0 — Provider */}
          {step === 0 && (
            <ProviderPicker selectedKey={selectedKey} onChange={handleProviderChange} />
          )}

          {/* Step 1 — Configure */}
          {step === 1 && provider && (
            <ConfigureStep
              provider={provider}
              authModeId={authModeId}
              onAuthModeChange={handleAuthModeChange}
              values={values}
              setValue={setFieldValue}
              errors={errors}
              displayName={displayName}
              setDisplayName={handleSetDisplayName}
              nameError={nameError}
              setNameError={setNameError}
            />
          )}

          {/* Step 2 — Access */}
          {step === 2 && (
            <AccessStep
              pendingPerms={pendingPerms}
              setPendingPerms={setPendingPerms}
              projects={projects}
            />
          )}
        </Wizard>
      </DialogContent>
    </Dialog>
  );
}
