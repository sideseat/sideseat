import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useApiKeysClient } from "@/lib/app-context";
import { apiKeyKeys } from "../keys";
import type { CreateApiKeyRequest } from "../types";

export function useCreateApiKey() {
  const queryClient = useQueryClient();
  const client = useApiKeysClient();

  return useMutation({
    mutationFn: ({ orgId, data }: { orgId: string; data: CreateApiKeyRequest }) =>
      client.create(orgId, data),
    onSuccess: (_, { orgId }) => {
      queryClient.invalidateQueries({ queryKey: apiKeyKeys.list(orgId) });
      // Don't toast here - ApiKeyCreatedDialog handles success UX
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to create API key");
    },
  });
}

export function useDeleteApiKey() {
  const queryClient = useQueryClient();
  const client = useApiKeysClient();

  return useMutation({
    mutationFn: ({ orgId, keyId }: { orgId: string; keyId: string }) => client.delete(orgId, keyId),
    onSuccess: (_, { orgId }) => {
      queryClient.invalidateQueries({ queryKey: apiKeyKeys.list(orgId) });
      toast.success("API key deleted");
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to delete API key");
    },
  });
}
