import { describe, it, expect, vi } from "vitest";

vi.mock("../api-client", () => ({
  API_BASE_URL: "http://localhost/api/v1",
}));

import { FilesClient } from "../files/client";

describe("FilesClient", () => {
  const client = new FilesClient("http://localhost/api/v1");

  describe("isFileUri", () => {
    it("recognizes URI without MIME", () => {
      expect(client.isFileUri("#!B64!#::abc123")).toBe(true);
    });

    it("recognizes URI with MIME", () => {
      expect(client.isFileUri("#!B64!#image/png::abc123")).toBe(true);
    });

    it("rejects non-file URIs", () => {
      expect(client.isFileUri("data:image/png;base64,abc")).toBe(false);
      expect(client.isFileUri("https://example.com")).toBe(false);
      expect(client.isFileUri("")).toBe(false);
    });

    it("rejects prefix without separator", () => {
      expect(client.isFileUri("#!B64!#noseparator")).toBe(false);
    });
  });

  describe("extractHash", () => {
    it("extracts hash from URI without MIME", () => {
      expect(client.extractHash("#!B64!#::abc123")).toBe("abc123");
    });

    it("extracts hash from URI with MIME", () => {
      expect(client.extractHash("#!B64!#image/png::abc123")).toBe("abc123");
    });

    it("returns null for non-file URIs", () => {
      expect(client.extractHash("not-a-uri")).toBeNull();
      expect(client.extractHash("data:image/png;base64,abc")).toBeNull();
    });
  });

  describe("extractMediaType", () => {
    it("extracts MIME from URI with MIME", () => {
      expect(client.extractMediaType("#!B64!#image/png::abc123")).toBe("image/png");
    });

    it("extracts application MIME", () => {
      expect(client.extractMediaType("#!B64!#application/pdf::hash")).toBe("application/pdf");
    });

    it("returns undefined for URI without MIME", () => {
      expect(client.extractMediaType("#!B64!#::abc123")).toBeUndefined();
    });

    it("returns undefined for non-file URIs", () => {
      expect(client.extractMediaType("not-a-uri")).toBeUndefined();
    });
  });

  describe("resolveUri", () => {
    it("resolves URI with MIME to API URL", () => {
      const url = client.resolveUri("default", "#!B64!#image/jpeg::hash123");
      expect(url).toBe("http://localhost/api/v1/project/default/files/hash123");
    });

    it("resolves URI without MIME to API URL", () => {
      const url = client.resolveUri("default", "#!B64!#::hash123");
      expect(url).toBe("http://localhost/api/v1/project/default/files/hash123");
    });

    it("returns original data if not a file URI", () => {
      const url = client.resolveUri("default", "https://example.com/img.png");
      expect(url).toBe("https://example.com/img.png");
    });
  });
});
