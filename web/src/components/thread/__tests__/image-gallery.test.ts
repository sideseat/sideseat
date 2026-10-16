import { describe, expect, it } from "vitest";

import type { FilesClient } from "@/api/files/client";
import type { Block } from "@/api/otel/types";
import { extractMediaFromBlocks } from "../image-gallery-context";

const PNG_BASE64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk";
const STAND_IN = "Base64 string (218624 characters)";
const files = {} as FilesClient;

function block(content: unknown): Block {
  return { content } as unknown as Block;
}

describe("extractMediaFromBlocks", () => {
  it("leaves out an attachment the producer withheld, as a block and inside a tool result", () => {
    const withheld = {
      type: "image",
      media_type: "image/jpeg",
      source: "withheld",
      data: STAND_IN,
    };
    const media = extractMediaFromBlocks(
      [
        block(withheld),
        block({
          type: "document",
          media_type: "application/pdf",
          source: "withheld",
          data: STAND_IN,
        }),
        block({ type: "tool_result", content: [withheld] }),
      ],
      files,
    );
    expect(media).toEqual([]);
  });

  it("keeps an attachment whose bytes it holds", () => {
    const media = extractMediaFromBlocks(
      [block({ type: "image", media_type: "image/png", source: "base64", data: PNG_BASE64 })],
      files,
    );
    expect(media.map((m) => m.src)).toEqual([`data:image/png;base64,${PNG_BASE64}`]);
  });
});
