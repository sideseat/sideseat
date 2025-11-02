import { Link, Outlet } from "react-router";

export default function App() {
  return (
    <div>
      <nav style={{ padding: "1rem", borderBottom: "1px solid #ccc" }}>
        <Link to="/" style={{ marginRight: "1rem" }}>
          Home
        </Link>
        <Link to="/dashboard" style={{ marginRight: "1rem" }}>
          Dashboard
        </Link>
        <Link to="/traces" style={{ marginRight: "1rem" }}>
          Traces
        </Link>
        <Link to="/prompts" style={{ marginRight: "1rem" }}>
          Prompts
        </Link>
        <Link to="/proxy" style={{ marginRight: "1rem" }}>
          Proxy
        </Link>
        <Link to="/mcp" style={{ marginRight: "1rem" }}>
          MCP
        </Link>
        <Link to="/a2a" style={{ marginRight: "1rem" }}>
          A2A
        </Link>
      </nav>

      <main style={{ padding: "2rem" }}>
        <Outlet />
      </main>
    </div>
  );
}
