import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { ApiClient, ApiError, AuthError, NetworkError } from "../api-client";

describe("ApiClient", () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let client: ApiClient;

  beforeEach(() => {
    mockFetch = vi.fn();
    client = new ApiClient("/api/v1", mockFetch as unknown as typeof fetch);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("get()", () => {
    it("makes GET request with correct URL", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ data: "test" })),
      });

      await client.get("/traces");

      expect(mockFetch).toHaveBeenCalledWith(
        "/api/v1/traces",
        expect.objectContaining({
          credentials: "include",
          headers: expect.objectContaining({ "Content-Type": "application/json" }),
        }),
      );
    });

    it("serializes query params correctly", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ data: [] })),
      });

      await client.get("/traces", { page: 1, limit: 10 });

      expect(mockFetch).toHaveBeenCalledWith("/api/v1/traces?page=1&limit=10", expect.any(Object));
    });

    it("handles array params", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ data: [] })),
      });

      await client.get("/traces", { environment: ["prod", "staging"] });

      expect(mockFetch).toHaveBeenCalledWith(
        "/api/v1/traces?environment=prod&environment=staging",
        expect.any(Object),
      );
    });

    it("JSON stringifies filters param", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ data: [] })),
      });

      const filters = [{ type: "string", column: "trace_id", operator: "=", value: "abc" }];
      await client.get("/traces", { filters });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringContaining("filters=" + encodeURIComponent(JSON.stringify(filters))),
        expect.any(Object),
      );
    });

    it("omits undefined/null params", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ data: [] })),
      });

      await client.get("/traces", { page: 1, session_id: undefined, user_id: null });

      expect(mockFetch).toHaveBeenCalledWith("/api/v1/traces?page=1", expect.any(Object));
    });
  });

  describe("error handling", () => {
    it("throws AuthError on 401", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 401,
        statusText: "Unauthorized",
      });

      await expect(client.get("/traces")).rejects.toThrow(AuthError);
    });

    it("dispatches auth:required event on 401", async () => {
      const eventSpy = vi.fn();
      window.addEventListener("auth:required", eventSpy);

      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 401,
        statusText: "Unauthorized",
      });

      await expect(client.get("/traces")).rejects.toThrow();
      expect(eventSpy).toHaveBeenCalled();

      window.removeEventListener("auth:required", eventSpy);
    });

    it("throws ApiError with parsed body on 4xx/5xx", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 400,
        statusText: "Bad Request",
        json: () =>
          Promise.resolve({
            error: "bad_request",
            code: "INVALID_PAGE",
            message: "Page must be >= 1",
          }),
      });

      try {
        await client.get("/traces");
        expect.fail("Should have thrown");
      } catch (e) {
        expect(e).toBeInstanceOf(ApiError);
        expect((e as ApiError).code).toBe("INVALID_PAGE");
        expect((e as ApiError).message).toBe("Page must be >= 1");
      }
    });

    it("throws NetworkError when fetch fails", async () => {
      mockFetch.mockRejectedValueOnce(new TypeError("Failed to fetch"));

      await expect(client.get("/traces")).rejects.toThrow(NetworkError);
    });

    it("detects offline state", async () => {
      const originalOnLine = navigator.onLine;
      Object.defineProperty(navigator, "onLine", { value: false, writable: true });
      mockFetch.mockRejectedValueOnce(new TypeError("Failed to fetch"));

      try {
        await client.get("/traces");
      } catch (e) {
        expect((e as NetworkError).isOffline).toBe(true);
      }

      Object.defineProperty(navigator, "onLine", { value: originalOnLine, writable: true });
    });
  });

  describe("timeout", () => {
    it("aborts request after timeout", async () => {
      // Use a very short timeout and a delayed mock fetch
      const shortTimeoutClient = new ApiClient("/api/v1", mockFetch as unknown as typeof fetch);
      mockFetch.mockImplementationOnce(
        () =>
          new Promise((_, reject) => {
            setTimeout(() => reject(new DOMException("Aborted", "AbortError")), 100);
          }),
      );

      // Expect the request to be aborted
      await expect(shortTimeoutClient.get("/traces")).rejects.toThrow("Request timed out");
    }, 10000);
  });

  describe("checkHealth()", () => {
    it("calls /health endpoint at root", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ status: "ok" })),
      });

      await client.checkHealth();

      expect(mockFetch).toHaveBeenCalledWith("/api/v1/health", expect.any(Object));
    });
  });

  describe("post()", () => {
    it("sends POST with JSON body", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({ success: true })),
      });

      await client.post("/auth/exchange", { token: "abc123" });

      expect(mockFetch).toHaveBeenCalledWith(
        "/api/v1/auth/exchange",
        expect.objectContaining({
          method: "POST",
          body: JSON.stringify({ token: "abc123" }),
        }),
      );
    });
  });

  describe("delete()", () => {
    it("sends DELETE request", async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve(JSON.stringify({})),
      });

      await client.delete("/sessions/123");

      expect(mockFetch).toHaveBeenCalledWith(
        "/api/v1/sessions/123",
        expect.objectContaining({ method: "DELETE" }),
      );
    });
  });

  describe("buildQueryString()", () => {
    it("returns empty string for empty object", () => {
      expect(client.buildQueryString({})).toBe("");
    });

    it("serializes simple values", () => {
      expect(client.buildQueryString({ page: 1, limit: 10 })).toBe("page=1&limit=10");
    });

    it("handles arrays with repeated keys", () => {
      expect(client.buildQueryString({ env: ["a", "b"] })).toBe("env=a&env=b");
    });

    it("JSON stringifies filters", () => {
      const filters = [{ type: "string", column: "id", operator: "=", value: "x" }];
      const result = client.buildQueryString({ filters });
      expect(result).toBe("filters=" + encodeURIComponent(JSON.stringify(filters)));
    });

    it("skips null and undefined values", () => {
      expect(client.buildQueryString({ a: 1, b: null, c: undefined, d: 2 })).toBe("a=1&d=2");
    });
  });
});
