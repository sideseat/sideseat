import React from "react";
import ReactDOM from "react-dom/client";
import { RouterProvider, createBrowserRouter } from "react-router";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { AuthProvider } from "@/auth/context";
import { AuthGuard } from "@/auth/guard";
import App from "./app";
import HomePage from "./pages/home";
import DashboardPage from "./pages/dashboard";
import AuthPage from "./pages/auth";
import NotFoundPage from "./pages/not-found";
import "./index.css";

const router = createBrowserRouter(
  [
    {
      path: "/auth",
      element: <AuthPage />,
    },
    {
      path: "/",
      element: (
        <AuthGuard>
          <App />
        </AuthGuard>
      ),
      children: [
        { index: true, element: <HomePage /> },
        { path: "dashboard", element: <DashboardPage /> },
        { path: "*", element: <NotFoundPage /> },
      ],
    },
  ],
  {
    basename: "/ui",
  },
);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider defaultTheme="system" storageKey="theme">
      <AuthProvider>
        <RouterProvider router={router} />
        <Toaster />
      </AuthProvider>
    </ThemeProvider>
  </React.StrictMode>,
);

// Register service worker
if ("serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker
      .register("/ui/sw.js", { scope: "/ui/" })
      .then((registration) => {
        console.log("Service Worker registered:", registration);
      })
      .catch((error) => {
        console.error("Service Worker registration failed:", error);
      });
  });
}
