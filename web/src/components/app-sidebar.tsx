"use client";

import * as React from "react";
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

import { NavMain } from "@/components/nav-main";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from "@/components/ui/sidebar";
import { Link } from "react-router";

const data = {
  navMain: [
    {
      title: "Home",
      url: "/",
      icon: Home,
      isActive: true,
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
      url: "#",
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
  ],
};

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  return (
    <Sidebar collapsible="icon" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild>
              <Link to="/">
                <div className="flex aspect-square size-8 items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
                  <Sparkles className="size-4" />
                </div>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-semibold">SideSeat</span>
                  <span className="truncate text-xs">AI Development Toolkit</span>
                </div>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <NavMain items={data.navMain} />
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton asChild>
              <a href="https://github.com" target="_blank" rel="noopener noreferrer">
                <Zap className="size-4" />
                <span>Documentation</span>
              </a>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}
