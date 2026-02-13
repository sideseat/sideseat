/**
 * Sanitize ID for use in export formats.
 * Replaces invalid chars with underscore, ensures non-empty.
 * Does NOT add prefix - callers should add format-specific prefixes if needed.
 *
 * @param id - The raw ID to sanitize
 * @returns Sanitized ID (never empty)
 */
export function sanitizeId(id: string): string {
  const sanitized = id.replace(/[^a-zA-Z0-9_-]/g, "_");
  return sanitized || "node"; // Fallback for empty IDs
}

/**
 * Escape XML special characters.
 * Order matters: & must be replaced first.
 *
 * @param str - The string to escape
 * @returns XML-safe string
 */
export function escapeXml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

/**
 * Normalize coordinates to ensure all are positive with padding.
 * Rounds to integers for cleaner output.
 * Filters out items with missing/invalid positions (undefined, NaN, Infinity).
 *
 * @param items - Array of items with optional position property
 * @param padding - Padding from origin in pixels (default: 50)
 * @returns Filtered items with normalized, rounded positions
 */
export function normalizeCoordinates<T extends { position?: { x: number; y: number } }>(
  items: T[],
  padding = 50,
): (T & { position: { x: number; y: number } })[] {
  // Filter to items with valid, finite positions (excludes undefined, NaN, Infinity)
  const validItems = items.filter(
    (item): item is T & { position: { x: number; y: number } } =>
      item.position != null && Number.isFinite(item.position.x) && Number.isFinite(item.position.y),
  );

  if (validItems.length === 0) return [];

  const minX = Math.min(...validItems.map((n) => n.position.x));
  const minY = Math.min(...validItems.map((n) => n.position.y));

  return validItems.map((item) => ({
    ...item,
    position: {
      x: Math.round(item.position.x - minX + padding),
      y: Math.round(item.position.y - minY + padding),
    },
  }));
}
