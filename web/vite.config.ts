import path from "path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [
    react({
      babel: {
        plugins: [["babel-plugin-react-compiler"]],
      },
    }),
    tailwindcss(),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  base: "/ui",
  server: {
    port: 5389,
    proxy: {
      "/api/v1": {
        target: "http://localhost:5388",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 500,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return;

          // Scoped packages first — /@scope/ patterns are unambiguous
          if (
            id.includes("/@xyflow/") ||
            id.includes("/elkjs/") ||
            id.includes("/html-to-image/")
          ) {
            return "vendor-diagram";
          }
          if (id.includes("/@tanstack/react-query")) {
            return "vendor-query";
          }
          if (id.includes("/ag-grid-community/") || id.includes("/ag-grid-react/")) {
            return "vendor-ag-grid";
          }
          // Recharts + its exclusive transitive deps — prevents Rollup from
          // placing shared deps (like clsx) into this chunk
          if (
            id.includes("/recharts/") ||
            id.includes("/es-toolkit/") ||
            id.includes("/eventemitter3/") ||
            id.includes("/decimal.js-light/") ||
            id.includes("/immer/") ||
            id.includes("/reselect/") ||
            id.includes("/react-redux/") ||
            id.includes("/@reduxjs/")
          ) {
            return "vendor-recharts";
          }
          // UI component infrastructure — clsx/cva must be here (not in vendor-recharts)
          // because cn() is used statically in the entry
          if (
            id.includes("/@radix-ui/") ||
            id.includes("/lucide-react/") ||
            id.includes("/clsx/") ||
            id.includes("/class-variance-authority/")
          ) {
            return "vendor-radix";
          }
          // d3-* and victory-vendor together — victory-vendor re-exports d3-*
          if (id.includes("/d3-") || id.includes("/victory-vendor/")) {
            return "vendor-d3";
          }
          // Generic /react/ last — avoids matching @xyflow/react (already captured above)
          if (
            id.includes("/react-dom/") ||
            id.includes("/react/") ||
            id.includes("/react-router/") ||
            id.includes("/react-router-dom/") ||
            id.includes("/scheduler/")
          ) {
            return "vendor-react";
          }
        },
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});
