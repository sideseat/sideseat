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
  title: string;
  url: string;
  icon?: LucideIcon;
  external?: boolean;
  items?: NavigationItem[];
};

export const brand = {
  name: "SideSeat",
  description: "AI Development Toolkit",
  icon: Sparkles,
  docsUrl: "https://sideseat.spugachev.com",
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
    title: "Debuggers",
    url: "/debugger",
    icon: Bug,
    items: [
      {
        title: "MCP Debugger",
        url: "/debugger/mcp",
      },
      {
        title: "A2A Debugger",
        url: "/debugger/a2a",
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
