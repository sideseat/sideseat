import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useCredentialsClient } from "@/lib/app-context";
import { credentialKeys } from "../keys";
import type {
  CreateCredentialRequest,
  UpdateCredentialRequest,
  CreatePermissionRequest,
} from "../types";

export function useCreateCredential() {
  const queryClient = useQueryClient();
  const client = useCredentialsClient();

  return useMutation({
    mutationFn: ({ orgId, req }: { orgId: string; req: CreateCredentialRequest }) =>
      client.create(orgId, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: credentialKeys.lists() });
      toast.success("Credential added");
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to add credential");
    },
  });
}

export function useUpdateCredential() {
  const queryClient = useQueryClient();
  const client = useCredentialsClient();

  return useMutation({
    mutationFn: ({
      orgId,
      id,
      req,
    }: {
      orgId: string;
      id: string;
      req: UpdateCredentialRequest;
    }) => client.update(orgId, id, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: credentialKeys.lists() });
      toast.success("Credential updated");
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to update credential");
    },
  });
}

export function useDeleteCredential() {
  const queryClient = useQueryClient();
  const client = useCredentialsClient();

  return useMutation({
    mutationFn: ({ orgId, id }: { orgId: string; id: string }) => client.delete(orgId, id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: credentialKeys.lists() });
      toast.success("Credential deleted");
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to delete credential");
    },
  });
}

export function useTestCredential() {
  const client = useCredentialsClient();

  return useMutation({
    mutationFn: ({ orgId, id }: { orgId: string; id: string }) => client.test(orgId, id),
  });
}

export function useCreateCredentialPermission() {
  const queryClient = useQueryClient();
  const client = useCredentialsClient();

  return useMutation({
    mutationFn: ({
      orgId,
      credentialId,
      req,
    }: {
      orgId: string;
      credentialId: string;
      req: CreatePermissionRequest;
    }) => client.createPermission(orgId, credentialId, req),
    onSuccess: (_, { orgId, credentialId }) => {
      queryClient.invalidateQueries({
        queryKey: credentialKeys.permissions(orgId, credentialId),
      });
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to add permission");
    },
  });
}

export function useDeleteCredentialPermission() {
  const queryClient = useQueryClient();
  const client = useCredentialsClient();

  return useMutation({
    mutationFn: ({
      orgId,
      credentialId,
      permId,
    }: {
      orgId: string;
      credentialId: string;
      permId: string;
    }) => client.deletePermission(orgId, credentialId, permId),
    onSuccess: (_, { orgId, credentialId }) => {
      queryClient.invalidateQueries({
        queryKey: credentialKeys.permissions(orgId, credentialId),
      });
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to remove permission");
    },
  });
}
