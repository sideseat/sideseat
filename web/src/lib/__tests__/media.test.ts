import { describe, it, expect } from "vitest";
import { inferSource, findEmbeddedMedia } from "../media";

describe("inferSource", () => {
  it("detects file ref without MIME", () => {
    expect(inferSource("#!B64!#::hash")).toBe("file");
  });

  it("detects file ref with MIME", () => {
    expect(inferSource("#!B64!#image/png::hash")).toBe("file");
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
});

describe("findEmbeddedMedia", () => {
  it("detects file ref with embedded MIME and infers type", () => {
    const result = findEmbeddedMedia({ data: "#!B64!#image/jpeg::h" });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("image");
    expect(result!.mediaType).toBe("image/jpeg");
    expect(result!.data).toBe("#!B64!#image/jpeg::h");
  });

  it("detects file ref without MIME as generic file", () => {
    const result = findEmbeddedMedia({ data: "#!B64!#::h" });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("file");
    expect(result!.mediaType).toBeUndefined();
  });

  it("detects PDF MIME as document", () => {
    const result = findEmbeddedMedia({ data: "#!B64!#application/pdf::h" });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("document");
    expect(result!.mediaType).toBe("application/pdf");
  });

  it("detects audio MIME", () => {
    const result = findEmbeddedMedia({ data: "#!B64!#audio/mpeg::h" });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("audio");
    expect(result!.mediaType).toBe("audio/mpeg");
  });

  it("detects video MIME", () => {
    const result = findEmbeddedMedia({ data: "#!B64!#video/mp4::h" });
    expect(result).not.toBeNull();
    expect(result!.type).toBe("video");
    expect(result!.mediaType).toBe("video/mp4");
  });

  it("prefers sibling media_type over embedded MIME", () => {
    const result = findEmbeddedMedia({
      data: "#!B64!#image/png::h",
      media_type: "image/webp",
    });
    expect(result).not.toBeNull();
    // Sibling field wins because it's found first
    expect(result!.mediaType).toBe("image/webp");
  });

  it("uses embedded MIME when no sibling media_type", () => {
    const result = findEmbeddedMedia({
      data: "#!B64!#image/jpeg::h",
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
