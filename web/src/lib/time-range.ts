/**
 * Time Range Utilities
 *
 * Utilities for handling time range selection in the Project Home dashboard.
 */

export type TimeRange = "today" | "24h" | "7d";

export interface TimeRangeResult {
  from: Date;
  to: Date;
}

/**
 * Get the start and end timestamps for a time range.
 *
 * - today: Local midnight today to now
 * - 24h: now - 24 hours to now
 * - 7d: now - 7 days to now
 */
export function getTimeRange(range: TimeRange): TimeRangeResult {
  const now = new Date();
  const to = now;
  let from: Date;

  switch (range) {
    case "today": {
      // Local midnight today (user's timezone)
      from = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 0, 0, 0, 0);
      break;
    }
    case "24h": {
      // 24 hours ago
      from = new Date(now.getTime() - 24 * 60 * 60 * 1000);
      break;
    }
    case "7d": {
      // 7 days ago
      from = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000);
      break;
    }
  }

  return { from, to };
}

/**
 * Get display label for a time range.
 */
export function getTimeRangeLabel(range: TimeRange): string {
  switch (range) {
    case "today":
      return "Today";
    case "24h":
      return "Last 24 Hours";
    case "7d":
      return "Last 7 Days";
  }
}

/**
 * Get short display label for a time range (for toggle buttons).
 */
export function getTimeRangeShortLabel(range: TimeRange): string {
  switch (range) {
    case "today":
      return "Today";
    case "24h":
      return "24h";
    case "7d":
      return "7d";
  }
}

/**
 * Get the user's timezone (e.g., "America/New_York").
 */
function getUserTimezone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

/**
 * Convert time range to ISO strings for API params.
 * Includes the user's timezone for server-side bucketing.
 */
export function getTimeRangeParams(range: TimeRange): {
  from_timestamp: string;
  to_timestamp: string;
  timezone: string;
} {
  const { from, to } = getTimeRange(range);
  return {
    from_timestamp: from.toISOString(),
    to_timestamp: to.toISOString(),
    timezone: getUserTimezone(),
  };
}

/**
 * Get the localStorage key for storing time range preference.
 */
export function getTimeRangeStorageKey(projectId: string): string {
  return `sideseat_timerange_${projectId}`;
}

/**
 * Load time range from localStorage, defaulting to "24h".
 */
export function loadTimeRange(projectId: string): TimeRange {
  if (typeof localStorage === "undefined") {
    return "24h";
  }
  const stored = localStorage.getItem(getTimeRangeStorageKey(projectId));
  if (stored === "today" || stored === "24h" || stored === "7d") {
    return stored;
  }
  return "24h";
}

/**
 * Save time range to localStorage.
 */
export function saveTimeRange(projectId: string, range: TimeRange): void {
  if (typeof localStorage === "undefined") {
    return;
  }
  localStorage.setItem(getTimeRangeStorageKey(projectId), range);
}

export const TIME_RANGE_OPTIONS: TimeRange[] = ["today", "24h", "7d"];

/**
 * Format a bucket timestamp for chart display.
 * Returns both a short label (for axis) and long label (for tooltip).
 */
export function formatBucketLabel(
  bucket: string,
  timeRange: TimeRange,
): { short: string; long: string } {
  const date = new Date(bucket);
  if (Number.isNaN(date.getTime())) {
    return { short: bucket, long: bucket };
  }

  if (timeRange === "7d") {
    return {
      short: date.toLocaleDateString(undefined, { month: "short", day: "numeric" }),
      long: date.toLocaleDateString(undefined, {
        weekday: "short",
        month: "short",
        day: "numeric",
      }),
    };
  }

  // For 24h/today, add relative day context to tooltip
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const bucketDay = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  const dayDiff = Math.floor((today.getTime() - bucketDay.getTime()) / (1000 * 60 * 60 * 24));

  let dayLabel = "";
  if (dayDiff === 0) {
    dayLabel = "Today";
  } else if (dayDiff === 1) {
    dayLabel = "Yesterday";
  } else {
    dayLabel = date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }

  return {
    short: date.toLocaleTimeString(undefined, { hour: "numeric" }),
    long: `${dayLabel}, ${date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`,
  };
}
