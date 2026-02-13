import { toPng } from "html-to-image";
import { getNodesBounds, getViewportForBounds } from "@xyflow/react";
import type { Node } from "@xyflow/react";
import { downloadFile } from "@/lib/utils";

const IMAGE_WIDTH = 2048;
const IMAGE_HEIGHT = 1536;

/**
 * Export diagram as PNG image.
 * Renders the current viewport and triggers download.
 *
 * @param container - The diagram container element
 * @param nodes - Current nodes (for calculating bounds)
 * @param filename - Output filename (default: "trace-diagram.png")
 */
export async function exportDiagramAsPng(
  container: HTMLElement | null,
  nodes: Node[],
  filename = "trace-diagram.png",
): Promise<void> {
  if (!container || nodes.length === 0) return;

  const viewportEl = container.querySelector(".react-flow__viewport") as HTMLElement;
  if (!viewportEl) return;

  const nodesBounds = getNodesBounds(nodes);
  const viewport = getViewportForBounds(
    nodesBounds,
    IMAGE_WIDTH,
    IMAGE_HEIGHT,
    0.5, // minZoom
    2, // maxZoom
    0.2, // padding (fraction of viewport)
  );

  const dataUrl = await toPng(viewportEl, {
    backgroundColor: "#ffffff",
    width: IMAGE_WIDTH,
    height: IMAGE_HEIGHT,
    style: {
      width: `${IMAGE_WIDTH}px`,
      height: `${IMAGE_HEIGHT}px`,
      transform: `translate(${viewport.x}px, ${viewport.y}px) scale(${viewport.zoom})`,
    },
  });

  // Use shared downloadFile - handles data URLs
  downloadFile(dataUrl, filename);
}
