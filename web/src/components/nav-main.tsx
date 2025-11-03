"use client";

import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { Link, useLocation } from "react-router";

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import { type NavigationItem, isPathActive } from "@/lib/navigation";
import { settings } from "@/lib/settings";

const SIDEBAR_SECTIONS_KEY = "sidebar_sections_state";

export function NavMain({ items }: { items: NavigationItem[] }) {
  const { pathname } = useLocation();
  const [openSections, setOpenSections] = useState<Record<string, boolean>>(() => {
    const saved = settings.get<Record<string, boolean>>(SIDEBAR_SECTIONS_KEY);
    return saved ?? {};
  });

  const toggleSection = (id: string, isOpen: boolean) => {
    const newState = { ...openSections, [id]: isOpen };
    setOpenSections(newState);
    settings.set(SIDEBAR_SECTIONS_KEY, newState);
  };

  return (
    <SidebarGroup>
      <SidebarGroupLabel>Platform</SidebarGroupLabel>
      <SidebarMenu>
        {items.map((item) => {
          const hasChildren = item.items && item.items.length > 0;
          const itemActive =
            hasChildren && item.items
              ? item.items.some((child) => isPathActive(pathname, child.url)) ||
                isPathActive(pathname, item.url)
              : isPathActive(pathname, item.url);

          if (!hasChildren) {
            return (
              <SidebarMenuItem key={item.title}>
                <SidebarMenuButton tooltip={item.title} isActive={itemActive} asChild>
                  <Link to={item.url}>
                    {item.icon && <item.icon />}
                    <span>{item.title}</span>
                  </Link>
                </SidebarMenuButton>
              </SidebarMenuItem>
            );
          }

          const sectionId = item.id ?? item.title;

          return (
            <Collapsible
              key={item.title}
              asChild
              open={openSections[sectionId] ?? false}
              onOpenChange={(isOpen) => toggleSection(sectionId, isOpen)}
              className="group/collapsible"
            >
              <SidebarMenuItem>
                <CollapsibleTrigger asChild>
                  <SidebarMenuButton tooltip={item.title} isActive={itemActive}>
                    {item.icon && <item.icon />}
                    <span>{item.title}</span>
                    <ChevronRight className="ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90" />
                  </SidebarMenuButton>
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <SidebarMenuSub>
                    {item.items?.map((subItem) => {
                      const isSubActive = isPathActive(pathname, subItem.url);

                      return (
                        <SidebarMenuSubItem key={subItem.title}>
                          <SidebarMenuSubButton asChild isActive={isSubActive}>
                            <Link to={subItem.url}>
                              <span>{subItem.title}</span>
                            </Link>
                          </SidebarMenuSubButton>
                        </SidebarMenuSubItem>
                      );
                    })}
                  </SidebarMenuSub>
                </CollapsibleContent>
              </SidebarMenuItem>
            </Collapsible>
          );
        })}
      </SidebarMenu>
    </SidebarGroup>
  );
}
