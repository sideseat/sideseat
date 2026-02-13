import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { type TimeRange, TIME_RANGE_OPTIONS, getTimeRangeShortLabel } from "@/lib/time-range";

interface TimeRangeSelectorProps {
  value: TimeRange;
  onChange: (value: TimeRange) => void;
}

export function TimeRangeSelector({ value, onChange }: TimeRangeSelectorProps) {
  return (
    <ToggleGroup
      type="single"
      value={value}
      onValueChange={(v) => {
        if (v) onChange(v as TimeRange);
      }}
      variant="outline"
      size="sm"
    >
      {TIME_RANGE_OPTIONS.map((option) => (
        <ToggleGroupItem key={option} value={option} aria-label={getTimeRangeShortLabel(option)}>
          {getTimeRangeShortLabel(option)}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
