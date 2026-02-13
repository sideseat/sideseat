/**
 * Filter configuration and utilities for data tables
 *
 * This module provides filter configurations for different entity types (Traces, Spans, Sessions)
 * and utilities for serializing/deserializing filters to/from URL parameters.
 */

import type { Filter } from "@/api/otel/types";

// === Filter Configuration ===

export type FilterConfigType = "select" | "text" | "number" | "tags";

export interface FilterConfig {
  id: string;
  label: string;
  column: string;
  type: FilterConfigType;
  unit?: string;
}

// === TRACE FILTER CONFIGS ===
// Order matches HIDEABLE_COLUMNS in traces-columns.tsx
// Note: span_count and observation_count are excluded - they are aggregate columns
// that can't be filtered at row level in the optimized query.
export const TRACE_FILTER_CONFIGS: readonly FilterConfig[] = [
  { id: "trace_name", label: "Name", column: "trace_name", type: "select" },
  { id: "duration_ms", label: "Latency", column: "duration_ms", type: "number", unit: "ms" },
  { id: "total_tokens", label: "Total Tokens", column: "total_tokens", type: "number" },
  { id: "total_cost", label: "Total Cost", column: "total_cost", type: "number", unit: "$" },
  { id: "environment", label: "Environment", column: "environment", type: "select" },
  { id: "tags", label: "Tags", column: "tags", type: "tags" },
  { id: "session_id", label: "Session", column: "session_id", type: "text" },
  { id: "user_id", label: "User", column: "user_id", type: "text" },
  { id: "trace_id", label: "Trace ID", column: "trace_id", type: "text" },
  { id: "input_cost", label: "Input Cost", column: "input_cost", type: "number", unit: "$" },
  { id: "output_cost", label: "Output Cost", column: "output_cost", type: "number", unit: "$" },
  {
    id: "cache_read_cost",
    label: "Cache Read Cost",
    column: "cache_read_cost",
    type: "number",
    unit: "$",
  },
  {
    id: "cache_write_cost",
    label: "Cache Write Cost",
    column: "cache_write_cost",
    type: "number",
    unit: "$",
  },
  { id: "input_tokens", label: "Input Tokens", column: "input_tokens", type: "number" },
  { id: "output_tokens", label: "Output Tokens", column: "output_tokens", type: "number" },
  {
    id: "cache_read_tokens",
    label: "Cache Read Tokens",
    column: "cache_read_tokens",
    type: "number",
  },
  {
    id: "cache_write_tokens",
    label: "Cache Write Tokens",
    column: "cache_write_tokens",
    type: "number",
  },
  { id: "reasoning_tokens", label: "Reasoning Tokens", column: "reasoning_tokens", type: "number" },
];

// === SPAN FILTER CONFIGS ===
// Span-specific filters for GenAI spans page
export const SPAN_FILTER_CONFIGS: readonly FilterConfig[] = [
  { id: "observation_type", label: "Type", column: "observation_type", type: "select" },
  { id: "gen_ai_request_model", label: "Model", column: "gen_ai_request_model", type: "select" },
  { id: "framework", label: "Framework", column: "framework", type: "select" },
  { id: "status_code", label: "Status", column: "status_code", type: "select" },
  { id: "environment", label: "Environment", column: "environment", type: "select" },
  { id: "gen_ai_system", label: "Provider", column: "gen_ai_system", type: "select" },
  { id: "gen_ai_agent_name", label: "Agent", column: "gen_ai_agent_name", type: "select" },
  { id: "duration_ms", label: "Duration", column: "duration_ms", type: "number", unit: "ms" },
  {
    id: "gen_ai_usage_total_tokens",
    label: "Total Tokens",
    column: "gen_ai_usage_total_tokens",
    type: "number",
  },
  {
    id: "gen_ai_cost_total",
    label: "Total Cost",
    column: "gen_ai_cost_total",
    type: "number",
    unit: "$",
  },
  { id: "trace_id", label: "Trace ID", column: "trace_id", type: "text" },
  { id: "span_id", label: "Span ID", column: "span_id", type: "text" },
  { id: "session_id", label: "Session", column: "session_id", type: "text" },
  { id: "user_id", label: "User", column: "user_id", type: "text" },
  { id: "span_name", label: "Name", column: "span_name", type: "text" },
];

// === SESSION FILTER CONFIGS ===
// Order matches server's SESSION_FILTERABLE in filters.rs
// Note: trace_count, span_count, observation_count are excluded - they are aggregate
// columns that can't be filtered at row level in the optimized query.
export const SESSION_FILTER_CONFIGS: readonly FilterConfig[] = [
  { id: "environment", label: "Environment", column: "environment", type: "select" },
  { id: "user_id", label: "User", column: "user_id", type: "select" },
  { id: "session_id", label: "Session ID", column: "session_id", type: "text" },
];

// === UTILITY FUNCTIONS ===

/**
 * Serialize filters array to JSON string for URL parameters
 */
export function serializeFilters(filters: Filter[]): string {
  if (filters.length === 0) return "";
  return JSON.stringify(filters);
}

/**
 * Parse filters from JSON string (URL parameter)
 */
export function parseFilters(str: string): Filter[] {
  if (!str) return [];
  try {
    const parsed = JSON.parse(str);
    if (!Array.isArray(parsed)) return [];
    return parsed as Filter[];
  } catch {
    return [];
  }
}

/**
 * Get filters for a specific column from the full filter array
 */
export function getFiltersForColumn(filters: Filter[], column: string): Filter[] {
  return filters.filter((f) => f.column === column);
}

/**
 * Update filters for a specific column, preserving other column filters
 */
export function updateColumnFilters(
  allFilters: Filter[],
  column: string,
  columnFilters: Filter[],
): Filter[] {
  const otherFilters = allFilters.filter((f) => f.column !== column);
  return [...otherFilters, ...columnFilters];
}
