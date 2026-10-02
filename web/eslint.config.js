import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";
import { plugin as shadcn } from "@shadcn/lint";
import tsParser from "@typescript-eslint/parser";
import { defineConfig, globalIgnores } from "eslint/config";

export default defineConfig([
  globalIgnores(["dist"]),
  {
    files: ["**/*.{ts,tsx}"],
    // Vendored shadcn/ui primitives follow upstream style, not these rule sets.
    ignores: ["src/components/ui/**"],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      // Use the plugin's ESLint 10-compatible flat configuration.
      reactHooks.configs.flat["recommended-latest"],
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
  },
  {
    // Design-system rules. Tokens live in src/styles/index.css; component
    // variants and sizes live in src/components/ui/.
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    plugins: { shadcn },
    settings: {
      shadcn: {
        note: "Theme tokens and type steps live in src/styles/index.css; variants and sizes in src/components/ui/.",
      },
    },
    rules: {
      "shadcn/no-restyle": [
        "error",
        {
          allow: ["layout"],
          contracts: [
            {
              // Containers whose padding and gap depend on what the page puts in them.
              pattern:
                "^(CardHeader|CardContent|CardFooter|SheetHeader|SheetFooter|DialogHeader|DialogFooter|TabsContent|CollapsibleContent|PopoverContent)$",
              allow: ["layout", "spacing"],
            },
            // Spinner is an icon, so callers color it like any other icon.
            { pattern: "^Spinner$", allow: ["layout", "color"] },
          ],
        },
      ],
      "shadcn/no-raw-colors": "error",
      "shadcn/no-arbitrary-values": "error",
      "shadcn/no-inline-styles": "error",
      "shadcn/no-unknown-classes": "error",
      "shadcn/require-static-classes": "error",
    },
  },
  {
    // Primitives own their appearance and call their own variant functions,
    // so only the token rules (colors, inline styles, unknown classes) apply.
    files: ["src/components/ui/**"],
    rules: {
      "shadcn/no-restyle": "off",
      "shadcn/no-arbitrary-values": "off",
      "shadcn/require-static-classes": "off",
    },
  },
  {
    // The application entry point mounts the app and exports nothing by design.
    // react-refresh 0.5 started flagging export-less files, which cannot apply here.
    // Must come last: in flat config later entries override earlier ones.
    files: ["src/main.tsx"],
    rules: { "react-refresh/only-export-components": "off" },
  },
]);
