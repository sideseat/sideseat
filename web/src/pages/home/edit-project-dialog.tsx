import { useEffect } from "react";
import { useForm, useWatch } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import type { Project } from "@/api/projects";
import { useUpdateProject } from "@/api/projects";
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

interface EditProjectDialogProps {
  project: Project | null;
  onOpenChange: (open: boolean) => void;
}

const MAX_NAME_LENGTH = 100;

const editProjectSchema = z.object({
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

type EditProjectFormInput = z.input<typeof editProjectSchema>;
type EditProjectFormOutput = z.output<typeof editProjectSchema>;

export function EditProjectDialog({ project, onOpenChange }: EditProjectDialogProps) {
  const { mutate, isPending } = useUpdateProject();

  const {
    register,
    handleSubmit,
    reset,
    control,
    formState: { errors, isValid, isDirty },
  } = useForm<EditProjectFormInput, unknown, EditProjectFormOutput>({
    resolver: zodResolver(editProjectSchema),
    mode: "onChange",
    defaultValues: { name: "" },
  });

  useEffect(() => {
    if (project) {
      reset({ name: project.name });
    }
  }, [project, reset]);

  const nameValue = useWatch({ control, name: "name", defaultValue: "" });
  const trimmedLength = nameValue.trim().length;

  const onSubmit = (data: EditProjectFormOutput) => {
    if (!project) return;
    mutate(
      { id: project.id, data: { name: data.name } },
      {
        onSuccess: () => {
          onOpenChange(false);
        },
      },
    );
  };

  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen) {
      reset({ name: project?.name ?? "" });
    }
    onOpenChange(newOpen);
  };

  return (
    <Dialog open={!!project} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit Project</DialogTitle>
          <DialogDescription className="sr-only">Change the project name</DialogDescription>
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
            <Button type="submit" disabled={!isValid || !isDirty || isPending}>
              {isPending && <Spinner className="mr-2 h-4 w-4" />}
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
