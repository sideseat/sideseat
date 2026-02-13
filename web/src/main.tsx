import { StrictMode } from "react";
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
import ProjectLayout from "./project-layout";
import HomePage from "./pages/home";
import ProjectHomePage from "./pages/project-home";
import AuthPage from "./pages/auth";
import NotFoundPage from "./pages/not-found";
import TracesPage from "./pages/observability/traces/traces";
import TraceDetailPage from "./pages/observability/trace/trace-detail-page";
import SessionsPage from "./pages/observability/sessions/sessions";
import SessionDetailPage from "./pages/observability/session/session-detail-page";
import SpansPage from "./pages/observability/spans/spans";
import SpanDetailPage from "./pages/observability/span/span-detail-page";
import ConfigurationLayout from "./pages/configuration/layout";
import TelemetryPage from "./pages/configuration/telemetry";
import McpPage from "./pages/configuration/mcp";
import ApiKeysPage from "./pages/configuration/api-keys";
import RealtimePage from "./pages/observability/realtime";
import "./styles/index.css";

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
            <RouterProvider router={router} />
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
