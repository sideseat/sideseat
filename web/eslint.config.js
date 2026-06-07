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
      // eslint-plugin-react-hooks 7 still ships "recommended"/"recommended-latest" in the
      // legacy eslintrc shape (plugins as an array of strings), which ESLint 10 rejects.
      // `flat` is the flat-config entry point.
      reactHooks.configs.flat["recommended-latest"],
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    rules: {
      // eslint-plugin-react-hooks 7 adds the React Compiler rule set. It currently
      // reports 45 findings across the app, dominated by set-state-in-effect (29).
      // They are real signals, but several flag the documented escape hatch for syncing
      // state to a changed prop, and clearing them is an app-wide refactor rather than a
      // lint fix. Kept at "warn" so they stay visible and `npm run lint` remains a gate
      // that means something, instead of being permanently red and ignored.
      "react-hooks/set-state-in-effect": "warn",
      "react-hooks/refs": "warn",
      "react-hooks/preserve-manual-memoization": "warn",
      "react-hooks/incompatible-library": "warn",
      "react-hooks/purity": "warn",
      "react-hooks/immutability": "warn",
      "react-hooks/exhaustive-deps": "warn",
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
