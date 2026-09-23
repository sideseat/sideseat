import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";
import { defineConfig, globalIgnores } from "eslint/config";

export default defineConfig([
  globalIgnores(["dist", "src/components/ui"]),
  {
    files: ["**/*.{ts,tsx}"],
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
    // The application entry point mounts the app and exports nothing by design.
    // react-refresh 0.5 started flagging export-less files, which cannot apply here.
    // Must come last: in flat config later entries override earlier ones.
    files: ["src/main.tsx"],
    rules: { "react-refresh/only-export-components": "off" },
  },
]);
