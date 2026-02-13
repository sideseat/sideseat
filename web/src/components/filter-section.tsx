import { useState, useEffect } from "react";
import { ChevronDown, X } from "lucide-react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import type { Filter, FilterOption } from "@/api/otel/types";
import type { FilterConfig } from "@/lib/filters";

interface FilterSectionProps {
  config: FilterConfig;
  filters: Filter[];
  options?: FilterOption[];
  onChange: (filters: Filter[]) => void;
  isLoading?: boolean;
}

export function FilterSection({
  config,
  filters,
  options,
  onChange,
  isLoading,
}: FilterSectionProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [mode, setMode] = useState<"select" | "text">("select");

  const hasActiveFilters = filters.length > 0;

  return (
    <Collapsible open={isOpen} onOpenChange={setIsOpen}>
      <CollapsibleTrigger className="flex w-full items-center justify-between border-b px-4 py-3 hover:bg-accent">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium">{config.label}</span>
          {hasActiveFilters && (
            <span className="h-2 w-2 rounded-full bg-primary" aria-label="Active filter" />
          )}
        </div>
        <ChevronDown
          className={cn(
            "h-4 w-4 text-muted-foreground transition-transform",
            isOpen && "rotate-180",
          )}
        />
      </CollapsibleTrigger>
      <CollapsibleContent className="border-b bg-muted/30 px-4 py-4">
        {config.type === "select" && (
          <div className="mb-3 flex gap-1">
            <Button
              type="button"
              size="sm"
              variant={mode === "select" ? "default" : "outline"}
              onClick={() => setMode("select")}
              className="h-7 text-xs"
            >
              SELECT
            </Button>
            <Button
              type="button"
              size="sm"
              variant={mode === "text" ? "default" : "outline"}
              onClick={() => setMode("text")}
              className="h-7 text-xs"
            >
              TEXT
            </Button>
          </div>
        )}

        {config.type === "select" && mode === "select" && (
          <SelectFilter
            config={config}
            options={options}
            filters={filters}
            onChange={onChange}
            isLoading={isLoading}
          />
        )}
        {config.type === "select" && mode === "text" && (
          <TextFilter config={config} filters={filters} onChange={onChange} />
        )}
        {config.type === "text" && (
          <TextFilter config={config} filters={filters} onChange={onChange} />
        )}
        {config.type === "tags" && (
          <TagsFilter
            config={config}
            options={options}
            filters={filters}
            onChange={onChange}
            isLoading={isLoading}
          />
        )}
        {config.type === "number" && (
          <NumberFilter config={config} filters={filters} onChange={onChange} />
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

// === SelectFilter ===

interface SelectFilterProps {
  config: FilterConfig;
  options?: FilterOption[];
  filters: Filter[];
  onChange: (filters: Filter[]) => void;
  isLoading?: boolean;
}

function SelectFilter({ config, options, filters, onChange, isLoading }: SelectFilterProps) {
  const currentFilter = filters.find((f) => f.type === "string_options");
  const selected = new Set<string>((currentFilter?.value as string[]) ?? []);

  const handleToggle = (value: string, checked: boolean) => {
    const next = new Set(selected);
    if (checked) {
      next.add(value);
    } else {
      next.delete(value);
    }

    if (next.size === 0) {
      onChange([]);
    } else {
      onChange([
        {
          type: "string_options",
          column: config.column,
          operator: "any of",
          value: Array.from(next),
        },
      ]);
    }
  };

  if (isLoading && !options) {
    return (
      <div className="space-y-2">
        <div className="h-5 w-3/4 animate-pulse rounded bg-muted" />
        <div className="h-5 w-2/3 animate-pulse rounded bg-muted" />
        <div className="h-5 w-1/2 animate-pulse rounded bg-muted" />
      </div>
    );
  }

  if (!options || options.length === 0) {
    return <p className="text-sm text-muted-foreground">No options available</p>;
  }

  return (
    <div className="max-h-48 space-y-2 overflow-y-auto">
      {options.map((option) => (
        <label key={option.value} className="flex cursor-pointer items-center gap-2 text-sm">
          <Checkbox
            checked={selected.has(option.value)}
            onCheckedChange={(checked) => handleToggle(option.value, !!checked)}
          />
          <span className="flex-1 truncate">{option.value}</span>
          <span className="tabular-nums text-xs text-muted-foreground">{option.count}</span>
        </label>
      ))}
    </div>
  );
}

// === TextFilter ===

interface TextFilterProps {
  config: FilterConfig;
  filters: Filter[];
  onChange: (filters: Filter[]) => void;
}

function TextFilter({ config, filters, onChange }: TextFilterProps) {
  const [operator, setOperator] = useState<"contains" | "=" | "starts_with" | "ends_with">(
    "contains",
  );
  const [text, setText] = useState("");

  const textFilters = filters.filter((f) => f.type === "string");

  const handleAdd = () => {
    if (!text.trim()) return;

    const newFilter: Filter = {
      type: "string",
      column: config.column,
      operator,
      value: text.trim(),
    };

    onChange([...textFilters, newFilter]);
    setText("");
  };

  const handleRemove = (index: number) => {
    const updated = textFilters.filter((_, i) => i !== index);
    onChange(updated);
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap gap-1">
        {(["contains", "=", "starts_with", "ends_with"] as const).map((op) => (
          <Button
            type="button"
            key={op}
            size="sm"
            variant={operator === op ? "default" : "outline"}
            onClick={() => setOperator(op)}
            className="h-7 text-xs"
          >
            {op === "=" ? "equals" : op.replace("_", " ")}
          </Button>
        ))}
      </div>

      <div className="flex gap-2">
        <Input
          placeholder="Enter value..."
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleAdd()}
          className="h-8"
        />
        <Button type="button" size="sm" onClick={handleAdd} className="h-8">
          Add
        </Button>
      </div>

      {textFilters.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {textFilters.map((f, i) => (
            <span
              key={i}
              className="inline-flex items-center gap-1 rounded-md bg-primary/10 px-2 py-1 text-xs"
            >
              <span className="text-muted-foreground">{f.operator}</span>
              <span className="font-medium">"{f.value}"</span>
              <button
                type="button"
                onClick={() => handleRemove(i)}
                className="ml-1 hover:text-destructive"
                aria-label="Remove filter"
              >
                <X className="h-3 w-3" />
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

// === TagsFilter ===

interface TagsFilterProps {
  config: FilterConfig;
  options?: FilterOption[];
  filters: Filter[];
  onChange: (filters: Filter[]) => void;
  isLoading?: boolean;
}

function TagsFilter({ config, options, filters, onChange, isLoading }: TagsFilterProps) {
  const selectedTags = new Set<string>(filters.flatMap((f) => (f.value as string[]) ?? []));

  const handleToggle = (tag: string, checked: boolean) => {
    const next = new Set(selectedTags);
    if (checked) {
      next.add(tag);
    } else {
      next.delete(tag);
    }

    if (next.size === 0) {
      onChange([]);
    } else {
      onChange([
        {
          type: "string_options",
          column: config.column,
          operator: "any of",
          value: Array.from(next),
        },
      ]);
    }
  };

  if (isLoading && !options) {
    return (
      <div className="space-y-2">
        <div className="h-5 w-3/4 animate-pulse rounded bg-muted" />
        <div className="h-5 w-2/3 animate-pulse rounded bg-muted" />
        <div className="h-5 w-1/2 animate-pulse rounded bg-muted" />
      </div>
    );
  }

  if (!options || options.length === 0) {
    return <p className="text-sm text-muted-foreground">No tags found</p>;
  }

  return (
    <div className="max-h-48 space-y-2 overflow-y-auto">
      {options.map((option) => (
        <label key={option.value} className="flex cursor-pointer items-center gap-2 text-sm">
          <Checkbox
            checked={selectedTags.has(option.value)}
            onCheckedChange={(checked) => handleToggle(option.value, !!checked)}
          />
          <span className="flex-1 truncate">{option.value}</span>
          <span className="tabular-nums text-xs text-muted-foreground">{option.count}</span>
        </label>
      ))}
    </div>
  );
}

// === NumberFilter ===

interface NumberFilterProps {
  config: FilterConfig;
  filters: Filter[];
  onChange: (filters: Filter[]) => void;
}

function NumberFilter({ config, filters, onChange }: NumberFilterProps) {
  const minFilter = filters.find((f) => f.operator === ">=" || f.operator === ">");
  const maxFilter = filters.find((f) => f.operator === "<=" || f.operator === "<");

  const [minValue, setMinValue] = useState<string>(minFilter?.value?.toString() ?? "");
  const [maxValue, setMaxValue] = useState<string>(maxFilter?.value?.toString() ?? "");

  // Sync local state when filters change externally (e.g., "Clear all")
  useEffect(() => {
    setMinValue(minFilter?.value?.toString() ?? "");
  }, [minFilter?.value]);

  useEffect(() => {
    setMaxValue(maxFilter?.value?.toString() ?? "");
  }, [maxFilter?.value]);

  const handleApply = () => {
    const newFilters: Filter[] = [];

    if (minValue !== "") {
      const num = parseFloat(minValue);
      if (!isNaN(num)) {
        newFilters.push({
          type: "number",
          column: config.column,
          operator: ">=",
          value: num,
        });
      }
    }

    if (maxValue !== "") {
      const num = parseFloat(maxValue);
      if (!isNaN(num)) {
        newFilters.push({
          type: "number",
          column: config.column,
          operator: "<=",
          value: num,
        });
      }
    }

    onChange(newFilters);
  };

  const handleClear = () => {
    setMinValue("");
    setMaxValue("");
    onChange([]);
  };

  return (
    <div className="space-y-3">
      <div className="flex gap-3">
        <div className="flex-1">
          <label className="text-xs text-muted-foreground">Min.</label>
          <div className="flex items-center gap-1">
            <Input
              type="number"
              value={minValue}
              onChange={(e) => setMinValue(e.target.value)}
              className="h-8"
              placeholder="0"
            />
            {config.unit && <span className="text-xs text-muted-foreground">{config.unit}</span>}
          </div>
        </div>
        <div className="flex-1">
          <label className="text-xs text-muted-foreground">Max.</label>
          <div className="flex items-center gap-1">
            <Input
              type="number"
              value={maxValue}
              onChange={(e) => setMaxValue(e.target.value)}
              className="h-8"
              placeholder="any"
            />
            {config.unit && <span className="text-xs text-muted-foreground">{config.unit}</span>}
          </div>
        </div>
      </div>
      <div className="flex gap-2">
        <Button type="button" size="sm" variant="outline" onClick={handleApply} className="flex-1">
          Apply
        </Button>
        {filters.length > 0 && (
          <Button type="button" size="sm" variant="ghost" onClick={handleClear}>
            Clear
          </Button>
        )}
      </div>
    </div>
  );
}
