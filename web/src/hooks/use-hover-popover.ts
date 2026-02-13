import { useCallback, useEffect, useRef, useState } from "react";

const OPEN_DELAY = 400;
const CLOSE_DELAY = 150;

export function useHoverPopover() {
  const [open, setOpen] = useState(false);
  const openTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const closeTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimeouts = useCallback(() => {
    if (openTimeoutRef.current) {
      clearTimeout(openTimeoutRef.current);
      openTimeoutRef.current = null;
    }
    if (closeTimeoutRef.current) {
      clearTimeout(closeTimeoutRef.current);
      closeTimeoutRef.current = null;
    }
  }, []);

  const handleMouseEnter = useCallback(() => {
    clearTimeouts();
    openTimeoutRef.current = setTimeout(() => setOpen(true), OPEN_DELAY);
  }, [clearTimeouts]);

  const handleMouseLeave = useCallback(() => {
    clearTimeouts();
    closeTimeoutRef.current = setTimeout(() => setOpen(false), CLOSE_DELAY);
  }, [clearTimeouts]);

  useEffect(() => {
    return () => clearTimeouts();
  }, [clearTimeouts]);

  return {
    open,
    setOpen,
    handleMouseEnter,
    handleMouseLeave,
  };
}
