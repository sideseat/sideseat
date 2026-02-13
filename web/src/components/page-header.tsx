import { Link } from "react-router";

import { useAuth } from "@/auth/context";
import { MainNav } from "@/components/main-nav";
import { ThemeSwitcher } from "@/components/theme-switcher";
import { brand } from "@/lib/navigation";

export function PageHeader() {
  const { version } = useAuth();

  return (
    <header className="page-header sticky top-0 z-40 flex h-14 items-center justify-between border-b bg-background/95 px-4 backdrop-blur supports-backdrop-filter:bg-background/60 sm:px-6">
      <Link to="/" className="flex items-center gap-2">
        <img
          src="/ui/icons/android-chrome-192x192.png"
          alt={brand.name}
          className="h-7 w-7 rounded-md"
        />
      </Link>
      <MainNav />
      <div className="flex items-center gap-2">
        {version && (
          <span className="hidden font-mono text-[10px] tracking-wider text-muted-foreground/60 sm:inline">
            v{version}
          </span>
        )}
        <ThemeSwitcher />
      </div>
    </header>
  );
}
