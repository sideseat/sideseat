import { StrictMode, lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider, createBrowserRouter, Navigate } from "react-router";
import { QueryClientProvider } from "@tanstack/react-query";
import { ReactQueryDevtools } from "@tanstack/react-query-devtools";
import { ThemeProvider } from "@/components/theme-provider";
import { ErrorBoundary } from "@/components/error-boundary";
import { Toaster } from "@/components/ui/sonner";
import { toast } from "sonner";
import { AuthProvider } from "@/auth/context";
import { AuthGuard } from "@/auth/guard";
import { queryClient, setQueryErrorHandler } from "@/api/query-client";
import { AppProvider } from "@/lib/app-context";
import "./styles/index.css";

const ProjectLayout = lazy(() => import("./project-layout"));
const HomePage = lazy(() => import("./pages/home"));
const ProjectHomePage = lazy(() => import("./pages/project-home"));
const AuthPage = lazy(() => import("./pages/auth"));
const NotFoundPage = lazy(() => import("./pages/not-found"));
const TracesPage = lazy(() => import("./pages/observability/traces/traces"));
const TraceDetailPage = lazy(() => import("./pages/observability/trace/trace-detail-page"));
const SessionsPage = lazy(() => import("./pages/observability/sessions/sessions"));
const SessionDetailPage = lazy(() => import("./pages/observability/session/session-detail-page"));
const SpansPage = lazy(() => import("./pages/observability/spans/spans"));
const SpanDetailPage = lazy(() => import("./pages/observability/span/span-detail-page"));
const ConfigurationLayout = lazy(() => import("./pages/configuration/layout"));
const TelemetryPage = lazy(() => import("./pages/configuration/telemetry"));
const McpPage = lazy(() => import("./pages/configuration/mcp"));
const ApiKeysPage = lazy(() => import("./pages/configuration/api-keys"));
const RealtimePage = lazy(() => import("./pages/observability/realtime"));

const HIDE_QUERY_DEVTOOLS = true;

// Set up global error handling for TanStack Query
setQueryErrorHandler((message) => {
  toast.error(message);
});

const router = createBrowserRouter(
  [
    {
      path: "/auth",
      element: <AuthPage />,
    },
    {
      path: "/",
      element: (
        <AuthGuard>
          <AppProvider>
            <HomePage />
          </AppProvider>
        </AuthGuard>
      ),
    },
    // Redirect old configuration URLs to org-scoped URLs
    {
      path: "/configuration",
      element: <Navigate to="/organizations/default/configuration" replace />,
    },
    {
      path: "/configuration/*",
      element: <Navigate to="/organizations/default/configuration" replace />,
    },
    // Org-scoped configuration routes
    {
      path: "/organizations/:orgId/configuration",
      element: (
        <AuthGuard>
          <AppProvider>
            <ConfigurationLayout />
          </AppProvider>
        </AuthGuard>
      ),
      children: [
        { index: true, element: <Navigate to="telemetry" replace /> },
        { path: "telemetry", element: <TelemetryPage /> },
        { path: "mcp", element: <McpPage /> },
        { path: "api-keys", element: <ApiKeysPage /> },
      ],
    },
    {
      path: "/projects/:projectId",
      element: (
        <AuthGuard>
          <ProjectLayout />
        </AuthGuard>
      ),
      children: [
        { index: true, element: <Navigate to="home" replace /> },
        { path: "home", element: <ProjectHomePage /> },
        { path: "observability", element: <Navigate to="traces" replace /> },
        { path: "observability/traces", element: <TracesPage /> },
        { path: "observability/traces/:traceId", element: <TraceDetailPage /> },
        { path: "observability/realtime", element: <RealtimePage /> },
        { path: "observability/spans", element: <SpansPage /> },
        { path: "observability/spans/:traceId/:spanId", element: <SpanDetailPage /> },
        { path: "observability/sessions", element: <SessionsPage /> },
        { path: "observability/sessions/:sessionId", element: <SessionDetailPage /> },
        { path: "*", element: <NotFoundPage /> },
      ],
    },
    {
      path: "*",
      element: <NotFoundPage />,
    },
  ],
  {
    basename: "/ui",
  },
);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <ThemeProvider defaultTheme="system" storageKey="theme">
          <AuthProvider>
            <Suspense
              fallback={
                <div className="flex h-screen w-full items-center justify-center">
                  <div className="flex flex-col items-center gap-4">
                    <div className="h-8 w-48 animate-pulse rounded-md bg-muted" />
                    <div className="h-4 w-32 animate-pulse rounded-md bg-muted" />
                  </div>
                </div>
              }
            >
              <RouterProvider router={router} />
            </Suspense>
            <Toaster />
          </AuthProvider>
        </ThemeProvider>
        {!HIDE_QUERY_DEVTOOLS && import.meta.env.DEV && (
          <ReactQueryDevtools buttonPosition="bottom-left" />
        )}
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);

// Register service worker
if ("serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker
      .register("/ui/sw.js", { scope: "/ui/" })
      .then((registration) => {
        console.log("Service Worker registered:", registration);
      })
      .catch((error) => {
        console.error("Service Worker registration failed:", error);
      });
  });
}
