// API client for backend communication

const API_BASE_URL = import.meta.env.PROD ? "/api" : "http://localhost:3000/api";

export async function fetchAPI<T>(endpoint: string, options?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE_URL}${endpoint}`, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      ...options?.headers,
    },
  });

  if (!response.ok) {
    throw new Error(`API error: ${response.statusText}`);
  }

  return response.json();
}

// TODO: Add specific API methods
// export async function getTraces() { ... }
// export async function getPrompts() { ... }
