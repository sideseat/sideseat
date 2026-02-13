/**
 * Format Utilities
 *
 * Shared formatting functions for displaying data in tables and UI.
 */

/**
 * Format an ISO timestamp to a locale-aware date-time string.
 * Uses the user's locale for date/time formatting.
 */
export function formatTimestamp24h(iso: string | null): string {
  if (!iso) return "-";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "-";

  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/**
 * Format duration in milliseconds to a readable string
 * Examples: "123ms", "1.2s", "2m 30.5s"
 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return "-";
  if (ms < 1) return "<1ms";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(2)}s`;
  const minutes = Math.floor(ms / 60000);
  const seconds = ((ms % 60000) / 1000).toFixed(1);
  return `${minutes}m ${seconds}s`;
}

/**
 * Format cost as currency with dynamic precision.
 * Shows enough decimal places to display significant digits without truncation.
 * Examples: "$0.0000025", "$0.0123", "$1.23"
 */
export function formatCost(cost: number): string {
  if (cost === 0) return "$0.00";
  if (cost >= 1) return `$${cost.toFixed(2)}`;

  // Show 3 significant figures for small costs (e.g., $0.0000025)
  const decimals = Math.min(10, Math.max(2, Math.ceil(-Math.log10(cost)) + 2));
  const formatted = cost.toFixed(decimals);
  return `$${formatted.replace(/(\.\d{2,}?)0+$/, "$1")}`;
}

/**
 * Format token count with thousands separator
 * Examples: "1,234", "12,345,678"
 */
export function formatTokens(count: number): string {
  return count.toLocaleString();
}

/**
 * Format a number in compact notation (e.g., 847293 → "847K").
 */
export function formatCompact(n: number): string {
  return new Intl.NumberFormat(undefined, { notation: "compact" }).format(n);
}

/**
 * Format a number as USD currency with fixed 2 decimals (e.g., 7.94 → "$7.94").
 * Use this for dashboard displays where consistent formatting is needed.
 */
export function formatCurrencyFixed(n: number): string {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(n);
}

/**
 * Format a number as a percentage (e.g., 45 → "45%").
 * Input is expected to be a percentage value (0-100), not a decimal.
 */
export function formatPercent(n: number): string {
  return new Intl.NumberFormat(undefined, {
    style: "percent",
    minimumFractionDigits: 0,
    maximumFractionDigits: 1,
  }).format(n / 100);
}

/**
 * Format a relative time (e.g., "2s ago", "5m ago", "1h ago").
 */
export function formatRelativeTime(date: Date | string): string {
  const now = new Date();
  const then = typeof date === "string" ? new Date(date) : date;
  const diffMs = now.getTime() - then.getTime();
  const diffSeconds = Math.floor(diffMs / 1000);

  if (diffSeconds < 60) {
    return `${diffSeconds}s ago`;
  }

  const diffMinutes = Math.floor(diffSeconds / 60);
  if (diffMinutes < 60) {
    return `${diffMinutes}m ago`;
  }

  const diffHours = Math.floor(diffMinutes / 60);
  if (diffHours < 24) {
    return `${diffHours}h ago`;
  }

  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}d ago`;
}
