import { useEffect, useState } from "react";

export default function HomePage() {
  const [healthStatus, setHealthStatus] = useState<string>("Checking...");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const checkHealth = async () => {
      try {
        const response = await fetch("/api/v1/health");
        if (response.ok) {
          const text = await response.text();
          setHealthStatus(`✓ API Connected (${text})`);
          setError(null);
        } else {
          setHealthStatus("✗ API Error");
          setError(`Status: ${response.status}`);
        }
      } catch (err) {
        setHealthStatus("✗ Connection Failed");
        setError(err instanceof Error ? err.message : "Unknown error");
      }
    };

    checkHealth();
    // Check health every 30 seconds
    const interval = setInterval(checkHealth, 30000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div>
      <h1>SideSeat - AI Development Toolkit</h1>
      <div
        style={{
          marginTop: "2rem",
          padding: "1rem",
          border: "1px solid #ccc",
          borderRadius: "4px",
        }}
      >
        <h2>System Status</h2>
        <p>
          <strong>Backend Health:</strong>{" "}
          <span style={{ color: healthStatus.startsWith("✓") ? "green" : "red" }}>
            {healthStatus}
          </span>
        </p>
        {error && <p style={{ color: "red", fontSize: "0.9em" }}>Error: {error}</p>}
        <p style={{ fontSize: "0.9em", color: "#666" }}>API Endpoint: /api/v1/health</p>
      </div>
      <div style={{ marginTop: "2rem" }}>
        <h2>Features</h2>
        <ul>
          <li>Dashboard - TODO</li>
          <li>Traces - TODO</li>
          <li>Prompts - TODO</li>
          <li>Proxy - TODO</li>
          <li>MCP Debugger - TODO</li>
          <li>A2A Debugger - TODO</li>
        </ul>
      </div>
    </div>
  );
}
