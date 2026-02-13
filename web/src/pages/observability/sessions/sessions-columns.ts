import type { ColDef } from "ag-grid-community";
import { formatTimestamp24h, formatDuration, formatCost, formatTokens } from "@/lib/format";
import type { SessionSummary } from "@/api/otel/types";
import type { ColumnConfig } from "@/components/column-selector";
import {
  ActionsCellRenderer,
  FavoriteCellRenderer,
  TokensCellRenderer,
  CostCellRenderer,
} from "@/components/grid";

export const defaultColDef: ColDef<SessionSummary> = {
  resizable: true,
  sortable: false,
  sortingOrder: ["asc", "desc"],
  flex: 1,
  cellStyle: { fontVariantNumeric: "tabular-nums" },
};

export const DEFAULT_VISIBLE_COLUMNS: readonly string[] = [
  "start_time",
  "session_id",
  "user_id",
  "trace_count",
  "duration_ms",
  "tokens",
  "total_cost",
];

export const HIDEABLE_COLUMNS: readonly ColumnConfig[] = [
  { id: "start_time", label: "Timestamp" },
  { id: "session_id", label: "Session ID" },
  { id: "user_id", label: "User" },
  { id: "trace_count", label: "Traces" },
  { id: "observation_count", label: "Observations" },
  { id: "duration_ms", label: "Duration" },
  { id: "tokens", label: "Tokens" },
  { id: "total_cost", label: "Total Cost" },
  { id: "environment", label: "Environment" },
  { id: "span_count", label: "Spans" },
  { id: "input_tokens", label: "Input Tokens" },
  { id: "output_tokens", label: "Output Tokens" },
  { id: "cache_read_tokens", label: "Cache Read Tokens" },
  { id: "cache_write_tokens", label: "Cache Write Tokens" },
  { id: "reasoning_tokens", label: "Reasoning Tokens" },
  { id: "input_cost", label: "Input Cost" },
  { id: "output_cost", label: "Output Cost" },
  { id: "cache_read_cost", label: "Cache Read Cost" },
  { id: "cache_write_cost", label: "Cache Write Cost" },
  { id: "reasoning_cost", label: "Reasoning Cost" },
];

export const NON_HIDEABLE_COLUMNS = new Set(["favorite", "actions"]);

export const columnDefs: ColDef<SessionSummary>[] = [
  {
    colId: "favorite",
    headerName: "",
    width: 26,
    minWidth: 26,
    maxWidth: 26,
    sortable: false,
    resizable: false,
    flex: 0,
    cellRenderer: FavoriteCellRenderer,
    cellStyle: {
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      padding: 0,
      marginLeft: -8,
    },
  },
  {
    colId: "start_time",
    field: "start_time",
    headerName: "Timestamp",
    minWidth: 190,
    sortable: true,
    sort: "desc",
    valueFormatter: (p) => formatTimestamp24h(p.value),
    cellStyle: { paddingLeft: 0, marginLeft: -6, fontVariantNumeric: "tabular-nums" },
  },
  {
    colId: "session_id",
    field: "session_id",
    headerName: "Session ID",
    minWidth: 200,
    valueFormatter: (p) => p.value || "-",
  },
  {
    colId: "user_id",
    field: "user_id",
    headerName: "User",
    minWidth: 150,
    valueFormatter: (p) => p.value || "-",
  },
  {
    colId: "trace_count",
    field: "trace_count",
    headerName: "Traces",
    minWidth: 100,
    sortable: true,
  },
  {
    colId: "observation_count",
    field: "observation_count",
    headerName: "Observations",
    minWidth: 100,
    hide: true,
  },
  {
    colId: "duration_ms",
    headerName: "Duration",
    minWidth: 120,
    valueGetter: (p) => {
      if (!p.data?.start_time || !p.data?.end_time) return null;
      return new Date(p.data.end_time).getTime() - new Date(p.data.start_time).getTime();
    },
    valueFormatter: (p) => formatDuration(p.value),
  },
  {
    colId: "tokens",
    headerName: "Tokens",
    minWidth: 200,
    cellRenderer: TokensCellRenderer,
  },
  {
    colId: "total_cost",
    field: "total_cost",
    headerName: "Total Cost",
    minWidth: 120,
    sortable: true,
    cellRenderer: CostCellRenderer,
  },
  {
    colId: "environment",
    field: "environment",
    headerName: "Environment",
    minWidth: 120,
    hide: true,
    valueFormatter: (p) => p.value || "-",
  },
  {
    colId: "span_count",
    field: "span_count",
    headerName: "Spans",
    minWidth: 80,
    hide: true,
  },
  {
    colId: "input_tokens",
    field: "input_tokens",
    headerName: "Input Tokens",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatTokens(p.value),
  },
  {
    colId: "output_tokens",
    field: "output_tokens",
    headerName: "Output Tokens",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatTokens(p.value),
  },
  {
    colId: "cache_read_tokens",
    field: "cache_read_tokens",
    headerName: "Cache Read Tokens",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatTokens(p.value),
  },
  {
    colId: "cache_write_tokens",
    field: "cache_write_tokens",
    headerName: "Cache Write Tokens",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatTokens(p.value),
  },
  {
    colId: "reasoning_tokens",
    field: "reasoning_tokens",
    headerName: "Reasoning Tokens",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatTokens(p.value),
  },
  {
    colId: "input_cost",
    field: "input_cost",
    headerName: "Input Cost",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatCost(p.value),
  },
  {
    colId: "output_cost",
    field: "output_cost",
    headerName: "Output Cost",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatCost(p.value),
  },
  {
    colId: "cache_read_cost",
    field: "cache_read_cost",
    headerName: "Cache Read Cost",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatCost(p.value),
  },
  {
    colId: "cache_write_cost",
    field: "cache_write_cost",
    headerName: "Cache Write Cost",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatCost(p.value),
  },
  {
    colId: "reasoning_cost",
    field: "reasoning_cost",
    headerName: "Reasoning Cost",
    minWidth: 100,
    hide: true,
    valueFormatter: (p) => formatCost(p.value),
  },
  {
    colId: "actions",
    headerName: "Actions",
    minWidth: 96,
    width: 96,
    maxWidth: 120,
    sortable: false,
    resizable: false,
    cellRenderer: ActionsCellRenderer,
    cellStyle: { display: "flex", alignItems: "center", justifyContent: "center", padding: 0 },
  },
];

export const rowSelectionConfig = {
  mode: "multiRow" as const,
  headerCheckbox: true,
  checkboxes: true,
  enableClickSelection: false,
};
