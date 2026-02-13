import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function sortProjectsWithDefaultFirst<T extends { id: string; name: string }>(
  projects: T[],
): T[] {
  return [...projects].sort((a, b) => {
    if (a.id === "default") return -1;
    if (b.id === "default") return 1;
    return a.name.localeCompare(b.name);
  });
}

/**
 * Recursively parses JSON strings within an object.
 * If a string value starts with '{' or '[', attempts to parse it as JSON.
 */
export function deepParseJsonStrings<T>(value: T): T {
  if (value === null || value === undefined) {
    return value;
  }

  if (typeof value === "string") {
    const trimmed = value.trim();
    if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
      try {
        const parsed = JSON.parse(trimmed);
        return deepParseJsonStrings(parsed);
      } catch {
        return value;
      }
    }
    return value;
  }

  if (Array.isArray(value)) {
    return value.map(deepParseJsonStrings) as T;
  }

  if (typeof value === "object") {
    const result: Record<string, unknown> = {};
    for (const key in value) {
      if (Object.prototype.hasOwnProperty.call(value, key)) {
        result[key] = deepParseJsonStrings((value as Record<string, unknown>)[key]);
      }
    }
    return result as T;
  }

  return value;
}

/**
 * Download content as a file.
 * Supports string content (converted to blob), data URLs, or existing blobs.
 *
 * @param content - String content, data URL (starts with "data:"), or Blob
 * @param filename - Name for the downloaded file
 * @param mimeType - MIME type (required for string content, ignored for data URLs/blobs)
 */
export function downloadFile(content: string | Blob, filename: string, mimeType?: string): void {
  let url: string;
  let shouldRevoke = true;

  if (content instanceof Blob) {
    url = URL.createObjectURL(content);
  } else if (content.startsWith("data:")) {
    // Data URL - use directly, no need to revoke
    url = content;
    shouldRevoke = false;
  } else {
    // String content - convert to blob
    if (!mimeType) {
      throw new Error("mimeType is required for string content");
    }
    url = URL.createObjectURL(new Blob([content], { type: mimeType }));
  }

  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();

  if (shouldRevoke) {
    URL.revokeObjectURL(url);
  }
}

/**
 * Download a file from a remote URL.
 * Fetches the URL with credentials and triggers a download.
 * Falls back to opening in a new tab if fetch fails.
 *
 * @param url - Remote URL to download from
 * @param filename - Name for the downloaded file
 */
export async function downloadFromUrl(url: string, filename: string): Promise<void> {
  try {
    const response = await fetch(url, { credentials: "include" });
    const blob = await response.blob();
    const blobUrl = URL.createObjectURL(blob);

    const link = document.createElement("a");
    link.href = blobUrl;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);

    URL.revokeObjectURL(blobUrl);
  } catch {
    // Fallback: open in new tab
    window.open(url, "_blank");
  }
}

/** Format file size from bytes to human-readable string */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Map MIME type to file extension */
export function getExtensionForMediaType(mediaType?: string): string {
  if (!mediaType) return "bin";
  const subtype = mediaType.split("/")[1];
  if (!subtype) return "bin";

  const mappings: Record<string, string> = {
    jpeg: "jpg",
    "svg+xml": "svg",
    mpeg: "mp3",
    "x-wav": "wav",
    "x-m4a": "m4a",
    quicktime: "mov",
    "x-msvideo": "avi",
    "x-matroska": "mkv",
    plain: "txt",
    javascript: "js",
    typescript: "ts",
    "x-python": "py",
    "x-ruby": "rb",
    "x-sh": "sh",
    "x-yaml": "yaml",
    "vnd.openxmlformats-officedocument.wordprocessingml.document": "docx",
    "vnd.openxmlformats-officedocument.spreadsheetml.sheet": "xlsx",
    "vnd.openxmlformats-officedocument.presentationml.presentation": "pptx",
    msword: "doc",
    "vnd.ms-excel": "xls",
    "vnd.ms-powerpoint": "ppt",
  };

  return mappings[subtype] || subtype;
}

/** Check if data is a placeholder value (not actual content) */
export function isPlaceholderData(data: string): boolean {
  if (!data || data.length === 0) return true;
  const trimmed = data.trim();
  if (trimmed === "<replaced>" || trimmed === "<binary>") return true;
  if (trimmed === "[binary]" || trimmed === "[replaced]") return true;
  if (trimmed === "...") return true;
  // Generic angle bracket placeholders like <...>, <truncated>, <omitted>
  if (/^<[^>]{1,30}>$/.test(trimmed)) return true;
  // Generic square bracket placeholders
  if (/^\[[^\]]{1,30}\]$/.test(trimmed)) return true;
  // Too short to be valid base64 content (less than ~10 bytes encoded)
  // Allow URLs and file references regardless of length
  if (trimmed.length < 16 && !/^(https?:\/\/|#!B64!#)/.test(trimmed)) return true;
  return false;
}

/** Get short type label from media type */
export function getMediaTypeLabel(mediaType?: string, fallback = "FILE"): string {
  if (!mediaType) return fallback;
  const subtype = mediaType.split("/")[1];
  if (!subtype) return fallback;
  if (subtype === "jpeg" || subtype === "jpg") return "JPEG";
  if (subtype === "png") return "PNG";
  if (subtype === "gif") return "GIF";
  if (subtype === "webp") return "WEBP";
  if (subtype === "svg+xml") return "SVG";
  if (subtype === "pdf") return "PDF";
  return subtype.toUpperCase().slice(0, 4);
}
