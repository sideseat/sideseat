import type { ReactNode } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatTokens } from "@/lib/format";
import { useHoverPopover } from "@/hooks/use-hover-popover";
import { tokenDisplay } from "@/lib/token-breakdown";
import { BreakdownRow } from "./breakdown-row";

export interface TokenBreakdown {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
}

interface UsageBreakdownPopoverProps {
  data: TokenBreakdown;
  children: ReactNode;
}

export function UsageBreakdownPopover({ data, children }: UsageBreakdownPopoverProps) {
  const { open, setOpen, handleMouseEnter, handleMouseLeave } = useHoverPopover();

  const { cache_read_tokens, cache_write_tokens, reasoning_tokens } = data;
  const { input, output, separate, separateOf, total } = tokenDisplay(data);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <span
          className="cursor-default"
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          {children}
        </span>
      </PopoverTrigger>
      <PopoverContent
        className="w-72"
        align="center"
        side="top"
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
      >
        <div className="space-y-3">
          <h4 className="font-semibold">Usage Breakdown</h4>
          <div className="space-y-2 text-sm">
            <div className="space-y-1">
              <BreakdownRow label="Input" value={formatTokens(input)} bold />
              {cache_read_tokens > 0 && (
                <BreakdownRow label="cache read" value={formatTokens(cache_read_tokens)} indent />
              )}
              {cache_write_tokens > 0 && (
                <BreakdownRow label="cache write" value={formatTokens(cache_write_tokens)} indent />
              )}
            </div>
            <div className="space-y-1">
              <BreakdownRow label="Output" value={formatTokens(output)} bold />
              {reasoning_tokens > 0 && (
                <BreakdownRow label="reasoning" value={formatTokens(reasoning_tokens)} indent />
              )}
            </div>
            {separate > 0 && (
              <BreakdownRow
                label={`Billed separately (${separateOf.join(", ")})`}
                value={formatTokens(separate)}
              />
            )}
            <div className="border-t pt-2">
              <BreakdownRow label="Total" value={formatTokens(total)} bold />
            </div>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
