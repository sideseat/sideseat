import { useContext } from "react";
import { TraceViewContext, type TraceViewContextValue } from "./context";

export function useTraceView(): TraceViewContextValue {
  const context = useContext(TraceViewContext);
  if (!context) {
    throw new Error("useTraceView must be used within a TraceViewProvider");
  }
  return context;
}
