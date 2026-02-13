import {
  useCallback,
  useEffect,
  useState,
  useRef,
  type ReactNode,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { Sheet, SheetContent } from "@/components/ui/sheet";
import { settings } from "@/lib/settings";
import { cn } from "@/lib/utils";

interface ResizableSheetProps {
  open: boolean;
  children: ReactNode;
  storageKey: string;
  defaultWidth?: number;
  minWidth?: number;
  maxWidth?: number;
  className?: string;
  modal?: boolean;
  onInteractOutside?: (e: Event) => void;
  onPointerDownOutside?: (e: Event) => void;
}

export function ResizableSheet({
  open,
  children,
  storageKey,
  defaultWidth = 800,
  minWidth = 400,
  maxWidth = 1400,
  className,
  modal = false,
  onInteractOutside,
  onPointerDownOutside,
}: ResizableSheetProps) {
  const [width, setWidth] = useState(() => {
    const stored = settings.get<number>(storageKey);
    const maxAllowed =
      typeof window !== "undefined" ? Math.min(maxWidth, window.innerWidth) : maxWidth;
    return stored
      ? Math.min(Math.max(stored, minWidth), maxAllowed)
      : Math.min(defaultWidth, maxAllowed);
  });
  const isResizing = useRef(false);
  const widthRef = useRef(width);

  // Keep ref in sync for settings save
  useEffect(() => {
    widthRef.current = width;
  }, [width]);

  const handleMouseDown = useCallback((e: ReactMouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    isResizing.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isResizing.current) return;
      const newWidth = window.innerWidth - e.clientX;
      const clampedWidth = Math.min(Math.max(newWidth, minWidth), maxWidth, window.innerWidth);
      setWidth(clampedWidth);
    };

    const handleMouseUp = () => {
      if (isResizing.current) {
        isResizing.current = false;
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        settings.set(storageKey, widthRef.current);
      }
    };

    const handleWindowResize = () => {
      setWidth((prev) => Math.min(prev, window.innerWidth));
    };

    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);
    window.addEventListener("resize", handleWindowResize);
    return () => {
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
      window.removeEventListener("resize", handleWindowResize);
    };
  }, [minWidth, maxWidth, storageKey]);

  return (
    <Sheet open={open} modal={modal}>
      <SheetContent
        side="right"
        hideCloseButton
        noAnimation
        hideOverlay
        onInteractOutside={onInteractOutside}
        onPointerDownOutside={onPointerDownOutside}
        className={cn("@container flex flex-col gap-0 p-0", className)}
        style={{ width, minWidth: `min(${minWidth}px, 100vw)`, maxWidth: "100vw" }}
        aria-describedby={undefined}
      >
        {/* Resize handle */}
        <div
          onMouseDown={handleMouseDown}
          className="absolute inset-y-0 left-0 z-50 hidden w-4 -translate-x-1/2 cursor-e-resize transition-all ease-linear after:absolute after:inset-y-0 after:left-1/2 after:w-0.5 hover:after:bg-border sm:flex"
        />
        {children}
      </SheetContent>
    </Sheet>
  );
}
