import { Fragment } from "react";
import { Link, Outlet, useLocation } from "react-router";
import { AppSidebar } from "@/components/app-sidebar";
import { ThemeSwitcher } from "@/components/theme-switcher";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Separator } from "@/components/ui/separator";
import { SidebarInset, SidebarProvider, SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { brand, findNavigationTrail, type NavigationItem } from "@/lib/navigation";

export default function App() {
  const { pathname } = useLocation();
  const navigationTrail = findNavigationTrail(pathname);

  return (
    <SidebarProvider>
      <AppSidebar />
      <SidebarInset>
        <LayoutHeader navigationTrail={navigationTrail} />
        <div className="flex flex-1 flex-col gap-4 p-4 pt-24 sm:p-6 sm:pt-28">
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

function LayoutHeader({ navigationTrail }: { navigationTrail: NavigationItem[] }) {
  const { state, isMobile } = useSidebar();

  const sidebarOffset = isMobile
    ? "0px"
    : state === "collapsed"
      ? "var(--sidebar-width-icon)"
      : "var(--sidebar-width)";

  return (
    <header
      style={{ left: sidebarOffset, right: 0 }}
      className="fixed top-0 z-40 flex h-16 shrink-0 items-center gap-2 border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60 transition-[left,height] ease-linear"
    >
      <div className="flex w-full items-center gap-3 px-4 sm:px-6">
        <SidebarTrigger className="-ml-1" />
        <Separator orientation="vertical" className="h-6" />
        <Breadcrumb>
          <BreadcrumbList>
            <BreadcrumbItem className="hidden md:block">
              <BreadcrumbLink asChild>
                <Link to="/">{brand.description}</Link>
              </BreadcrumbLink>
            </BreadcrumbItem>
            {navigationTrail.length > 0 ? (
              navigationTrail.map((crumb, index) => (
                <Fragment key={crumb.title}>
                  <BreadcrumbSeparator className={index === 0 ? "hidden md:block" : undefined} />
                  <BreadcrumbItem>
                    {index === navigationTrail.length - 1 ? (
                      <BreadcrumbPage>{crumb.title}</BreadcrumbPage>
                    ) : (
                      <BreadcrumbLink asChild>
                        <Link to={crumb.url}>{crumb.title}</Link>
                      </BreadcrumbLink>
                    )}
                  </BreadcrumbItem>
                </Fragment>
              ))
            ) : (
              <BreadcrumbItem>
                <BreadcrumbPage>Overview</BreadcrumbPage>
              </BreadcrumbItem>
            )}
          </BreadcrumbList>
        </Breadcrumb>
        <div className="ml-auto flex items-center gap-2">
          <ThemeSwitcher />
        </div>
      </div>
    </header>
  );
}
