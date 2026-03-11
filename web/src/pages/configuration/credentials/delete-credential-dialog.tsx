import type { Credential } from "@/api/credentials";
import { useDeleteCredential } from "@/api/credentials";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Spinner } from "@/components/ui/spinner";

interface DeleteCredentialDialogProps {
  credential: Credential | null;
  orgId: string;
  onOpenChange: (open: boolean) => void;
}

export function DeleteCredentialDialog({
  credential,
  orgId,
  onOpenChange,
}: DeleteCredentialDialogProps) {
  const { mutate, isPending } = useDeleteCredential();

  const handleDelete = () => {
    if (!credential) return;
    mutate(
      { orgId, id: credential.id },
      { onSuccess: () => onOpenChange(false) },
    );
  };

  return (
    <AlertDialog open={!!credential} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete Credential</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div>
              <span>Are you sure you want to delete </span>
              <span className="font-semibold break-all">"{credential?.display_name}"</span>
              <span>? This will remove the stored secret and cannot be undone.</span>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={handleDelete} disabled={isPending}>
            {isPending && <Spinner className="mr-2 h-4 w-4" />}
            Delete
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
