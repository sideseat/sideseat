import React from "react";
import ReactDOM from "react-dom/client";
import { RouterProvider, createBrowserRouter } from "react-router";
import App from "./app";
import HomePage from "./pages/home";
import DashboardPage from "./pages/dashboard";
import TracesPage from "./pages/traces";
import PromptsPage from "./pages/prompts";
import ProxyPage from "./pages/proxy";
import McpDebuggerPage from "./pages/mcp-debugger";
import A2aDebuggerPage from "./pages/a2a-debugger";
import "./app.css";

const router = createBrowserRouter([
  {
    path: "/",
    element: <App />,
    children: [
      { index: true, element: <HomePage /> },
      { path: "dashboard", element: <DashboardPage /> },
      { path: "traces", element: <TracesPage /> },
      { path: "prompts", element: <PromptsPage /> },
      { path: "proxy", element: <ProxyPage /> },
      { path: "mcp", element: <McpDebuggerPage /> },
      { path: "a2a", element: <A2aDebuggerPage /> },
    ],
  },
]);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
);
