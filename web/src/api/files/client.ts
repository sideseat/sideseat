import { API_BASE_URL } from "../api-client";
import { FILE_URI_PREFIX, parseFileUri } from "../../lib/file-uri";

export { FILE_URI_PREFIX };

/**
 * FilesClient provides methods for working with stored files.
 *
 * Files are stored with content-addressed storage using BLAKE3 hashes.
 * The #!B64!# prefix is used to reference files in message content.
 */
export class FilesClient {
  private baseUrl: string;

  constructor(baseUrl: string = API_BASE_URL) {
    this.baseUrl = baseUrl;
  }

  /**
   * Check if a data string is a sideseat file reference.
   * Format: `#!B64!#[mime/type]::<64-character BLAKE3 hash>`
   */
  isFileUri(data: string): boolean {
    return parseFileUri(data) !== null;
  }

  /**
   * Extract file hash from a `#!B64!#[mime]::hash` URI.
   * Returns null if the URI is not a valid file reference.
   */
  extractHash(uri: string): string | null {
    return parseFileUri(uri)?.hash ?? null;
  }

  /**
   * Extract MIME type from a `#!B64!#mime/type::hash` URI.
   * Returns undefined if the URI has no embedded MIME type.
   */
  extractMediaType(uri: string): string | undefined {
    return parseFileUri(uri)?.mediaType;
  }

  /**
   * Build the API URL for a file
   */
  getFileUrl(projectId: string, hash: string): string {
    return `${this.baseUrl}/project/${encodeURIComponent(projectId)}/files/${encodeURIComponent(hash)}`;
  }

  /**
   * Resolve a #!B64!# URI to an API URL
   * Returns the original data if it's not a file URI
   */
  resolveUri(projectId: string, data: string): string {
    const hash = this.extractHash(data);
    if (hash) {
      return this.getFileUrl(projectId, hash);
    }
    return data;
  }

  /**
   * Resolve content block source to a usable URL
   *
   * Handles three source types:
   * - "url": data is already a URL, return as-is
   * - "base64": data is base64, construct data URL
   * - "file": data is #!B64!# URI, resolve to API URL
   *
   * Also handles cases where source is "base64" but data contains #!B64!# URI
   * (legacy/transition format where we didn't update source type)
   */
  resolveContentBlockSource(
    projectId: string,
    source: string,
    data: string,
    mediaType?: string,
  ): string {
    if (this.isFileUri(data)) {
      return this.resolveUri(projectId, data);
    }

    switch (source) {
      case "url":
        return data;
      case "base64": {
        const mime = mediaType || "application/octet-stream";
        return `data:${mime};base64,${data}`;
      }
      case "file":
        return this.resolveUri(projectId, data);
      default:
        return data;
    }
  }
}
