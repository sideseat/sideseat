import {
  Bot,
  Sparkles,
  Wrench,
  Database,
  Search,
  PanelTopBottomDashed,
  Box,
  Binary,
  type LucideIcon,
} from "lucide-react";
import { formatDuration } from "@/lib/format";
import { tw } from "@/lib/utils";
import type { SpanType } from "./types";

export { formatDuration };

interface SpanTypeConfig {
  icon: LucideIcon;
  label: string;
  accent: string;
  hexColor: string; // For exports (draw.io, SVG, etc.)
}

export const SPAN_TYPE_CONFIG: Record<SpanType, SpanTypeConfig> = {
  llm: {
    icon: Sparkles,
    label: "LLM Call",
    accent: tw("text-span-llm"),
    hexColor: "#3B82F6", // blue-500
  },
  tool: {
    icon: Wrench,
    label: "Tool",
    accent: tw("text-span-tool"),
    hexColor: "#F97316", // orange-500
  },
  agent: {
    icon: Bot,
    label: "Agent",
    accent: tw("text-span-agent"),
    hexColor: "#10B981", // emerald-500
  },
  embedding: {
    icon: Binary,
    label: "Embedding",
    accent: tw("text-span-embedding"),
    hexColor: "#8B5CF6", // purple-500
  },
  retriever: {
    icon: Search,
    label: "Retriever",
    accent: tw("text-span-retriever"),
    hexColor: "#14B8A6", // teal-500
  },
  http: {
    icon: PanelTopBottomDashed,
    label: "HTTP",
    accent: tw("text-span-http"),
    hexColor: "#64748B", // slate-500
  },
  db: {
    icon: Database,
    label: "Database",
    accent: tw("text-span-db"),
    hexColor: "#F59E0B", // amber-500
  },
  span: {
    icon: Box,
    label: "Span",
    accent: tw("text-muted-foreground"),
    hexColor: "#6B7280", // gray-500
  },
};

export function getDurationHeatmapColor(duration: number, maxDuration: number): string {
  if (maxDuration === 0) return "text-muted-foreground";

  const percentage = (duration / maxDuration) * 100;

  if (percentage < 25) return "text-muted-foreground";
  if (percentage < 50) return tw("text-heat-low");
  if (percentage < 75) return tw("text-heat-mid");
  return tw("text-destructive");
}

export function formatTokens(input: number, output: number, total: number): string {
  if (input > 0 || output > 0) {
    const sum = total > 0 ? total : input + output;
    return `${input.toLocaleString()} → ${output.toLocaleString()} (Σ ${sum.toLocaleString()})`;
  }
  if (total > 0) {
    return `${total.toLocaleString()} tokens`;
  }
  return "";
}

export function formatCost(cost: number): string {
  if (cost === 0) return "";
  if (cost >= 1) return `$${cost.toFixed(2)}`;

  // Dynamic precision: show 3 significant figures
  const decimals = Math.min(10, Math.max(2, Math.ceil(-Math.log10(cost)) + 2));
  const formatted = cost.toFixed(decimals);
  // Strip trailing zeros but keep at least 2 decimal places
  return `$${formatted.replace(/(\.\d{2,}?)0+$/, "$1")}`;
}
