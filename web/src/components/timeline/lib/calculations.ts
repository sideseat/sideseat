import type { TimeScaleTick, TimelineMetrics } from "../types";

// Fixed constants for timeline
export const SCALE_WIDTH = 900; // Fixed timeline width in pixels
export const MIN_BAR_WIDTH = 10; // Minimum bar width for visibility

export function calculateTimelineMetrics(
  itemStartTime: Date,
  itemDuration: number | undefined,
  traceStartTime: Date,
  traceDuration: number,
  scaleWidth: number = SCALE_WIDTH,
): TimelineMetrics {
  if (traceDuration <= 0) {
    return { barWidth: scaleWidth, marginLeft: 0 };
  }

  const timeFromStart = itemStartTime.getTime() - traceStartTime.getTime();
  const marginLeft = (timeFromStart / traceDuration) * scaleWidth;
  const barWidth = Math.max(MIN_BAR_WIDTH, ((itemDuration ?? 0) / traceDuration) * scaleWidth);

  return { barWidth, marginLeft };
}

export function calculateTimeScale(totalDuration: number, containerWidth: number): TimeScaleTick[] {
  if (totalDuration === 0 || containerWidth === 0) {
    return [{ position: 0, label: "0s" }];
  }

  const minPixelsBetweenTicks = 80;
  const maxTicks = Math.floor(containerWidth / minPixelsBetweenTicks);
  const targetTicks = Math.max(2, Math.min(maxTicks, 10));

  const intervals = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000, 50000, 100000,
  ];

  const rawInterval = totalDuration / targetTicks;
  const interval = intervals.find((i) => i >= rawInterval) ?? totalDuration;

  const ticks: TimeScaleTick[] = [];
  let time = 0;

  while (time <= totalDuration) {
    const position = (time / totalDuration) * 100;
    ticks.push({
      position,
      label: formatTimeLabel(time),
    });
    time += interval;
  }

  if (ticks.length > 0 && ticks[ticks.length - 1].position < 95) {
    ticks.push({
      position: 100,
      label: formatTimeLabel(totalDuration),
    });
  }

  return ticks;
}

export function formatTimeLabel(ms: number): string {
  if (ms === 0) return "0s";
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60000);
  const seconds = Math.round((ms % 60000) / 1000);
  return seconds > 0 ? `${minutes}m${seconds}s` : `${minutes}m`;
}
