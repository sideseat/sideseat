import { QueryClient, MutationCache, QueryCache } from "@tanstack/react-query";

// Error notification callback - set this from your app to integrate with toast/notification
type ErrorHandler = (message: string, error: unknown) => void;
let errorHandler: ErrorHandler = (message) => console.error("[Query Error]", message);

export function setQueryErrorHandler(handler: ErrorHandler) {
  errorHandler = handler;
}

// Internal error processing
function handleError(error: unknown, context?: { meta?: { skipGlobalError?: boolean } }) {
  // Allow individual queries/mutations to opt out of global error handling
  if (context?.meta?.skipGlobalError) return;

  const message = error instanceof Error ? error.message : "An error occurred";
  errorHandler(message, error);
}

export const queryClient = new QueryClient({
  queryCache: new QueryCache({
    onError: (error, query) => handleError(error, query),
  }),
  mutationCache: new MutationCache({
    onError: (error, _vars, _context, mutation) => handleError(error, mutation),
  }),
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      gcTime: 5 * 60_000,
      retry: 1,
      refetchOnWindowFocus: false,
      structuralSharing: true,
    },
  },
});
