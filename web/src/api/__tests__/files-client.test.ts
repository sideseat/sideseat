import { describe, it, expect, vi } from "vitest";

vi.mock("../api-client", () => ({
  API_BASE_URL: "http://localhost/api/v1",
}));

import { FilesClient } from "../files/client";

describe("FilesClient", () => {
  const client = new FilesClient("http://localhost/api/v1");
  const hash = "a".repeat(64);

  describe("isFileUri", () => {
    it("recognizes URI without MIME", () => {
      expect(client.isFileUri(`#!B64!#::${hash}`)).toBe(true);
    });

    it("recognizes URI with MIME", () => {
      expect(client.isFileUri(`#!B64!#image/png::${hash}`)).toBe(true);
    });

    it("rejects non-file URIs", () => {
      expect(client.isFileUri("data:image/png;base64,abc")).toBe(false);
      expect(client.isFileUri("https://example.com")).toBe(false);
      expect(client.isFileUri("")).toBe(false);
    });

    it("rejects prefix without separator", () => {
      expect(client.isFileUri("#!B64!#noseparator")).toBe(false);
    });

    it("rejects malformed storage hashes", () => {
      expect(client.isFileUri("#!B64!#::abc123")).toBe(false);
      expect(client.isFileUri(`#!B64!#::${"g".repeat(64)}`)).toBe(false);
    });
  });

  describe("extractHash", () => {
    it("extracts hash from URI without MIME", () => {
      expect(client.extractHash(`#!B64!#::${hash}`)).toBe(hash);
    });

    it("extracts hash from URI with MIME", () => {
      expect(client.extractHash(`#!B64!#image/png::${hash}`)).toBe(hash);
    });

    it("returns null for non-file URIs", () => {
      expect(client.extractHash("not-a-uri")).toBeNull();
      expect(client.extractHash("data:image/png;base64,abc")).toBeNull();
    });
  });

  describe("extractMediaType", () => {
    it("extracts MIME from URI with MIME", () => {
      expect(client.extractMediaType(`#!B64!#image/png::${hash}`)).toBe("image/png");
    });

    it("extracts application MIME", () => {
      expect(client.extractMediaType(`#!B64!#application/pdf::${hash}`)).toBe("application/pdf");
    });

    it("returns undefined for URI without MIME", () => {
      expect(client.extractMediaType(`#!B64!#::${hash}`)).toBeUndefined();
    });

    it("returns undefined for non-file URIs", () => {
      expect(client.extractMediaType("not-a-uri")).toBeUndefined();
    });
  });

  describe("resolveUri", () => {
    it("resolves URI with MIME to API URL", () => {
      const url = client.resolveUri("default", `#!B64!#image/jpeg::${hash}`);
      expect(url).toBe(`http://localhost/api/v1/project/default/files/${hash}`);
    });

    it("resolves URI without MIME to API URL", () => {
      const url = client.resolveUri("default", `#!B64!#::${hash}`);
      expect(url).toBe(`http://localhost/api/v1/project/default/files/${hash}`);
    });

    it("returns original data if not a file URI", () => {
      const url = client.resolveUri("default", "https://example.com/img.png");
      expect(url).toBe("https://example.com/img.png");
    });

    it("returns malformed file references unchanged", () => {
      const uri = "#!B64!#image/jpeg::not-a-storage-hash";
      expect(client.resolveUri("default", uri)).toBe(uri);
    });
  });

  describe("getFileUrl", () => {
    it("encodes path components", () => {
      expect(client.getFileUrl("project/name", "hash/value")).toBe(
        "http://localhost/api/v1/project/project%2Fname/files/hash%2Fvalue",
      );
    });
  });
});
