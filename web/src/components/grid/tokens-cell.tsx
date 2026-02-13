import type { ICellRendererParams } from "ag-grid-community";
import { formatTokens } from "@/lib/format";
import type { TokenBreakdown } from "@/components/breakdown-popover";
import { UsageBreakdownPopover } from "@/components/breakdown-popover";

export function TokensCellRenderer(params: ICellRendererParams<TokenBreakdown>) {
  const data = params.data;

  if (!data) return null;

  const {
    input_tokens,
    output_tokens,
    cache_read_tokens,
    cache_write_tokens,
    reasoning_tokens,
    total_tokens,
  } = data;

  const inputTotal = input_tokens + cache_read_tokens + cache_write_tokens;
  const outputTotal = output_tokens + reasoning_tokens;
  const grandTotal = total_tokens || inputTotal + outputTotal;

  if (grandTotal === 0) return <span className="text-muted-foreground">-</span>;

  return (
    <UsageBreakdownPopover data={data}>
      <span className="w-full h-full flex items-center tabular-nums">
        {formatTokens(inputTotal)} &rarr; {formatTokens(outputTotal)} (&Sigma;{" "}
        {formatTokens(grandTotal)})
      </span>
    </UsageBreakdownPopover>
  );
}
