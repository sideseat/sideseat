import { useCallback, useEffect, useRef, useState } from "react";
import type { Node, Edge } from "@xyflow/react";
import type { SpanNodeData } from "../components/span-node";

const ANIMATION_INTERVAL_MS = 600;

interface UseDiagramAnimationOptions {
  nodes: Node<SpanNodeData>[];
  setNodes: React.Dispatch<React.SetStateAction<Node<SpanNodeData>[]>>;
  setEdges: React.Dispatch<React.SetStateAction<Edge[]>>;
}

interface UseDiagramAnimationReturn {
  isPlaying: boolean;
  onPlayStop: () => void;
}

/**
 * Get animation sequence ordered by start time (chronological execution order)
 */
function getAnimationSequence(nodes: Node<SpanNodeData>[]): string[] {
  if (nodes.length === 0) return [];

  const sorted = [...nodes].sort((a, b) => {
    const aTime = a.data.startTime ?? 0;
    const bTime = b.data.startTime ?? 0;
    return aTime - bTime;
  });

  return sorted.map((n) => n.id);
}

/**
 * Hook to manage diagram animation state and controls
 */
export function useDiagramAnimation({
  nodes,
  setNodes,
  setEdges,
}: UseDiagramAnimationOptions): UseDiagramAnimationReturn {
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentStep, setCurrentStep] = useState(-1);
  const animationSequenceRef = useRef<string[]>([]);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Reset animation state
  const resetAnimation = useCallback(() => {
    setCurrentStep(-1);
    setNodes((nds) =>
      nds.map((node) => ({
        ...node,
        data: { ...node.data, animationState: undefined },
      })),
    );
    setEdges((eds) =>
      eds.map((edge) => ({
        ...edge,
        animated: false,
        style: undefined,
      })),
    );
  }, [setNodes, setEdges]);

  // Apply animation state for current step
  useEffect(() => {
    if (currentStep < 0 || !isPlaying) return;

    const sequence = animationSequenceRef.current;
    const visitedSet = new Set(sequence.slice(0, currentStep));
    const activeId = sequence[currentStep];

    setNodes((nds) =>
      nds.map((node) => ({
        ...node,
        data: {
          ...node.data,
          animationState:
            node.id === activeId ? "active" : visitedSet.has(node.id) ? "visited" : "inactive",
        },
      })),
    );

    // Animate edge leading to active node
    setEdges((eds) =>
      eds.map((edge) => ({
        ...edge,
        animated: edge.target === activeId,
        style: edge.target === activeId ? { stroke: "var(--primary)", strokeWidth: 3 } : undefined,
      })),
    );
  }, [currentStep, isPlaying, setNodes, setEdges]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, []);

  // Play/Stop toggle handler
  const handlePlayStop = useCallback(() => {
    if (isPlaying) {
      // Stop and reset
      setIsPlaying(false);
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
      resetAnimation();
      return;
    }

    // Play - compute sequence from current nodes and start from beginning
    const sequence = getAnimationSequence(nodes);
    animationSequenceRef.current = sequence;

    resetAnimation();
    setIsPlaying(true);

    let step = 0;

    intervalRef.current = setInterval(() => {
      if (step >= sequence.length) {
        setIsPlaying(false);
        if (intervalRef.current) clearInterval(intervalRef.current);
        resetAnimation();
        return;
      }
      setCurrentStep(step);
      step++;
    }, ANIMATION_INTERVAL_MS);
  }, [isPlaying, nodes, resetAnimation]);

  return {
    isPlaying,
    onPlayStop: handlePlayStop,
  };
}
