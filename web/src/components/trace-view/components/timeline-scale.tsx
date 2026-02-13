import { TimelineScale as BaseTimelineScale } from "@/components/timeline";
import { useTraceView } from "../contexts/use-trace-view";

interface TimelineScaleProps {
  scaleWidth: number;
}

export function TimelineScale({ scaleWidth }: TimelineScaleProps) {
  const { traceDuration } = useTraceView();
  return <BaseTimelineScale duration={traceDuration} scaleWidth={scaleWidth} />;
}
