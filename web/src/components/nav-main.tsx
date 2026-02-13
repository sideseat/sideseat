import { useState, useEffect, useRef } from "react";
import { ChevronRight, MoreHorizontal } from "lucide-react";
import { Link, useLocation, useNavigate, useParams } from "react-router";

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SidebarGroup,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  useSidebar,
} from "@/components/ui/sidebar";
import { type NavigationItem, isPathActive } from "@/lib/navigation";
import { settings, SIDEBAR_SECTIONS_KEY } from "@/lib/settings";

export function NavMain({ items }: { items: NavigationItem[] }) {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const { projectId } = useParams<{ projectId: string }>();
  const { state, isMobile } = useSidebar();

  // Extract the path relative to the project (remove /projects/:projectId prefix)
  const projectPathMatch = pathname.match(/^\/projects\/[^/]+\/(.*)$/);
  const relativePath = projectPathMatch ? `/${projectPathMatch[1]}` : "/";

  // Build full URL from relative navigation URL
  const buildUrl = (url: string) => `/projects/${projectId}/${url}`;
  const [openSections, setOpenSections] = useState<Record<string, boolean>>(() => {
    const saved = settings.get<Record<string, boolean>>(SIDEBAR_SECTIONS_KEY);
    if (saved) return saved;

    // Initialize with defaultOpen values
    const defaults: Record<string, boolean> = {};
    for (const item of items) {
      if (item.defaultOpen && (item.id ?? item.title)) {
        defaults[item.id ?? item.title] = true;
      }
    }
    return defaults;
  });
  const containerRef = useRef<HTMLUListElement>(null);

  // In mobile mode, always show expanded view regardless of state
  const isCollapsed = state === "collapsed" && !isMobile;

  const [visibleCount, setVisibleCount] = useState<number>(items.length);

  const toggleSection = (id: string, isOpen: boolean) => {
    const newState = { ...openSections, [id]: isOpen };
    setOpenSections(newState);
    settings.set(SIDEBAR_SECTIONS_KEY, newState);
  };

  // Calculate overflow in collapsed mode
  useEffect(() => {
    // In mobile mode OR expanded state, always show all items
    if (isMobile || !isCollapsed) {
      setVisibleCount(items.length);
      return;
    }

    const calculateOverflow = () => {
      const menu = containerRef.current;
      if (!menu) return;

      // Find the SidebarContent parent which has the height constraint
      const sidebarContent = menu.closest('[data-sidebar="content"]') as HTMLElement;
      if (!sidebarContent) return;

      const availableHeight = sidebarContent.clientHeight;

      // Get dynamic measurements from actual elements
      const firstMenuItem = menu.querySelector('[data-sidebar="menu-button"]') as HTMLElement;
      if (!firstMenuItem) return;

      const itemHeight = firstMenuItem.offsetHeight;

      // Get gap from computed styles
      const menuStyles = window.getComputedStyle(menu);
      const gapHeight = parseFloat(menuStyles.gap) || 0;

      // Get group padding from the SidebarGroup parent
      const sidebarGroup = menu.closest('[data-sidebar="group"]') as HTMLElement;
      const groupStyles = sidebarGroup ? window.getComputedStyle(sidebarGroup) : null;
      const groupPadding = groupStyles
        ? parseFloat(groupStyles.paddingTop) + parseFloat(groupStyles.paddingBottom)
        : 0;

      // First, check if all items can fit WITHOUT the More button
      const totalHeightNeeded =
        groupPadding + items.length * itemHeight + (items.length - 1) * gapHeight;

      if (totalHeightNeeded <= availableHeight) {
        // All items fit, no need for More button
        setVisibleCount(items.length);
      } else {
        // Not all items fit, calculate with More button
        const moreButtonHeight = itemHeight;
        const usableHeight = availableHeight - groupPadding - moreButtonHeight;
        const itemsWithGaps = Math.floor(usableHeight / (itemHeight + gapHeight));
        const canFit = Math.max(0, itemsWithGaps);
        setVisibleCount(canFit);
      }
    };

    // Use RAF to ensure layout is complete
    const rafId = requestAnimationFrame(() => {
      calculateOverflow();
    });

    // Recalculate on resize
    const resizeObserver = new ResizeObserver(() => {
      requestAnimationFrame(calculateOverflow);
    });

    if (containerRef.current) {
      const sidebarContent = containerRef.current.closest('[data-sidebar="content"]');
      if (sidebarContent) {
        resizeObserver.observe(sidebarContent);
      }
    }

    return () => {
      cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
    };
  }, [isCollapsed, items.length, isMobile]);

  const visibleItems = isCollapsed ? items.slice(0, visibleCount) : items;
  const overflowItems = isCollapsed ? items.slice(visibleCount) : [];

  const renderMenuItem = (item: NavigationItem, forDropdown: boolean = false) => {
    const hasChildren = item.items && item.items.length > 0;
    // Use relativePath (with leading /) for matching against navigation URLs (which are relative, no leading /)
    const normalizedPath = relativePath.startsWith("/") ? relativePath.slice(1) : relativePath;
    const itemActive =
      hasChildren && item.items
        ? // Check if any child is active (respecting their exactMatch setting)
          item.items.some((child) => {
            const useExactMatch = child.exactMatch ?? true;
            return useExactMatch
              ? normalizedPath === child.url
              : isPathActive(`/${normalizedPath}`, `/${child.url}`);
          }) ||
          // For parent section URL, always use exact match
          normalizedPath === item.url
        : isPathActive(`/${normalizedPath}`, `/${item.url}`);

    if (!hasChildren) {
      if (forDropdown) {
        return (
          <DropdownMenuItem key={item.title} asChild>
            <Link to={buildUrl(item.url)} className="flex items-center gap-2">
              {item.icon && <item.icon />}
              <span>{item.title}</span>
            </Link>
          </DropdownMenuItem>
        );
      }

      return (
        <SidebarMenuItem key={item.title}>
          <SidebarMenuButton tooltip={item.title} isActive={itemActive} asChild>
            <Link to={buildUrl(item.url)}>
              {item.icon && <item.icon />}
              {!isCollapsed && <span>{item.title}</span>}
            </Link>
          </SidebarMenuButton>
        </SidebarMenuItem>
      );
    }

    const sectionId = item.id ?? item.title;
    const firstItemUrl = item.items?.[0]?.url;

    if (forDropdown) {
      return (
        <DropdownMenuItem
          key={item.title}
          onSelect={() => firstItemUrl && navigate(buildUrl(firstItemUrl))}
          className="flex items-center gap-2"
        >
          {item.icon && <item.icon />}
          <span>{item.title}</span>
        </DropdownMenuItem>
      );
    }

    const handleSectionClick = (e: React.MouseEvent) => {
      // In collapsed mode, navigate to first item instead of toggling
      if (isCollapsed && firstItemUrl) {
        e.preventDefault();
        navigate(buildUrl(firstItemUrl));
      }
    };

    return (
      <Collapsible
        key={item.title}
        asChild
        open={openSections[sectionId] ?? false}
        onOpenChange={(isOpen) => toggleSection(sectionId, isOpen)}
        className="group/collapsible"
      >
        <SidebarMenuItem>
          <CollapsibleTrigger asChild onClick={handleSectionClick}>
            <SidebarMenuButton
              tooltip={item.title}
              isActive={itemActive}
              className={isCollapsed ? "cursor-pointer" : ""}
            >
              {item.icon && <item.icon />}
              {!isCollapsed && <span>{item.title}</span>}
              {!isCollapsed && (
                <ChevronRight className="ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90" />
              )}
            </SidebarMenuButton>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <SidebarMenuSub>
              {item.items?.map((subItem) => {
                // Use exactMatch field to determine matching behavior (defaults to true)
                const useExactMatch = subItem.exactMatch ?? true;
                const isSubActive = useExactMatch
                  ? normalizedPath === subItem.url
                  : isPathActive(`/${normalizedPath}`, `/${subItem.url}`);

                return (
                  <SidebarMenuSubItem key={subItem.title}>
                    <SidebarMenuSubButton asChild isActive={isSubActive}>
                      <Link to={buildUrl(subItem.url)}>
                        {subItem.icon && <subItem.icon className="size-4" />}
                        {!isCollapsed && <span>{subItem.title}</span>}
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
  };

  return (
    <SidebarGroup>
      <SidebarMenu ref={containerRef}>
        {visibleItems.map((item) => renderMenuItem(item))}
        {isCollapsed && overflowItems.length > 0 && (
          <SidebarMenuItem>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <SidebarMenuButton tooltip="More">
                  <MoreHorizontal />
                </SidebarMenuButton>
              </DropdownMenuTrigger>
              <DropdownMenuContent side="right" align="start" className="w-48">
                {overflowItems.map((item) => renderMenuItem(item, true))}
              </DropdownMenuContent>
            </DropdownMenu>
          </SidebarMenuItem>
        )}
      </SidebarMenu>
    </SidebarGroup>
  );
}
