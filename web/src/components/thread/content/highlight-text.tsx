import type React from "react";

export const MAX_SEARCH_LENGTH = 200;

export function highlightText(text: unknown, search: string): React.ReactNode {
  if (!search || typeof text !== "string") return text as React.ReactNode;
  // Limit search length to prevent "Regular expression too large" error
  if (search.length > MAX_SEARCH_LENGTH) return text;
  const escaped = search.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const parts = text.split(new RegExp(`(${escaped})`, "gi"));
  if (parts.length === 1) return text;
  return parts.map((part, i) =>
    part.toLowerCase() === search.toLowerCase() ? (
      <mark key={i} className="bg-yellow-300 dark:bg-yellow-500 text-inherit rounded-sm px-0.5">
        {part}
      </mark>
    ) : (
      part
    ),
  );
}
