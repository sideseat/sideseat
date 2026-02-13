import { useForm, useWatch } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import { useCreateProject, type Project } from "@/api/projects";
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
import { Spinner } from "@/components/ui/spinner";

interface CreateProjectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated?: (project: Project) => void;
}

const MAX_NAME_LENGTH = 100;

const createProjectSchema = z.object({
  name: z
    .string()
    .transform((val) => val.trim())
    .pipe(
      z
        .string()
        .min(1, "Project name is required")
        .max(MAX_NAME_LENGTH, `Name must be ${MAX_NAME_LENGTH} characters or less`),
    ),
});

type CreateProjectFormInput = z.input<typeof createProjectSchema>;
type CreateProjectFormOutput = z.output<typeof createProjectSchema>;

export function CreateProjectDialog({ open, onOpenChange, onCreated }: CreateProjectDialogProps) {
  const { mutate, isPending } = useCreateProject();

  const {
    register,
    handleSubmit,
    reset,
    control,
    formState: { errors, isValid },
  } = useForm<CreateProjectFormInput, unknown, CreateProjectFormOutput>({
    resolver: zodResolver(createProjectSchema),
    mode: "onChange",
    defaultValues: { name: "" },
  });

  const nameValue = useWatch({ control, name: "name", defaultValue: "" });
  const trimmedLength = nameValue.trim().length;

  const onSubmit = (data: CreateProjectFormOutput) => {
    mutate(
      { name: data.name, organization_id: "default" },
      {
        onSuccess: (project) => {
          reset();
          if (onCreated) {
            onCreated(project);
          } else {
            onOpenChange(false);
          }
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
          <DialogTitle>Create Project</DialogTitle>
          <DialogDescription className="sr-only">
            Enter a name for your new project
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit(onSubmit)}>
          <div className="py-4">
            <Field data-invalid={!!errors.name}>
              <FieldLabel>Project Name</FieldLabel>
              <Input
                {...register("name")}
                placeholder="My Project"
                maxLength={MAX_NAME_LENGTH + 1}
                aria-invalid={!!errors.name}
                autoFocus
              />
              {errors.name && <FieldError>{errors.name.message}</FieldError>}
              <p className="mt-1 text-xs text-muted-foreground">
                {trimmedLength}/{MAX_NAME_LENGTH} characters
              </p>
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
