/**
 * Files API Client
 *
 * Client for retrieving files stored with content-addressed storage.
 * Files are identified by their SHA-256 hash.
 */

import { API_BASE_URL } from "../api-client";

/** URI prefix for file references */
export const FILE_URI_PREFIX = "#!B64!#";

/**
 * FilesClient provides methods for working with stored files.
 *
 * Files are stored with content-addressed storage using SHA-256 hashes.
 * The #!B64!# prefix is used to reference files in message content.
 */
export class FilesClient {
  private baseUrl: string;

  constructor(baseUrl: string = API_BASE_URL) {
    this.baseUrl = baseUrl;
  }

  /**
   * Check if a data string is a sideseat file reference.
   * Format: `#!B64!#[mime/type]::hash`
   */
  isFileUri(data: string): boolean {
    return data.startsWith(FILE_URI_PREFIX) && data.includes("::");
  }

  /**
   * Extract file hash from a `#!B64!#[mime]::hash` URI.
   * Returns null if the URI is not a valid file reference.
   */
  extractHash(uri: string): string | null {
    if (!this.isFileUri(uri)) {
      return null;
    }
    const rest = uri.slice(FILE_URI_PREFIX.length);
    const sepIdx = rest.indexOf("::");
    if (sepIdx === -1) return null;
    return rest.slice(sepIdx + 2);
  }

  /**
   * Extract MIME type from a `#!B64!#mime/type::hash` URI.
   * Returns undefined if the URI has no embedded MIME type.
   */
  extractMediaType(uri: string): string | undefined {
    if (!this.isFileUri(uri)) {
      return undefined;
    }
    const rest = uri.slice(FILE_URI_PREFIX.length);
    const sepIdx = rest.indexOf("::");
    if (sepIdx === -1) return undefined;
    const mimePart = rest.slice(0, sepIdx);
    return mimePart.length > 0 ? mimePart : undefined;
  }

  /**
   * Build the API URL for a file
   */
  getFileUrl(projectId: string, hash: string): string {
    return `${this.baseUrl}/project/${projectId}/files/${hash}`;
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
    // Check if data is a #!B64!# URI regardless of declared source
    // This handles cases where source is still "base64" but data was replaced
    if (this.isFileUri(data)) {
      return this.resolveUri(projectId, data);
    }

    switch (source) {
      case "url":
        return data;
      case "base64": {
        // Construct data URL from base64
        const mime = mediaType || "application/octet-stream";
        return `data:${mime};base64,${data}`;
      }
      case "file":
        // Should have been caught by isFileUri check above
        return this.resolveUri(projectId, data);
      default:
        // Unknown source, return data as-is
        return data;
    }
  }
}
