/* eslint-disable react-refresh/only-export-components */
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

interface ToolbarSlotContextValue {
  setNode: (node: ReactNode) => void;
}

const ToolbarSlotContext = createContext<ToolbarSlotContextValue | null>(null);

interface ProviderProps {
  children: (node: ReactNode) => ReactNode;
}

/**
 * Wraps a region that wants to render a page-controlled toolbar slot.
 * The render prop receives the current slot node so the consumer (e.g.
 * the project layout's header) can mount it wherever appropriate.
 */
export function PageToolbarProvider({ children }: ProviderProps) {
  const [node, setNode] = useState<ReactNode>(null);
  const value = useMemo<ToolbarSlotContextValue>(() => ({ setNode }), []);
  return (
    <ToolbarSlotContext.Provider value={value}>
      {children(node)}
    </ToolbarSlotContext.Provider>
  );
}

/**
 * Pages call this to mount actions into the layout's top toolbar.
 * Pass `null` (or unmount) to clear the slot.
 */
export function usePageToolbar(node: ReactNode): void {
  const ctx = useContext(ToolbarSlotContext);
  useEffect(() => {
    if (!ctx) return;
    ctx.setNode(node);
    return () => ctx.setNode(null);
  }, [ctx, node]);
}
