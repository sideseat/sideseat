import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  useNodesState,
  useEdgesState,
  useReactFlow,
  ReactFlowProvider,
  type Node,
  type Edge,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  Maximize2,
  MoreHorizontal,
  Download,
  Copy,
  Check,
  FileDown,
  Play,
  Square,
} from "lucide-react";
import { useTraceView } from "../contexts/use-trace-view";
import { treeToFlow, layoutNodes } from "../lib/diagram-utils";
import { useDiagramAnimation } from "../lib/use-diagram-animation";
import { exportDiagramAsPng } from "../lib/export-png";
import { generateMermaid } from "../lib/export-mermaid";
import { generateDrawIO } from "../lib/export-drawio";
import { downloadFile } from "@/lib/utils";
import { SpanNode, type SpanNodeData } from "./span-node";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

const nodeTypes = { span: SpanNode };

interface AnimationControls {
  isPlaying: boolean;
  onPlayStop: () => void;
}

function DiagramControls({
  nodes,
  edges,
  onFullscreen,
  containerRef,
  animation,
}: {
  nodes: Node<SpanNodeData>[];
  edges: Edge[];
  onFullscreen?: () => void;
  containerRef: React.RefObject<HTMLDivElement | null>;
  animation?: AnimationControls;
}) {
  const { getNodes } = useReactFlow();
  const [mermaidCopied, setMermaidCopied] = useState(false);
  const copyTimeoutRef = useRef<ReturnType<typeof setTimeout>>(null);

  useEffect(() => {
    return () => {
      if (copyTimeoutRef.current) {
        clearTimeout(copyTimeoutRef.current);
      }
    };
  }, []);

  const handleExportPng = useCallback(async () => {
    try {
      await exportDiagramAsPng(containerRef.current, getNodes());
    } catch (err) {
      console.error("Failed to export PNG:", err);
    }
  }, [getNodes, containerRef]);

  const handleCopyMermaid = useCallback(async () => {
    try {
      const mermaid = generateMermaid(nodes, edges);
      await navigator.clipboard.writeText(mermaid);
      setMermaidCopied(true);
      if (copyTimeoutRef.current) clearTimeout(copyTimeoutRef.current);
      copyTimeoutRef.current = setTimeout(() => setMermaidCopied(false), 2000);
    } catch (err) {
      console.error("Failed to copy Mermaid:", err);
    }
  }, [nodes, edges]);

  const handleExportDrawIO = useCallback(() => {
    try {
      const drawio = generateDrawIO(nodes, edges);
      downloadFile(drawio, "trace-diagram.drawio", "application/xml");
    } catch (err) {
      console.error("Failed to export Draw.io:", err);
    }
  }, [nodes, edges]);

  return (
    <Controls>
      {animation && (
        <button
          type="button"
          onClick={animation.onPlayStop}
          className="react-flow__controls-button"
          title={animation.isPlaying ? "Stop animation" : "Play animation"}
        >
          {animation.isPlaying ? (
            <Square className="h-3.5 w-3.5" />
          ) : (
            <Play className="h-3.5 w-3.5" />
          )}
        </button>
      )}
      {onFullscreen && (
        <button
          type="button"
          onClick={onFullscreen}
          className="react-flow__controls-button"
          title="Open fullscreen"
        >
          <Maximize2 className="h-3.5 w-3.5" />
        </button>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className="react-flow__controls-button" title="More options">
            <MoreHorizontal className="h-3.5 w-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right">
          <DropdownMenuItem onClick={handleExportPng}>
            <Download className="mr-2 h-4 w-4" />
            Download as PNG
          </DropdownMenuItem>
          <DropdownMenuItem onClick={handleExportDrawIO}>
            <FileDown className="mr-2 h-4 w-4" />
            Download as Draw.io
          </DropdownMenuItem>
          <DropdownMenuItem onClick={handleCopyMermaid}>
            {mermaidCopied ? (
              <Check className="mr-2 h-4 w-4 text-primary" />
            ) : (
              <Copy className="mr-2 h-4 w-4" />
            )}
            Copy as Mermaid
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </Controls>
  );
}

function DiagramContent({
  nodes,
  edges,
  onNodesChange,
  onNodeClick,
  onFullscreen,
  containerRef,
  animation,
}: {
  nodes: Node<SpanNodeData>[];
  edges: Edge[];
  onNodesChange: ReturnType<typeof useNodesState<Node<SpanNodeData>>>[2];
  onNodeClick: (_: React.MouseEvent, node: Node<SpanNodeData>) => void;
  onFullscreen?: () => void;
  containerRef: React.RefObject<HTMLDivElement | null>;
  animation?: AnimationControls;
}) {
  return (
    <div ref={containerRef} className="h-full w-full">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onNodeClick={onNodeClick}
        nodeTypes={nodeTypes}
        nodesConnectable={false}
        selectionOnDrag={false}
        selectNodesOnDrag={false}
        fitView
        proOptions={{ hideAttribution: true }}
        defaultEdgeOptions={{
          style: {
            stroke: "var(--border)",
            strokeWidth: 2,
          },
        }}
      >
        <Background />
        <DiagramControls
          nodes={nodes}
          edges={edges}
          onFullscreen={onFullscreen}
          containerRef={containerRef}
          animation={animation}
        />
      </ReactFlow>
    </div>
  );
}

export function SpanDiagram() {
  const { filteredTree, selectedSpanId, setSelectedSpanId } = useTraceView();
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<SpanNodeData>>([]);
  const [edges, setEdges] = useEdgesState<Edge>([]);
  const [isLayouting, setIsLayouting] = useState(true);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const mainContainerRef = useRef<HTMLDivElement>(null);
  const fullscreenContainerRef = useRef<HTMLDivElement>(null);

  // Animation hook
  const { isPlaying, onPlayStop } = useDiagramAnimation({
    nodes,
    setNodes,
    setEdges,
  });

  // Layout nodes when tree changes
  useEffect(() => {
    if (!filteredTree) return;

    let cancelled = false;

    const runLayout = async () => {
      setIsLayouting(true);
      const flow = treeToFlow(filteredTree);
      const layouted = await layoutNodes(flow);
      if (cancelled) return;
      setNodes(layouted.nodes as Node<SpanNodeData>[]);
      setEdges(layouted.edges);
      setIsLayouting(false);
    };

    runLayout();

    return () => {
      cancelled = true;
    };
  }, [filteredTree, setNodes, setEdges]);

  // Update node selection (only when not animating)
  useEffect(() => {
    if (isLayouting || isPlaying) return;
    setNodes((nds) =>
      nds.map((node) => ({
        ...node,
        selected: node.id === selectedSpanId,
        data: { ...node.data, animationState: undefined },
      })),
    );
  }, [selectedSpanId, isLayouting, isPlaying, setNodes]);

  const onNodeClick = useCallback(
    (_: React.MouseEvent, node: Node<SpanNodeData>) => {
      setSelectedSpanId(node.id);
    },
    [setSelectedSpanId],
  );

  const animationControls = useMemo<AnimationControls>(
    () => ({ isPlaying, onPlayStop }),
    [isPlaying, onPlayStop],
  );

  if (!filteredTree) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No spans available
      </div>
    );
  }

  if (isLayouting) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Loading diagram...
      </div>
    );
  }

  return (
    <>
      <DiagramContent
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onNodeClick={onNodeClick}
        onFullscreen={() => setIsFullscreen(true)}
        containerRef={mainContainerRef}
        animation={animationControls}
      />

      <Dialog open={isFullscreen} onOpenChange={setIsFullscreen}>
        <DialogContent className="fixed! inset-0! translate-x-0! translate-y-0! max-w-none! h-full! w-full! rounded-none! border-0! p-0! gap-0! sm:max-w-none!">
          <DialogTitle className="sr-only">Trace Diagram</DialogTitle>
          <ReactFlowProvider>
            <DiagramContent
              nodes={nodes}
              edges={edges}
              onNodesChange={onNodesChange}
              onNodeClick={onNodeClick}
              containerRef={fullscreenContainerRef}
              animation={animationControls}
            />
          </ReactFlowProvider>
        </DialogContent>
      </Dialog>
    </>
  );
}
