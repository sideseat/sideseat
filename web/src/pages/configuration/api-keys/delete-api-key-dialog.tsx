import type { ApiKey } from "@/api/api-keys";
import { useDeleteApiKey } from "@/api/api-keys";
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

interface DeleteApiKeyDialogProps {
  apiKey: ApiKey | null;
  orgId: string;
  onOpenChange: (open: boolean) => void;
}

export function DeleteApiKeyDialog({ apiKey, orgId, onOpenChange }: DeleteApiKeyDialogProps) {
  const { mutate, isPending } = useDeleteApiKey();

  const handleDelete = () => {
    if (!apiKey) return;
    mutate(
      { orgId, keyId: apiKey.id },
      {
        onSuccess: () => onOpenChange(false),
      },
    );
  };

  return (
    <AlertDialog open={!!apiKey} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete API Key</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div>
              <span>Are you sure you want to delete </span>
              <span className="font-semibold break-all">"{apiKey?.name}"</span>
              <span>? Any applications using this key will lose access immediately.</span>
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
