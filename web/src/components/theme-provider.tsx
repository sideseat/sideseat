/* eslint-disable react-refresh/only-export-components */
import { createContext, useContext, useEffect, useState } from "react";
import { settings, COLOR_SCHEME_KEY } from "@/lib/settings";

export type Theme = "dark" | "light" | "system";
export type ColorScheme = "professional" | "fancy" | "ocean" | "ember";

type ThemeProviderProps = {
  children: React.ReactNode;
  defaultTheme?: Theme;
  defaultColorScheme?: ColorScheme;
  storageKey?: string;
};

type ThemeProviderState = {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  resolvedTheme: "light" | "dark";
  colorScheme: ColorScheme;
  setColorScheme: (scheme: ColorScheme) => void;
};

const initialState: ThemeProviderState = {
  theme: "system",
  setTheme: () => null,
  resolvedTheme: "light",
  colorScheme: "professional",
  setColorScheme: () => null,
};

const ThemeProviderContext = createContext<ThemeProviderState>(initialState);

export function ThemeProvider({
  children,
  defaultTheme = "system",
  defaultColorScheme = "professional",
  storageKey = "theme",
}: ThemeProviderProps) {
  const [theme, setTheme] = useState<Theme>(() => settings.get<Theme>(storageKey) || defaultTheme);
  const [resolvedTheme, setResolvedTheme] = useState<"light" | "dark">(() => {
    if (typeof window === "undefined") return "light";
    if (theme === "system") {
      return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    }
    return theme;
  });
  const [colorScheme, setColorScheme] = useState<ColorScheme>(
    () => settings.get<ColorScheme>(COLOR_SCHEME_KEY) || defaultColorScheme,
  );

  useEffect(() => {
    const root = window.document.documentElement;

    const applyTheme = (resolvedTheme: "light" | "dark") => {
      root.classList.remove("light", "dark");
      root.classList.add(resolvedTheme);
      setResolvedTheme(resolvedTheme);
    };

    if (theme === "system") {
      const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
      const systemTheme = mediaQuery.matches ? "dark" : "light";
      applyTheme(systemTheme);

      const handleChange = (e: MediaQueryListEvent) => {
        applyTheme(e.matches ? "dark" : "light");
      };

      mediaQuery.addEventListener("change", handleChange);
      return () => mediaQuery.removeEventListener("change", handleChange);
    } else {
      applyTheme(theme);
    }
  }, [theme]);

  useEffect(() => {
    const root = window.document.documentElement;

    root.classList.remove("professional", "fancy", "ocean", "ember");
    root.classList.add(colorScheme);
  }, [colorScheme]);

  const value = {
    theme,
    setTheme: (theme: Theme) => {
      settings.set(storageKey, theme);
      setTheme(theme);
    },
    resolvedTheme,
    colorScheme,
    setColorScheme: (scheme: ColorScheme) => {
      settings.set(COLOR_SCHEME_KEY, scheme);
      setColorScheme(scheme);
    },
  };

  return <ThemeProviderContext.Provider value={value}>{children}</ThemeProviderContext.Provider>;
}

export const useTheme = () => {
  const context = useContext(ThemeProviderContext);

  if (context === undefined) throw new Error("useTheme must be used within a ThemeProvider");

  return context;
};
