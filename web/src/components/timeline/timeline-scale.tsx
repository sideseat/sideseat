import { useRef, useState, useLayoutEffect } from "react";
import { cn } from "@/lib/utils";
import { calculateTimeScale } from "./lib/calculations";

interface TimelineScaleProps {
  duration: number;
  scaleWidth?: number;
  className?: string;
}

export function TimelineScale({ duration, scaleWidth, className }: TimelineScaleProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(scaleWidth ?? 0);

  useLayoutEffect(() => {
    if (scaleWidth !== undefined) {
      setWidth(scaleWidth);
      return;
    }

    if (!containerRef.current) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setWidth(entry.contentRect.width);
      }
    });

    observer.observe(containerRef.current);
    return () => observer.disconnect();
  }, [scaleWidth]);

  const ticks = calculateTimeScale(duration, width);

  return (
    <div
      ref={containerRef}
      className={cn("relative h-6 text-xs text-muted-foreground", className)}
      style={scaleWidth !== undefined ? { width: scaleWidth } : undefined}
    >
      {ticks.map((tick, index) => (
        <div
          key={index}
          className="absolute top-0 flex h-full flex-col items-start"
          style={{ left: `${tick.position}%` }}
        >
          <div className="h-2 w-px bg-border" />
          <span className="mt-0.5 -translate-x-1/2 whitespace-nowrap px-0.5">{tick.label}</span>
        </div>
      ))}
    </div>
  );
}
