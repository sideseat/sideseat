import { Moon, Sun, Monitor, Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useTheme } from "@/components/theme-provider";
import { cn, tw } from "@/lib/utils";

const colorSchemes = [
  {
    id: "professional" as const,
    name: "Professional",
    description: "Clean & minimal",
    gradient: tw("from-scheme-professional-from to-scheme-professional-to"),
  },
  {
    id: "fancy" as const,
    name: "Fancy",
    description: "Purple vibes",
    gradient: tw("from-scheme-fancy-from to-scheme-fancy-to"),
  },
  {
    id: "ocean" as const,
    name: "Ocean",
    description: "Calm & focused",
    gradient: tw("from-scheme-ocean-from to-scheme-ocean-to"),
  },
  {
    id: "ember" as const,
    name: "Ember",
    description: "Warm & creative",
    gradient: tw("from-scheme-ember-from to-scheme-ember-to"),
  },
] as const;

export function ThemeSwitcher() {
  const { theme, setTheme, colorScheme, setColorScheme } = useTheme();

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="icon" className="h-9 w-9">
          <Sun className="h-4 w-4 rotate-0 scale-100 transition-all dark:-rotate-90 dark:scale-0" />
          <Moon className="absolute h-4 w-4 rotate-90 scale-0 transition-all dark:rotate-0 dark:scale-100" />
          <span className="sr-only">Theme settings</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-56 p-2">
        {/* Mode Selection */}
        <div className="mb-2">
          <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">Mode</div>
          <div className="grid grid-cols-3 gap-1">
            <button
              type="button"
              onClick={() => setTheme("light")}
              className={cn(
                "flex flex-col items-center gap-1 rounded-md px-2 py-1.5 text-xs transition-colors",
                theme === "light" ? "bg-primary text-primary-foreground" : "hover:bg-accent",
              )}
            >
              <Sun className="h-4 w-4" />
              Light
            </button>
            <button
              type="button"
              onClick={() => setTheme("dark")}
              className={cn(
                "flex flex-col items-center gap-1 rounded-md px-2 py-1.5 text-xs transition-colors",
                theme === "dark" ? "bg-primary text-primary-foreground" : "hover:bg-accent",
              )}
            >
              <Moon className="h-4 w-4" />
              Dark
            </button>
            <button
              type="button"
              onClick={() => setTheme("system")}
              className={cn(
                "flex flex-col items-center gap-1 rounded-md px-2 py-1.5 text-xs transition-colors",
                theme === "system" ? "bg-primary text-primary-foreground" : "hover:bg-accent",
              )}
            >
              <Monitor className="h-4 w-4" />
              Auto
            </button>
          </div>
        </div>

        <div className="my-2 h-px bg-border" />

        {/* Color Scheme Selection */}
        <div>
          <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">Style</div>
          <div className="space-y-1">
            {colorSchemes.map((scheme) => (
              <button
                type="button"
                key={scheme.id}
                onClick={() => setColorScheme(scheme.id)}
                className={cn(
                  "flex w-full items-center gap-3 rounded-md px-2 py-2 transition-colors",
                  colorScheme === scheme.id ? "bg-accent" : "hover:bg-accent/50",
                )}
              >
                <div
                  className={cn("h-8 w-8 shrink-0 rounded-md bg-linear-to-br", scheme.gradient)}
                />
                <div className="flex-1 text-left">
                  <div className="text-sm font-medium">{scheme.name}</div>
                  <div className="text-xs text-muted-foreground">{scheme.description}</div>
                </div>
                {colorScheme === scheme.id && <Check className="h-4 w-4 shrink-0 text-primary" />}
              </button>
            ))}
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
