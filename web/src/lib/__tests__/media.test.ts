import { describe, it, expect } from "vitest";
import { findEmbeddedMedia, getDataUrlByteLength, holdsMediaContent, inferSource } from "../media";

const hash = "a".repeat(64);

describe("inferSource", () => {
  it("detects file ref without MIME", () => {
    expect(inferSource(`#!B64!#::${hash}`)).toBe("file");
  });

  it("detects file ref with MIME", () => {
    expect(inferSource(`#!B64!#image/png::${hash}`)).toBe("file");
  });

  it("detects data URL", () => {
    expect(inferSource("data:image/png;base64,abc")).toBe("url");
  });

  it("defaults to base64", () => {
    expect(inferSource("iVBORw0KGgo")).toBe("base64");
  });

  it("does not match prefix without separator", () => {
    expect(inferSource("#!B64!#noseparator")).toBe("base64");
  });

  it("does not treat malformed hashes as stored files", () => {
    expect(inferSource("#!B64!#image/png::hash")).toBe("base64");
  });
});

describe("getDataUrlByteLength", () => {
  it("accounts for base64 padding", () => {
    expect(getDataUrlByteLength("data:text/plain;base64,TQ==")).toBe(1);
    expect(getDataUrlByteLength("data:text/plain;base64,TWE=")).toBe(2);
    expect(getDataUrlByteLength("data:text/plain;base64,TWFu")).toBe(3);
  });

  it("rejects non-base64 data URLs", () => {
    expect(getDataUrlByteLength("data:text/plain,hello")).toBeNull();
    expect(getDataUrlByteLength("https://example.com/file")).toBeNull();
  });
});

describe("findEmbeddedMedia", () => {
  it("detects file ref with embedded MIME and infers type", () => {
    const data = `#!B64!#image/jpeg::${hash}`;
    const result = findEmbeddedMedia({ data });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("image");
    expect(result!.mediaType).toBe("image/jpeg");
    expect(result!.data).toBe(data);
  });

  it("detects file ref without MIME as generic file", () => {
    const result = findEmbeddedMedia({ data: `#!B64!#::${hash}` });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("file");
    expect(result!.mediaType).toBeUndefined();
  });

  it("detects PDF MIME as document", () => {
    const result = findEmbeddedMedia({ data: `#!B64!#application/pdf::${hash}` });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("document");
    expect(result!.mediaType).toBe("application/pdf");
  });

  it("detects audio MIME", () => {
    const result = findEmbeddedMedia({ data: `#!B64!#audio/mpeg::${hash}` });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("audio");
    expect(result!.mediaType).toBe("audio/mpeg");
  });

  it("detects video MIME", () => {
    const result = findEmbeddedMedia({ data: `#!B64!#video/mp4::${hash}` });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("video");
    expect(result!.mediaType).toBe("video/mp4");
  });

  it("prefers sibling media_type over embedded MIME", () => {
    const result = findEmbeddedMedia({
      data: `#!B64!#image/png::${hash}`,
      media_type: "image/webp",
    });
    expect(result).not.toBeNull();
    // Sibling field wins because it's found first
    expect(result!.mediaType).toBe("image/webp");
  });

  it("uses embedded MIME when no sibling media_type", () => {
    const result = findEmbeddedMedia({
      data: `#!B64!#image/jpeg::${hash}`,
      name: "photo.jpg",
    });
    expect(result).not.toBeNull();
    expect(result!.mediaType).toBe("image/jpeg");
    expect(result!.name).toBe("photo.jpg");
  });

  it("returns null for objects without media", () => {
    const result = findEmbeddedMedia({ key: "value", count: "42" });
    expect(result).toBeNull();
  });
});

describe("holdsMediaContent", () => {
  it("is false for bytes the producer withheld, whatever their stand-in looks like", () => {
    expect(holdsMediaContent("withheld", "Base64 string (218624 characters)")).toBe(false);
    expect(holdsMediaContent("withheld", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ")).toBe(
      false,
    );
  });

  it("is false for a placeholder and true for content", () => {
    expect(holdsMediaContent("base64", "<replaced>")).toBe(false);
    expect(holdsMediaContent("base64", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ")).toBe(true);
    expect(holdsMediaContent("url", "https://example.com/a.png")).toBe(true);
  });
});

describe("findEmbeddedMedia for an attachment the producer withheld", () => {
  it("keeps the declared source, even for a stand-in that looks like base64", () => {
    const standIn = "A".repeat(120);
    const found = findEmbeddedMedia({
      type: "image",
      media_type: "image/jpeg",
      source: "withheld",
      data: standIn,
      detail: "auto",
    });
    expect(found).toEqual({
      data: standIn,
      mediaType: "image/jpeg",
      name: undefined,
      type: "image",
      rest: { detail: "auto" },
      source: "withheld",
    });
  });

  it("names a withheld document by its filename", () => {
    const found = findEmbeddedMedia({
      type: "document",
      media_type: "application/pdf",
      name: "task.pdf",
      source: "withheld",
      data: "Base64 string (17612 characters)",
    });
    expect(found?.source).toBe("withheld");
    expect(found?.name).toBe("task.pdf");
    expect(found?.type).toBe("document");
  });
});
