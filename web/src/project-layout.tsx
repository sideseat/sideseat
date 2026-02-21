import { Suspense, useEffect, useState } from "react";
import { Link, Outlet, useLocation, useNavigate, useParams } from "react-router";
import { QueryParamProvider } from "use-query-params";
import { ReactRouter6Adapter } from "use-query-params/adapters/react-router-6";
import { AlertCircle, Check, ChevronDown, Home, Plug } from "lucide-react";
import { AppSidebar } from "@/components/app-sidebar";
import { ProjectSwitcher } from "@/components/project-switcher";
import { ThemeSwitcher } from "@/components/theme-switcher";
import { AppProvider } from "@/lib/app-context";
import { ProjectProvider } from "@/lib/project-provider";
import { useCurrentProject } from "@/hooks/use-project";
import {
  Breadcrumb,
  BreadcrumbEllipsis,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { SidebarInset, SidebarProvider, SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { findNavigationTrail, mainNavigation, type NavigationItem } from "@/lib/navigation";
import { prefetchOnIdle } from "@/lib/prefetch";

export default function ProjectLayout() {
  return (
    <QueryParamProvider adapter={ReactRouter6Adapter}>
      <AppProvider>
        <ProjectProvider>
          <SidebarProvider>
            <ProjectLayoutContent />
          </SidebarProvider>
        </ProjectProvider>
      </AppProvider>
    </QueryParamProvider>
  );
}

function ProjectLayoutContent() {
  const { project, isLoading, error } = useCurrentProject();
  const { pathname } = useLocation();

  const projectPathMatch = pathname.match(/^\/projects\/[^/]+\/(.*)$/);
  const relativePath = projectPathMatch ? `/${projectPathMatch[1]}` : "/";
  const navigationTrail = findNavigationTrail(relativePath);

  useEffect(() => {
    return prefetchOnIdle(
      () => import("./pages/project-home"),
      () => import("./pages/observability/traces/traces"),
      () => import("./pages/observability/trace/trace-detail-page"),
      () => import("./pages/observability/sessions/sessions"),
      () => import("./pages/observability/session/session-detail-page"),
      () => import("./pages/observability/spans/spans"),
      () => import("./pages/observability/span/span-detail-page"),
      () => import("./pages/observability/realtime"),
    );
  }, []);

  if (isLoading) {
    return <ProjectLoadingState />;
  }

  if (error || !project) {
    return <ProjectErrorState />;
  }

  return (
    <>
      <AppSidebar />
      <SidebarInset className="min-w-0">
        <LayoutHeader navigationTrail={navigationTrail} />
        <div className="flex flex-1 flex-col">
          <Suspense
            fallback={
              <div className="flex h-64 w-full items-center justify-center">
                <div className="flex flex-col items-center gap-4">
                  <div className="h-8 w-48 animate-pulse rounded-md bg-muted" />
                  <div className="h-4 w-32 animate-pulse rounded-md bg-muted" />
                </div>
              </div>
            }
          >
            <Outlet />
          </Suspense>
        </div>
      </SidebarInset>
    </>
  );
}

function LayoutHeader({ navigationTrail }: { navigationTrail: NavigationItem[] }) {
  const { state, isMobile } = useSidebar();
  const { projectId, traceId, sessionId, spanId } = useParams<{
    projectId: string;
    traceId?: string;
    sessionId?: string;
    spanId?: string;
  }>();
  const { project } = useCurrentProject();
  const { pathname } = useLocation();
  const [createDialogOpen, setCreateDialogOpen] = useState(false);

  const isSpanDetail = pathname.includes("/spans/") && traceId && spanId;
  const detailPageTitle = isSpanDetail ? "Span" : traceId ? "Trace" : sessionId ? "Session" : null;

  const sidebarOffset = isMobile
    ? "0px"
    : state === "collapsed"
      ? "var(--sidebar-width-icon)"
      : "var(--sidebar-width)";

  const parentSection = navigationTrail.find((item) => item.items && item.items.length > 0);
  const activeSubPage =
    navigationTrail.length > 1 ? navigationTrail[navigationTrail.length - 1] : null;

  const collapsedDropdownItems = [
    { label: "All Projects", to: "/" },
    project && { label: project.name, to: `/projects/${projectId}/home` },
    parentSection && {
      label: parentSection.title,
      to: `/projects/${projectId}/${parentSection.items?.[0]?.url ?? parentSection.url}`,
    },
    detailPageTitle &&
      activeSubPage && {
        label: activeSubPage.title,
        to: `/projects/${projectId}/${activeSubPage.url}`,
      },
  ].filter(Boolean) as { label: string; to: string }[];

  const collapsedCurrentPage =
    detailPageTitle ?? activeSubPage?.title ?? navigationTrail[0]?.title ?? "Home";

  return (
    <header
      style={{ left: sidebarOffset, right: 0, height: "var(--header-height)" }}
      className="fixed top-0 z-40 flex shrink-0 items-center gap-2 border-b bg-background/95 backdrop-blur supports-backdrop-filter:bg-background/60 transition-[left,height] ease-linear"
    >
      <div className="flex w-full items-center gap-3 px-2 sm:px-4">
        <SidebarTrigger className="-ml-1" />
        <Separator orientation="vertical" className="h-6" />
        <Breadcrumb>
          <BreadcrumbList>
            {/* Mobile/Tablet: Collapsed view */}
            <BreadcrumbItem className="lg:hidden">
              <DropdownMenu>
                <DropdownMenuTrigger className="flex items-center gap-1">
                  <BreadcrumbEllipsis />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
                  {collapsedDropdownItems.map((item) => (
                    <DropdownMenuItem key={item.to} asChild>
                      <Link to={item.to}>{item.label}</Link>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </BreadcrumbItem>
            <BreadcrumbSeparator className="lg:hidden" />
            <BreadcrumbItem className="lg:hidden">
              <BreadcrumbPage>{collapsedCurrentPage}</BreadcrumbPage>
            </BreadcrumbItem>

            {/* Desktop: Full breadcrumb */}
            <BreadcrumbItem className="hidden lg:block">
              <ProjectSwitcher
                createDialogOpen={createDialogOpen}
                onCreateDialogOpenChange={setCreateDialogOpen}
              />
            </BreadcrumbItem>

            {parentSection?.items ? (
              <>
                <BreadcrumbSeparator className="hidden lg:block" />
                <BreadcrumbItem className="hidden lg:block">
                  <SectionDropdown section={parentSection} projectId={projectId!} />
                </BreadcrumbItem>

                {activeSubPage && (
                  <>
                    <BreadcrumbSeparator className="hidden lg:block" />
                    <BreadcrumbItem className="hidden lg:block">
                      {detailPageTitle ? (
                        <BreadcrumbLink asChild>
                          <Link to={`/projects/${projectId}/${activeSubPage.url}`}>
                            {activeSubPage.title}
                          </Link>
                        </BreadcrumbLink>
                      ) : (
                        <BreadcrumbPage>{activeSubPage.title}</BreadcrumbPage>
                      )}
                    </BreadcrumbItem>

                    {detailPageTitle && (
                      <>
                        <BreadcrumbSeparator className="hidden lg:block" />
                        <BreadcrumbItem className="hidden lg:block">
                          <BreadcrumbPage>{detailPageTitle}</BreadcrumbPage>
                        </BreadcrumbItem>
                      </>
                    )}
                  </>
                )}
              </>
            ) : navigationTrail.length > 0 ? (
              <>
                <BreadcrumbSeparator className="hidden lg:block" />
                <BreadcrumbItem className="hidden lg:block">
                  <BreadcrumbPage>{navigationTrail[0].title}</BreadcrumbPage>
                </BreadcrumbItem>
              </>
            ) : null}
          </BreadcrumbList>
        </Breadcrumb>
        <div className="ml-auto flex items-center gap-2">
          <Button variant="outline" size="sm" className="h-8 gap-1.5" asChild>
            <Link to={`/organizations/default/configuration/telemetry?project=${projectId}`}>
              <Plug className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">Connect</span>
            </Link>
          </Button>
          <ThemeSwitcher />
        </div>
      </div>
    </header>
  );
}

function SectionDropdown({ section, projectId }: { section: NavigationItem; projectId: string }) {
  const sections = mainNavigation.filter((item) => item.items && item.items.length > 0);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex items-center gap-1 rounded-md px-2 py-1 text-sm font-medium hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        {section.icon && <section.icon className="mr-1 h-4 w-4" />}
        {section.title}
        <ChevronDown className="h-4 w-4 opacity-50" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-48">
        {sections.map((item) => (
          <DropdownMenuItem key={item.url} asChild>
            <Link
              to={`/projects/${projectId}/${item.items?.[0]?.url ?? item.url}`}
              className="flex items-center justify-between"
            >
              <div className="flex items-center gap-2">
                {item.icon && <item.icon className="h-4 w-4" />}
                <span>{item.title}</span>
              </div>
              {section.id === item.id && <Check className="h-4 w-4 shrink-0" />}
            </Link>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ProjectLoadingState() {
  return (
    <div className="flex h-screen w-full items-center justify-center">
      <div className="flex flex-col items-center gap-4">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-4 w-32" />
      </div>
    </div>
  );
}

function ProjectErrorState() {
  const navigate = useNavigate();

  return (
    <div className="flex h-screen w-full items-center justify-center">
      <div className="flex flex-col items-center gap-6 text-center">
        <div className="rounded-full bg-destructive/10 p-4">
          <AlertCircle className="h-8 w-8 text-destructive" />
        </div>
        <div className="space-y-2">
          <h1 className="text-2xl font-semibold">Project not found</h1>
          <p className="text-muted-foreground">
            The project you're looking for doesn't exist or you don't have access to it.
          </p>
        </div>
        <Button onClick={() => navigate("/")} className="gap-2">
          <Home className="h-4 w-4" />
          Back to Projects
        </Button>
      </div>
    </div>
  );
}
