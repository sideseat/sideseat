import { useState, useCallback, useRef, useEffect } from "react";

interface UseCopyResult {
  copied: boolean;
  copy: (text: string) => Promise<void>;
}

export function useCopy(timeout = 2000): UseCopyResult {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  const copy = useCallback(
    async (text: string) => {
      try {
        await navigator.clipboard.writeText(text);
        setCopied(true);
        if (timeoutRef.current) {
          clearTimeout(timeoutRef.current);
        }
        timeoutRef.current = setTimeout(() => setCopied(false), timeout);
      } catch {
        // Clipboard API failed silently
      }
    },
    [timeout],
  );

  return { copied, copy };
}
