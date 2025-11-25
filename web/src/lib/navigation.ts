import {
  Activity,
  Bug,
  FileText,
  Home,
  LayoutDashboard,
  Network,
  Settings2,
  Sparkles,
  Zap,
} from "lucide-react";

import type { LucideIcon } from "lucide-react";

export type NavigationItem = {
  id?: string;
  title: string;
  url: string;
  icon?: LucideIcon;
  external?: boolean;
  exactMatch?: boolean; // If true, only exact URL match. If false, matches all sub-URLs. Defaults to true.
  items?: NavigationItem[];
};

export const brand = {
  name: "SideSeat",
  description: "AI Development Toolkit",
  icon: Sparkles,
  docsUrl: "https://sideseat.dev",
  docsIcon: Zap,
};

export const mainNavigation: NavigationItem[] = [
  {
    title: "Home",
    url: "/",
    icon: Home,
  },
  {
    title: "Dashboard",
    url: "/dashboard",
    icon: LayoutDashboard,
  },
  {
    id: "traces",
    title: "Traces",
    url: "/traces",
    icon: Activity,
    items: [
      {
        title: "View All",
        url: "/traces",
      },
      {
        title: "Analytics",
        url: "/traces/analytics",
      },
    ],
  },
  {
    id: "prompts",
    title: "Prompts",
    url: "/prompts",
    icon: FileText,
    items: [
      {
        title: "Library",
        url: "/prompts",
      },
      {
        title: "Templates",
        url: "/prompts/templates",
      },
    ],
  },
  {
    title: "Proxy",
    url: "/proxy",
    icon: Network,
  },
  {
    id: "debuggers",
    title: "Debuggers",
    url: "/debugger",
    icon: Bug,
    items: [
      {
        title: "MCP Debugger",
        url: "/debugger/mcp",
        exactMatch: false, // Will match /debugger/mcp and all sub-paths
      },
      {
        title: "A2A Debugger",
        url: "/debugger/a2a",
        exactMatch: false, // Will match /debugger/a2a and all sub-paths
      },
    ],
  },
  {
    title: "Settings",
    url: "/settings",
    icon: Settings2,
  },
];

export function isPathActive(pathname: string, target: string) {
  if (!target || target === "#") return false;
  if (target === "/") return pathname === "/";
  return pathname === target || pathname.startsWith(`${target}/`);
}

export function findNavigationTrail(pathname: string, items = mainNavigation): NavigationItem[] {
  for (const item of items) {
    if (isPathActive(pathname, item.url)) {
      return [item];
    }

    if (item.items?.length) {
      const childTrail = findNavigationTrail(pathname, item.items);
      if (childTrail.length) {
        return [item, ...childTrail];
      }
    }
  }

  return [];
}
