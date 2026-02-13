import { Link, useLocation, useParams } from "react-router";
import { FolderKanban, Settings, type LucideIcon } from "lucide-react";

import { cn } from "@/lib/utils";

type NavItem = {
  to: string | ((projectId?: string) => string);
  activePrefix: string;
  label: string;
  icon: LucideIcon;
};

const DEFAULT_ORG_ID = "default";

const navItems: NavItem[] = [
  { to: "/", activePrefix: "/", label: "Projects", icon: FolderKanban },
  {
    to: (projectId) => {
      const base = `/organizations/${DEFAULT_ORG_ID}/configuration/telemetry`;
      return projectId ? `${base}?project=${projectId}` : base;
    },
    activePrefix: "/organizations",
    label: "Configuration",
    icon: Settings,
  },
];

export function MainNav() {
  const { pathname } = useLocation();
  const { projectId } = useParams<{ projectId: string }>();

  return (
    <nav
      aria-label="Main navigation"
      className="main-nav absolute left-1/2 grid -translate-x-1/2 grid-cols-2 gap-1 rounded-lg bg-muted p-1"
    >
      {navItems.map((item) => {
        // "/" is active only on exact match; other prefixes use startsWith
        const isActive =
          item.activePrefix === "/" ? pathname === "/" : pathname.startsWith(item.activePrefix);
        const to = typeof item.to === "function" ? item.to(projectId) : item.to;
        return (
          <Link
            key={typeof item.to === "function" ? item.label : item.to}
            to={to}
            aria-current={isActive ? "page" : undefined}
            data-active={isActive}
            className={cn(
              "main-nav-item inline-flex h-7 items-center justify-center gap-1.5 rounded-md px-3 text-sm font-medium transition-all sm:px-4",
              isActive ? "bg-background shadow-sm" : "text-foreground/70 hover:text-foreground",
            )}
          >
            <item.icon className="h-4 w-4 shrink-0" />
            <span className="hidden sm:inline">{item.label}</span>
          </Link>
        );
      })}
    </nav>
  );
}
