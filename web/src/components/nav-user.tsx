import { ChevronsUpDown, LogOut, Settings, User } from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { useOrganizations } from "@/api/organizations";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import { useAuth } from "@/auth/context";
import { cn } from "@/lib/utils";

function UserAvatar({ name, className }: { name?: string; className?: string }) {
  const initials = name
    ? name
        .split(" ")
        .filter((part) => part.length > 0)
        .map((part) => part[0])
        .join("")
        .toUpperCase()
        .slice(0, 2)
    : null;

  return (
    <div
      className={cn(
        "flex items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground",
        className,
      )}
    >
      {initials ? (
        <span className="text-xs font-medium">{initials}</span>
      ) : (
        <User className="size-4" />
      )}
    </div>
  );
}

function UserInfo({
  displayName,
  email,
  avatarName,
}: {
  displayName: string;
  email?: string;
  avatarName?: string;
}) {
  return (
    <>
      <UserAvatar name={avatarName} className="size-8" />
      <div className="grid flex-1 text-left text-sm leading-tight">
        <span className="truncate font-medium">{displayName}</span>
        {email && <span className="truncate text-xs text-muted-foreground">{email}</span>}
      </div>
    </>
  );
}

export function NavUser() {
  const { isMobile } = useSidebar();
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const { projectId } = useParams<{ projectId: string }>();
  const { data: orgsData } = useOrganizations();

  const displayName = user?.display_name || user?.email || "User";
  const email = user?.email;
  const defaultOrg = orgsData?.data?.[0];

  const handleLogout = async () => {
    try {
      await logout();
      navigate("/auth");
    } catch {
      // Logout failed - stay on current page
      // Error toast is handled by the auth context
    }
  };

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <SidebarMenuButton
              size="lg"
              className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
            >
              <UserInfo displayName={displayName} email={email} avatarName={user?.display_name} />
              <ChevronsUpDown className="ml-auto size-4" />
            </SidebarMenuButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            className="w-[--radix-dropdown-menu-trigger-width] min-w-56 rounded-lg"
            side={isMobile ? "bottom" : "right"}
            align="end"
            sideOffset={4}
          >
            <DropdownMenuLabel className="p-0 font-normal">
              <div className="flex items-center gap-2 px-1 py-1.5 text-left text-sm">
                <UserInfo displayName={displayName} email={email} avatarName={user?.display_name} />
              </div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              <DropdownMenuItem
                onClick={() => {
                  if (!defaultOrg) return;
                  const base = `/organizations/${defaultOrg.id}/configuration/telemetry`;
                  navigate(projectId ? `${base}?project=${projectId}` : base);
                }}
                disabled={!defaultOrg}
              >
                <Settings className="mr-2 size-4" />
                Configuration
              </DropdownMenuItem>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={handleLogout}>
              <LogOut className="mr-2 size-4" />
              Sign out
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}
