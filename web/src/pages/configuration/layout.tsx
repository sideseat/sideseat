import { Link, Navigate, Outlet, useLocation, useParams } from "react-router";
import { QueryParamProvider } from "use-query-params";
import { ReactRouter6Adapter } from "use-query-params/adapters/react-router-6";
import { Key, TerminalSquare, Workflow, type LucideIcon } from "lucide-react";

import { PageHeader } from "@/components/page-header";
import { isPathActive } from "@/lib/navigation";
import { cn } from "@/lib/utils";

type ConfigSection = {
  id: string;
  url: string;
  label: string;
  icon: LucideIcon;
  exactMatch?: boolean;
};

const configSections: ConfigSection[] = [
  { id: "telemetry", url: "telemetry", label: "Telemetry", icon: TerminalSquare },
  { id: "mcp", url: "mcp", label: "MCP Server", icon: Workflow },
  { id: "api-keys", url: "api-keys", label: "API Keys", icon: Key },
];

export default function ConfigurationLayout() {
  const { pathname } = useLocation();
  const { orgId } = useParams<{ orgId: string }>();

  // Redirect if no orgId (shouldn't happen with proper routing)
  if (!orgId) {
    return <Navigate to="/" replace />;
  }

  // Extract the path relative to /organizations/:orgId/configuration/
  // E.g., "/organizations/default/configuration/connection-strings" → "connection-strings"
  const configPathMatch = pathname.match(/^\/organizations\/[^/]+\/configuration\/(.*)$/);
  const relativePath = configPathMatch ? configPathMatch[1] : "";

  // Build full URL from relative configuration URL
  const buildUrl = (url: string) => `/organizations/${orgId}/configuration/${url}`;

  return (
    <div className="min-h-screen bg-background">
      <PageHeader />

      {/* Main content */}
      <div className="mx-auto w-full max-w-[1600px] px-4 py-6 sm:px-6">
        <div className="mb-6">
          <h1 className="text-2xl font-semibold tracking-tight">Configuration</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage your SideSeat settings and integrations.
          </p>
        </div>

        <div className="flex flex-col gap-6 md:flex-row">
          {/* Section navigation - left sidebar */}
          <nav
            aria-label="Configuration sections"
            className="config-sidebar w-full shrink-0 md:w-48"
          >
            <ul className="flex flex-row gap-1 overflow-x-auto md:flex-col md:gap-0.5 md:overflow-visible">
              {configSections.map((section) => {
                // Use exactMatch field to determine matching behavior (defaults to true)
                const useExactMatch = section.exactMatch ?? true;
                const isActive = useExactMatch
                  ? relativePath === section.url
                  : isPathActive(`/${relativePath}`, `/${section.url}`);

                return (
                  <li key={section.id}>
                    <Link
                      to={buildUrl(section.url)}
                      aria-current={isActive ? "page" : undefined}
                      data-active={isActive}
                      className={cn(
                        "config-nav-item flex w-full items-center gap-2.5 whitespace-nowrap rounded-md px-3 py-2 text-sm transition-colors",
                        isActive
                          ? "bg-primary/10 font-medium text-primary"
                          : "text-muted-foreground hover:bg-muted hover:text-foreground",
                      )}
                    >
                      <section.icon
                        className={cn("h-4 w-4 shrink-0", isActive && "text-primary")}
                      />
                      {section.label}
                    </Link>
                  </li>
                );
              })}
            </ul>
          </nav>

          {/* Content area - right side */}
          <main className="config-content min-w-0 flex-1 rounded-lg border bg-card p-6">
            <QueryParamProvider adapter={ReactRouter6Adapter}>
              <Outlet />
            </QueryParamProvider>
          </main>
        </div>
      </div>
    </div>
  );
}
