import { useState, useCallback, useMemo } from "react";
import { Calendar as CalendarIcon, Check, ChevronDown, ChevronLeft, Clock } from "lucide-react";
import type { DateRange } from "react-day-picker";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Calendar } from "@/components/ui/calendar";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import {
  TIME_PRESETS,
  DEFAULT_TIME_PRESET,
  getPresetByValue,
  getPresetRange,
  getTimezoneAbbr,
  combineDateAndTime,
  formatTime24to12,
  type TimeValue,
} from "@/lib/time-filter";

type ViewMode = "presets" | "calendar";

export interface TimeFilterProps {
  preset?: string;
  fromTimestamp?: string;
  toTimestamp?: string;
  onFilterChange: (
    preset: string | undefined,
    from: string | undefined,
    to: string | undefined,
  ) => void;
}

function TimeInput({
  label,
  value,
  onChange,
  showTimezone,
}: {
  label: string;
  value: TimeValue;
  onChange: (value: TimeValue) => void;
  showTimezone?: boolean;
}) {
  const timezoneAbbr = getTimezoneAbbr();

  const handleNumericChange = (field: "hours" | "minutes" | "seconds", inputValue: string) => {
    const numeric = inputValue.replace(/\D/g, "").slice(0, 2);
    onChange({ ...value, [field]: numeric });
  };

  const handleBlur = (field: "hours" | "minutes" | "seconds") => {
    let num = parseInt(value[field], 10) || 0;

    if (field === "hours") {
      if (num < 1) num = 1;
      if (num > 12) num = 12;
    } else {
      if (num > 59) num = 59;
    }

    onChange({ ...value, [field]: String(num).padStart(2, "0") });
  };

  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2">
        <label className="text-xs font-medium text-muted-foreground">{label}</label>
        {showTimezone && <span className="text-xs text-muted-foreground">({timezoneAbbr})</span>}
      </div>
      <div className="flex items-center gap-1.5">
        <Clock className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <Input
          type="text"
          inputMode="numeric"
          value={value.hours}
          onChange={(e) => handleNumericChange("hours", e.target.value)}
          onBlur={() => handleBlur("hours")}
          className="h-8 w-11 px-1 text-center tabular-nums"
          maxLength={2}
          aria-label={`${label} hours`}
        />
        <span className="text-muted-foreground" aria-hidden="true">
          :
        </span>
        <Input
          type="text"
          inputMode="numeric"
          value={value.minutes}
          onChange={(e) => handleNumericChange("minutes", e.target.value)}
          onBlur={() => handleBlur("minutes")}
          className="h-8 w-11 px-1 text-center tabular-nums"
          maxLength={2}
          aria-label={`${label} minutes`}
        />
        <span className="text-muted-foreground" aria-hidden="true">
          :
        </span>
        <Input
          type="text"
          inputMode="numeric"
          value={value.seconds}
          onChange={(e) => handleNumericChange("seconds", e.target.value)}
          onBlur={() => handleBlur("seconds")}
          className="h-8 w-11 px-1 text-center tabular-nums"
          maxLength={2}
          aria-label={`${label} seconds`}
        />
        <Select
          value={value.period}
          onValueChange={(p) => onChange({ ...value, period: p as "AM" | "PM" })}
        >
          <SelectTrigger className="h-8 w-[4.5rem]" aria-label={`${label} AM/PM`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="AM">AM</SelectItem>
            <SelectItem value="PM">PM</SelectItem>
          </SelectContent>
        </Select>
      </div>
    </div>
  );
}

export function TimeFilter({
  preset,
  fromTimestamp,
  toTimestamp,
  onFilterChange,
}: TimeFilterProps) {
  const [open, setOpen] = useState(false);
  const [viewMode, setViewMode] = useState<ViewMode>("presets");
  const [dateRange, setDateRange] = useState<DateRange | undefined>();
  const [startTime, setStartTime] = useState<TimeValue>({
    hours: "12",
    minutes: "00",
    seconds: "00",
    period: "AM",
  });
  const [endTime, setEndTime] = useState<TimeValue>({
    hours: "11",
    minutes: "59",
    seconds: "59",
    period: "PM",
  });

  const currentPreset = useMemo(() => getPresetByValue(preset ?? ""), [preset]);

  const displayLabel = useMemo(() => {
    if (currentPreset) {
      return currentPreset.label;
    }
    if (preset === "custom" && fromTimestamp && toTimestamp) {
      return "Custom range";
    }
    return "Select time";
  }, [currentPreset, preset, fromTimestamp, toTimestamp]);

  const displayBadge = useMemo(() => {
    if (currentPreset) {
      return currentPreset.value;
    }
    if (preset === "custom") {
      return "custom";
    }
    return null;
  }, [currentPreset, preset]);

  const handleOpenChange = useCallback((isOpen: boolean) => {
    setOpen(isOpen);
    if (!isOpen) {
      setViewMode("presets");
    }
  }, []);

  const handlePresetSelect = useCallback(
    (presetValue: string) => {
      const range = getPresetRange(presetValue);
      if (range) {
        onFilterChange(presetValue, range.from, undefined);
      }
      setOpen(false);
    },
    [onFilterChange],
  );

  const handleCalendarSelect = useCallback(() => {
    const now = new Date();

    if (fromTimestamp && toTimestamp) {
      const from = new Date(fromTimestamp);
      const to = new Date(toTimestamp);
      setDateRange({ from, to });
      setStartTime(formatTime24to12(from.getHours(), from.getMinutes(), from.getSeconds()));
      setEndTime(formatTime24to12(to.getHours(), to.getMinutes(), to.getSeconds()));
    } else if (fromTimestamp) {
      const from = new Date(fromTimestamp);
      setDateRange({ from, to: now });
      setStartTime(formatTime24to12(from.getHours(), from.getMinutes(), from.getSeconds()));
      setEndTime(formatTime24to12(now.getHours(), now.getMinutes(), now.getSeconds()));
    } else {
      const defaultPreset = getPresetByValue(DEFAULT_TIME_PRESET);
      const defaultMs = defaultPreset?.ms ?? 7 * 24 * 60 * 60 * 1000;
      const defaultFrom = new Date(now.getTime() - defaultMs);
      setDateRange({ from: defaultFrom, to: now });
      setStartTime({ hours: "12", minutes: "00", seconds: "00", period: "AM" });
      setEndTime(formatTime24to12(now.getHours(), now.getMinutes(), now.getSeconds()));
    }
    setViewMode("calendar");
  }, [fromTimestamp, toTimestamp]);

  const handleApplyCustomRange = useCallback(() => {
    if (!dateRange?.from || !dateRange?.to) return;

    const fromDate = combineDateAndTime(dateRange.from, startTime);
    const toDate = combineDateAndTime(dateRange.to, endTime);

    if (fromDate >= toDate) return;

    onFilterChange("custom", fromDate.toISOString(), toDate.toISOString());
    setOpen(false);
  }, [dateRange, startTime, endTime, onFilterChange]);

  const isValidRange = useMemo(() => {
    if (!dateRange?.from || !dateRange?.to) return false;
    const fromDate = combineDateAndTime(dateRange.from, startTime);
    const toDate = combineDateAndTime(dateRange.to, endTime);
    return fromDate < toDate;
  }, [dateRange, startTime, endTime]);

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" className="gap-2">
          {displayBadge && (
            <span className="rounded bg-muted px-1.5 py-0.5 text-xs font-medium tabular-nums">
              {displayBadge}
            </span>
          )}
          <span className="hidden sm:inline">{displayLabel}</span>
          <ChevronDown className="h-4 w-4 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-auto p-0" align="end">
        {viewMode === "presets" ? (
          <div className="p-1">
            {TIME_PRESETS.map((p) => {
              const isSelected = preset === p.value;
              return (
                <button
                  key={p.value}
                  type="button"
                  onClick={() => handlePresetSelect(p.value)}
                  className={cn(
                    "flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors",
                    "hover:bg-accent hover:text-accent-foreground",
                    isSelected && "bg-accent",
                  )}
                >
                  <span className="w-8 font-medium tabular-nums text-muted-foreground">
                    {p.value}
                  </span>
                  <span className="flex-1 text-left">{p.label}</span>
                  {isSelected && <Check className="h-4 w-4" />}
                </button>
              );
            })}
            <Separator className="my-1" />
            <button
              type="button"
              onClick={handleCalendarSelect}
              className={cn(
                "flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors",
                "hover:bg-accent hover:text-accent-foreground",
                preset === "custom" && "bg-accent",
              )}
            >
              <CalendarIcon className="h-4 w-4 text-muted-foreground" />
              <span className="flex-1 text-left">Select from calendar</span>
              {preset === "custom" && <Check className="h-4 w-4" />}
            </button>
          </div>
        ) : (
          <div className="p-3">
            <button
              type="button"
              onClick={() => setViewMode("presets")}
              className="mb-3 flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
            >
              <ChevronLeft className="h-4 w-4" />
              Back
            </button>

            <div className="flex justify-center">
              <Calendar
                mode="range"
                selected={dateRange}
                onSelect={setDateRange}
                numberOfMonths={1}
                defaultMonth={dateRange?.from}
              />
            </div>

            <Separator className="my-3" />

            <div className="space-y-3">
              <TimeInput
                label="Start time"
                value={startTime}
                onChange={setStartTime}
                showTimezone
              />
              <TimeInput label="End time" value={endTime} onChange={setEndTime} />
            </div>

            <Button
              onClick={handleApplyCustomRange}
              disabled={!isValidRange}
              className="mt-4 w-full"
            >
              Apply
            </Button>
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}
