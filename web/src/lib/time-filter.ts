export interface TimePreset {
  value: string;
  label: string;
  ms: number;
}

export const TIME_PRESETS: readonly TimePreset[] = [
  { value: "30m", label: "Past 30 min", ms: 30 * 60 * 1000 },
  { value: "1h", label: "Past 1 hour", ms: 60 * 60 * 1000 },
  { value: "6h", label: "Past 6 hours", ms: 6 * 60 * 60 * 1000 },
  { value: "1d", label: "Past 1 day", ms: 24 * 60 * 60 * 1000 },
  { value: "3d", label: "Past 3 days", ms: 3 * 24 * 60 * 60 * 1000 },
  { value: "7d", label: "Past 7 days", ms: 7 * 24 * 60 * 60 * 1000 },
  { value: "14d", label: "Past 14 days", ms: 14 * 24 * 60 * 60 * 1000 },
  { value: "30d", label: "Past 30 days", ms: 30 * 24 * 60 * 60 * 1000 },
  { value: "90d", label: "Past 90 days", ms: 90 * 24 * 60 * 60 * 1000 },
];

export const DEFAULT_TIME_PRESET = "7d";

export function getPresetByValue(value: string): TimePreset | undefined {
  return TIME_PRESETS.find((p) => p.value === value);
}

export function getPresetRange(presetValue: string): { from: string } | null {
  const preset = getPresetByValue(presetValue);
  if (!preset) return null;
  const now = new Date();
  const from = new Date(now.getTime() - preset.ms);
  return {
    from: from.toISOString(),
  };
}

// Use browser's default locale for timezone abbreviation
const timezoneAbbrCache = new Intl.DateTimeFormat(undefined, { timeZoneName: "short" })
  .formatToParts(new Date())
  .find((p) => p.type === "timeZoneName")?.value;

const resolvedTimeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;

export function getTimezoneAbbr(): string {
  if (timezoneAbbrCache && timezoneAbbrCache !== "GMT") {
    return timezoneAbbrCache;
  }

  const parts = resolvedTimeZone.split("/");
  if (parts.length > 1) {
    return parts[parts.length - 1].replace(/_/g, " ");
  }

  return resolvedTimeZone || "UTC";
}

export interface TimeValue {
  hours: string;
  minutes: string;
  seconds: string;
  period: "AM" | "PM";
}

export function parseTime12to24(time: TimeValue): { h: number; m: number; s: number } {
  let h = parseInt(time.hours, 10) || 0;
  const m = parseInt(time.minutes, 10) || 0;
  const s = parseInt(time.seconds, 10) || 0;

  if (h < 1) h = 1;
  if (h > 12) h = 12;

  if (time.period === "PM" && h !== 12) h += 12;
  if (time.period === "AM" && h === 12) h = 0;

  return { h, m: Math.min(59, Math.max(0, m)), s: Math.min(59, Math.max(0, s)) };
}

export function formatTime24to12(h: number, m: number, s: number): TimeValue {
  const period: "AM" | "PM" = h >= 12 ? "PM" : "AM";
  const h12 = h % 12 || 12;
  return {
    hours: String(h12).padStart(2, "0"),
    minutes: String(m).padStart(2, "0"),
    seconds: String(s).padStart(2, "0"),
    period,
  };
}

export function combineDateAndTime(date: Date, time: TimeValue): Date {
  const { h, m, s } = parseTime12to24(time);
  const result = new Date(date);
  result.setHours(h, m, s, 0);
  return result;
}
