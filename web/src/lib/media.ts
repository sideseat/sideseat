/**
 * Media detection utilities for embedded content in tool results.
 */

/** File reference marker used by SideSeat */
const FILE_REF_MARKER = "#!B64!#";

/** Field names that typically contain media/MIME types */
const MEDIA_TYPE_FIELDS = new Set([
  "mediaType",
  "media_type",
  "mimeType",
  "mime_type",
  "contentType",
  "content_type",
  "type",
]);

/** Field names that typically contain file/content names */
const NAME_FIELDS = new Set(["name", "fileName", "file_name", "filename"]);

/** Valid media type values */
const MEDIA_TYPES = new Set(["image", "audio", "video", "document", "file"]);

/** Media content types */
export type MediaType = "image" | "audio" | "video" | "document" | "file";

/** Data source types */
type SourceType = "file" | "url" | "base64";

/** Detected embedded media in an object */
export interface EmbeddedMedia {
  /** The data value (with #!B64!# prefix or base64) */
  data: string;
  /** Detected MIME type */
  mediaType: string | undefined;
  /** Detected file name */
  name: string | undefined;
  /** Inferred content type (image, audio, video, document, file) */
  type: MediaType;
  /** Remaining fields after extracting media-related ones */
  rest: Record<string, unknown>;
}

/**
 * Infer content type from MIME type.
 */
function inferTypeFromMime(mimeType: string | undefined): MediaType {
  if (!mimeType) return "file";
  const lower = mimeType.toLowerCase();
  if (lower.startsWith("image/")) return "image";
  if (lower.startsWith("audio/")) return "audio";
  if (lower.startsWith("video/")) return "video";
  if (lower.startsWith("application/pdf") || lower.includes("document")) return "document";
  return "file";
}

/**
 * Infer the source type from data format.
 */
export function inferSource(data: string): SourceType {
  if (data.startsWith(FILE_REF_MARKER) && data.includes("::")) return "file";
  if (data.startsWith("data:")) return "url";
  return "base64";
}

/**
 * Extract MIME type embedded in a file reference URI.
 * Returns undefined if no MIME is present or not a file ref.
 */
function extractMimeFromFileRef(data: string): string | undefined {
  if (!data.startsWith(FILE_REF_MARKER)) return undefined;
  const rest = data.slice(FILE_REF_MARKER.length);
  const sepIdx = rest.indexOf("::");
  if (sepIdx === -1) return undefined;
  const mimePart = rest.slice(0, sepIdx);
  return mimePart.length > 0 ? mimePart : undefined;
}

/**
 * Check if a string looks like base64 data.
 * Must start with alphanumeric and be reasonably long.
 */
function looksLikeBase64(str: string): boolean {
  if (str.length < 100) return false;
  // Must start with base64 char (not whitespace)
  const first = str.charCodeAt(0);
  if (
    !(first >= 65 && first <= 90) && // A-Z
    !(first >= 97 && first <= 122) && // a-z
    !(first >= 48 && first <= 57) // 0-9
  ) {
    return false;
  }
  // Check for base64 character set
  return /^[A-Za-z0-9+/\s]+=*$/.test(str);
}

/**
 * Scan an object for embedded media content.
 * Looks for #!B64!# markers or base64 data in ANY field.
 * Returns null if no embedded media found.
 */
export function findEmbeddedMedia(obj: Record<string, unknown>): EmbeddedMedia | null {
  let dataKey: string | null = null;
  let data: string | null = null;
  let mediaTypeKey: string | null = null;
  let mediaType: string | undefined;
  let nameKey: string | null = null;
  let name: string | undefined;
  let base64Candidate: { key: string; value: string } | null = null;

  const entries = Object.entries(obj);

  // Single pass to collect all relevant fields and their keys
  for (const [key, value] of entries) {
    if (typeof value !== "string") continue;

    // Priority 1: File reference marker (immediate match)
    if (value.startsWith(FILE_REF_MARKER) && value.includes("::")) {
      dataKey = key;
      data = value;
      // Don't break - continue to find mediaType/name
    }
    // Collect media type with its key
    else if (!mediaType && MEDIA_TYPE_FIELDS.has(key) && value.includes("/")) {
      mediaTypeKey = key;
      mediaType = value;
    }
    // Collect name with its key
    else if (!name && NAME_FIELDS.has(key)) {
      nameKey = key;
      name = value;
    }
    // Track base64 candidate (only if no file ref found yet)
    else if (!dataKey && !base64Candidate && looksLikeBase64(value)) {
      base64Candidate = { key, value };
    }
  }

  // If no file ref found, use base64 candidate (only if we have a media type hint)
  if (!dataKey && base64Candidate && mediaType) {
    dataKey = base64Candidate.key;
    data = base64Candidate.value;
  }

  // No embedded media found
  if (!dataKey || !data) return null;

  // If no sibling media_type field, try extracting MIME from the file URI itself
  if (!mediaType) {
    const embeddedMime = extractMimeFromFileRef(data);
    if (embeddedMime) {
      mediaType = embeddedMime;
    }
  }

  // Build used fields set
  const usedFields = new Set<string>([dataKey]);
  if (mediaTypeKey) usedFields.add(mediaTypeKey);
  if (nameKey) usedFields.add(nameKey);

  // Check if type field should be excluded from rest
  const typeVal = typeof obj.type === "string" ? obj.type.toLowerCase() : null;
  if (typeVal && MEDIA_TYPES.has(typeVal)) {
    usedFields.add("type");
  }

  // Build rest object (single pass)
  const rest: Record<string, unknown> = {};
  for (const [key, value] of entries) {
    if (!usedFields.has(key)) {
      rest[key] = value;
    }
  }

  // Infer type
  const type: MediaType =
    typeVal && MEDIA_TYPES.has(typeVal) ? (typeVal as MediaType) : inferTypeFromMime(mediaType);

  return { data, mediaType, name, type, rest };
}
