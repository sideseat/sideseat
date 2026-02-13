import type { ComponentProps } from "react";
import { Link } from "react-router";

import { NavMain } from "@/components/nav-main";
import { NavUser } from "@/components/nav-user";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from "@/components/ui/sidebar";
import { brand, mainNavigation } from "@/lib/navigation";

export function AppSidebar(props: ComponentProps<typeof Sidebar>) {
  const { state, isMobile } = useSidebar();
  // On mobile, sidebar shows as overlay even when "collapsed", so show full content
  const isCollapsed = state === "collapsed" && !isMobile;

  return (
    <Sidebar collapsible="icon" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild>
              <Link to="/">
                <div className="flex aspect-square size-8 items-center justify-center">
                  <img
                    src="/ui/icons/android-chrome-192x192.png"
                    alt={brand.name}
                    className="size-8 rounded-lg"
                  />
                </div>
                {!isCollapsed && (
                  <div className="grid flex-1 text-left text-sm leading-tight">
                    <span className="truncate font-semibold">{brand.name}</span>
                    <span className="truncate text-xs">{brand.description}</span>
                  </div>
                )}
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <NavMain items={mainNavigation} />
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton asChild>
              <a href={brand.docsUrl} target="_blank" rel="noopener noreferrer">
                <brand.docsIcon className="size-4" />
                {!isCollapsed && <span>Documentation</span>}
              </a>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
        <NavUser />
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}
