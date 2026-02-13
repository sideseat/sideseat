import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import {
  useCreateApiKey,
  API_KEY_SCOPES,
  SCOPE_DESCRIPTIONS,
  EXPIRATION_PRESETS,
  type ApiKeyScope,
  type CreateApiKeyResponse,
} from "@/api/api-keys";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
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
import { Spinner } from "@/components/ui/spinner";

interface CreateApiKeyDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  onCreated: (response: CreateApiKeyResponse) => void;
}

const MAX_NAME_LENGTH = 100;

const createApiKeySchema = z.object({
  name: z
    .string()
    .transform((val) => val.trim())
    .pipe(
      z
        .string()
        .min(1, "Name is required")
        .max(MAX_NAME_LENGTH, `Name must be ${MAX_NAME_LENGTH} characters or less`),
    ),
  scope: z.enum(API_KEY_SCOPES),
  expirationDays: z.number().nullable(),
});

type CreateApiKeyFormInput = z.input<typeof createApiKeySchema>;
type CreateApiKeyFormOutput = z.output<typeof createApiKeySchema>;

export function CreateApiKeyDialog({
  open,
  onOpenChange,
  orgId,
  onCreated,
}: CreateApiKeyDialogProps) {
  const { mutate, isPending } = useCreateApiKey();

  const {
    register,
    handleSubmit,
    reset,
    setValue,
    watch,
    formState: { errors, isValid },
  } = useForm<CreateApiKeyFormInput, unknown, CreateApiKeyFormOutput>({
    resolver: zodResolver(createApiKeySchema),
    mode: "onChange",
    defaultValues: {
      name: "",
      scope: "full",
      expirationDays: null,
    },
  });

  const scopeValue = watch("scope");
  const expirationValue = watch("expirationDays");

  const onSubmit = (data: CreateApiKeyFormOutput) => {
    const expiresAt = data.expirationDays
      ? Math.floor(Date.now() / 1000) + data.expirationDays * 86400
      : undefined;

    mutate(
      {
        orgId,
        data: {
          name: data.name,
          scope: data.scope,
          expires_at: expiresAt,
        },
      },
      {
        onSuccess: (response) => {
          reset();
          onCreated(response);
        },
      },
    );
  };

  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen) {
      reset();
    }
    onOpenChange(newOpen);
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create API Key</DialogTitle>
          <DialogDescription>Create a new API key for programmatic access.</DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)}>
          <div className="space-y-4 py-4">
            {/* Name field */}
            <Field data-invalid={!!errors.name}>
              <FieldLabel>Name</FieldLabel>
              <Input
                {...register("name")}
                placeholder="My API Key"
                maxLength={MAX_NAME_LENGTH + 1}
                aria-invalid={!!errors.name}
                autoFocus
              />
              {errors.name && <FieldError>{errors.name.message}</FieldError>}
            </Field>

            {/* Scope field */}
            <Field>
              <FieldLabel>Permission Scope</FieldLabel>
              <Select
                value={scopeValue}
                onValueChange={(value) => setValue("scope", value as ApiKeyScope)}
              >
                <SelectTrigger>
                  <SelectValue placeholder="Select scope">
                    <span className="capitalize">{scopeValue}</span>
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {API_KEY_SCOPES.map((scope) => (
                    <SelectItem key={scope} value={scope}>
                      <div className="flex flex-col items-start py-1">
                        <span className="font-medium capitalize">{scope}</span>
                        <span className="text-xs text-muted-foreground">
                          {SCOPE_DESCRIPTIONS[scope]}
                        </span>
                      </div>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>

            {/* Expiration field */}
            <Field>
              <FieldLabel>Expiration</FieldLabel>
              <Select
                value={expirationValue === null ? "never" : String(expirationValue)}
                onValueChange={(value) =>
                  setValue("expirationDays", value === "never" ? null : Number(value))
                }
              >
                <SelectTrigger>
                  <SelectValue placeholder="Select expiration" />
                </SelectTrigger>
                <SelectContent>
                  {EXPIRATION_PRESETS.map((preset) => (
                    <SelectItem
                      key={preset.label}
                      value={preset.days === null ? "never" : String(preset.days)}
                    >
                      {preset.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!isValid || isPending}>
              {isPending && <Spinner className="mr-2 h-4 w-4" />}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
