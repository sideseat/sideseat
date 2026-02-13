import { Eye, GitBranch, Home, Layers, Radio, Sparkles, UsersRound, Zap } from "lucide-react";

import type { LucideIcon } from "lucide-react";

export type NavigationItem = {
  id?: string;
  title: string;
  url: string;
  icon?: LucideIcon;
  external?: boolean;
  exactMatch?: boolean; // If true, only exact URL match. If false, matches all sub-URLs. Defaults to true.
  defaultOpen?: boolean; // If true, section is open by default
  items?: NavigationItem[];
};

export const brand = {
  name: "SideSeat",
  description: "AI Development Workbench",
  icon: Sparkles,
  docsUrl: "https://sideseat.ai",
  docsIcon: Zap,
};

export const mainNavigation: NavigationItem[] = [
  {
    title: "Home",
    url: "home",
    icon: Home,
  },
  {
    id: "observability",
    title: "Observability",
    url: "observability",
    icon: Eye,
    exactMatch: false,
    defaultOpen: true,
    items: [
      {
        title: "Realtime",
        url: "observability/realtime",
        icon: Radio,
      },
      {
        title: "Traces",
        url: "observability/traces",
        icon: GitBranch,
        exactMatch: false,
      },
      {
        title: "Spans",
        url: "observability/spans",
        icon: Layers,
        exactMatch: false,
      },
      {
        title: "Sessions",
        url: "observability/sessions",
        icon: UsersRound,
      },
    ],
  },
];

// Normalize path by ensuring it has a leading slash
function normalizePath(path: string): string {
  return path.startsWith("/") ? path : `/${path}`;
}

export function isPathActive(pathname: string, target: string) {
  if (!target || target === "#") return false;
  if (target === "/" || target === "") return pathname === "/" || pathname === "";

  const normalizedPathname = normalizePath(pathname);
  const normalizedTarget = normalizePath(target);

  return (
    normalizedPathname === normalizedTarget || normalizedPathname.startsWith(`${normalizedTarget}/`)
  );
}

export function findNavigationTrail(pathname: string, items = mainNavigation): NavigationItem[] {
  const normalizedPathname = normalizePath(pathname);

  for (const item of items) {
    const normalizedItemUrl = normalizePath(item.url);

    // Check children first (more specific matches)
    if (item.items?.length) {
      const childTrail = findNavigationTrail(normalizedPathname, item.items);
      if (childTrail.length) {
        return [item, ...childTrail];
      }
    }

    // Then check this item
    if (isPathActive(normalizedPathname, normalizedItemUrl)) {
      return [item];
    }
  }

  return [];
}
