/* eslint-disable react-refresh/only-export-components */
import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  useMemo,
  lazy,
  Suspense,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { X, ZoomIn, ZoomOut, RotateCcw, Download, ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  downloadFromUrl,
  formatBytes,
  getExtensionForMediaType,
  isPlaceholderData,
  getMediaTypeLabel,
} from "@/lib/utils";
import { useFilesClient } from "@/lib/app-context";
import type { FilesClient } from "@/api/files/client";
import type { Block, ContentBlock } from "@/api/otel/types";

// Lazy load PDF content to avoid ~1MB bundle impact
const PdfLightboxContent = lazy(() => import("./pdf-lightbox-content"));

interface MediaEntry {
  type: "image" | "pdf";
  src: string;
  mediaType?: string;
  typeLabel: string;
}

interface MediaGalleryContextValue {
  openMedia: (src: string) => void;
}

const MediaGalleryContext = createContext<MediaGalleryContextValue | null>(null);

export function useMediaGallery(): MediaGalleryContextValue {
  const context = useContext(MediaGalleryContext);
  if (!context) {
    throw new Error("useMediaGallery must be used within an MediaGalleryProvider");
  }
  return context;
}

// Backward compatible alias
export const useImageGallery = useMediaGallery;

function getDownloadFilename(type: "image" | "pdf", mediaType?: string): string {
  const ext = getExtensionForMediaType(mediaType);
  return type === "pdf" ? `document.${ext}` : `image.${ext}`;
}

/** Resolve media URL from source and data */
function resolveMediaUrl(
  filesClient: FilesClient,
  projectId: string | undefined,
  source: string,
  data: string,
  mediaType?: string,
): string | null {
  if (isPlaceholderData(data)) return null;

  if (!projectId) {
    if (source === "url") return data;
    if (source === "base64") {
      const mime = mediaType || "application/octet-stream";
      return `data:${mime};base64,${data}`;
    }
    return null;
  }
  return filesClient.resolveContentBlockSource(projectId, source, data, mediaType);
}

/** Extract media from a content block if it's an image or PDF */
function extractMediaFromContent(
  content: ContentBlock,
  filesClient: FilesClient,
  projectId?: string,
): MediaEntry | null {
  if (content.type === "image") {
    const src = resolveMediaUrl(
      filesClient,
      projectId,
      content.source,
      content.data,
      content.media_type,
    );
    if (src) {
      return {
        type: "image",
        src,
        mediaType: content.media_type,
        typeLabel: getMediaTypeLabel(content.media_type, "IMAGE"),
      };
    }
  }

  // PDF documents
  if (content.type === "document" && content.media_type === "application/pdf") {
    const src = resolveMediaUrl(
      filesClient,
      projectId,
      content.source,
      content.data,
      content.media_type,
    );
    if (src) {
      return {
        type: "pdf",
        src,
        mediaType: content.media_type,
        typeLabel: "PDF",
      };
    }
  }

  return null;
}

/** Try to find embedded media in a tool result value */
function findEmbeddedMedia(
  value: unknown,
  filesClient: FilesClient,
  projectId?: string,
): MediaEntry[] {
  const media: MediaEntry[] = [];

  if (!value || typeof value !== "object") return media;

  if (Array.isArray(value)) {
    for (const item of value) {
      media.push(...findEmbeddedMedia(item, filesClient, projectId));
    }
    return media;
  }

  const obj = value as Record<string, unknown>;

  // Check if this object is an image
  const type = obj.type as string | undefined;
  if (type === "image" && typeof obj.data === "string") {
    const source = (obj.source as string) || "base64";
    const mediaType = obj.media_type as string | undefined;
    const src = resolveMediaUrl(filesClient, projectId, source, obj.data, mediaType);
    if (src) {
      media.push({
        type: "image",
        src,
        mediaType,
        typeLabel: getMediaTypeLabel(mediaType, "IMAGE"),
      });
    }
  }

  // Check if this object is a PDF document
  if (type === "document" && typeof obj.data === "string") {
    const mediaType = obj.media_type as string | undefined;
    if (mediaType === "application/pdf") {
      const source = (obj.source as string) || "base64";
      const src = resolveMediaUrl(filesClient, projectId, source, obj.data, mediaType);
      if (src) {
        media.push({
          type: "pdf",
          src,
          mediaType,
          typeLabel: "PDF",
        });
      }
    }
  }

  // Check for base64 image data in common field patterns
  const dataField = obj.data ?? obj.image ?? obj.base64 ?? obj.content;
  if (typeof dataField === "string") {
    const mediaType = (obj.media_type ?? obj.mediaType ?? obj.mime_type ?? obj.mimeType) as
      | string
      | undefined;
    // Check if it looks like base64 image data or a URL
    if (
      mediaType?.startsWith("image/") ||
      (dataField.length > 100 && /^[A-Za-z0-9+/=]+$/.test(dataField.slice(0, 100)))
    ) {
      const source = dataField.startsWith("http") ? "url" : "base64";
      const src = resolveMediaUrl(filesClient, projectId, source, dataField, mediaType);
      if (src && !media.some((m) => m.src === src)) {
        media.push({
          type: "image",
          src,
          mediaType,
          typeLabel: getMediaTypeLabel(mediaType, "IMAGE"),
        });
      }
    }
  }

  // Recursively check nested objects
  for (const key of Object.keys(obj)) {
    if (key !== "data" && key !== "image" && key !== "base64" && key !== "content") {
      const nested = obj[key];
      if (nested && typeof nested === "object") {
        media.push(...findEmbeddedMedia(nested, filesClient, projectId));
      }
    }
  }

  return media;
}

/** Extract all media (images and PDFs) from blocks */
function extractMediaFromBlocks(
  blocks: Block[],
  filesClient: FilesClient,
  projectId?: string,
): MediaEntry[] {
  const media: MediaEntry[] = [];
  const seenSrcs = new Set<string>();

  for (const block of blocks) {
    const content = block.content;

    // Direct media content (images and PDFs)
    const directMedia = extractMediaFromContent(content, filesClient, projectId);
    if (directMedia && !seenSrcs.has(directMedia.src)) {
      seenSrcs.add(directMedia.src);
      media.push(directMedia);
    }

    // Media in tool results
    if (content.type === "tool_result" && content.content) {
      const embedded = findEmbeddedMedia(content.content, filesClient, projectId);
      for (const m of embedded) {
        if (!seenSrcs.has(m.src)) {
          seenSrcs.add(m.src);
          media.push(m);
        }
      }
    }
  }

  return media;
}

// Constants for image zoom
const IMAGE_MIN_ZOOM = 0.5;
const IMAGE_MAX_ZOOM = 4;
const IMAGE_ZOOM_STEP = 0.5;

/** Loading spinner for lazy-loaded content */
function LoadingSpinner() {
  return (
    <div className="flex items-center justify-center min-h-[200px]">
      <div className="w-10 h-10 border-2 border-white/20 border-t-white/80 rounded-full animate-spin" />
    </div>
  );
}

/** Image-specific lightbox content with zoom, pan, and rotation */
function ImageLightboxContent({
  src,
  showControls,
  onDownload,
  onHeaderInfo,
}: {
  src: string;
  showControls: boolean;
  onDownload: () => void;
  onHeaderInfo: (info: string) => void;
}) {
  const [zoom, setZoom] = useState(1);
  const [rotation, setRotation] = useState(0);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  const [isDragging, setIsDragging] = useState(false);
  const [dragStart, setDragStart] = useState({ x: 0, y: 0 });

  // Reset state when src changes
  useEffect(() => {
    setZoom(1);
    setRotation(0);
    setPosition({ x: 0, y: 0 });
  }, [src]);

  // Load image metadata
  useEffect(() => {
    const img = new window.Image();
    img.crossOrigin = "use-credentials";
    img.onload = () => {
      onHeaderInfo(`${img.naturalWidth}×${img.naturalHeight}`);
    };
    img.onerror = () => {
      // Silently handle error - dimensions just won't be shown
    };
    img.src = src;
    return () => {
      img.onload = null;
      img.onerror = null;
    };
  }, [src, onHeaderInfo]);

  const handleZoomIn = useCallback(() => {
    setZoom((z) => Math.min(z + IMAGE_ZOOM_STEP, IMAGE_MAX_ZOOM));
  }, []);

  const handleZoomOut = useCallback(() => {
    setZoom((z) => Math.max(z - IMAGE_ZOOM_STEP, IMAGE_MIN_ZOOM));
  }, []);

  const handleRotate = useCallback(() => {
    setRotation((r) => (r + 90) % 360);
  }, []);

  const handleReset = useCallback(() => {
    setZoom(1);
    setRotation(0);
    setPosition({ x: 0, y: 0 });
  }, []);

  // Keyboard controls for images
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      switch (e.key) {
        case "+":
        case "=":
          handleZoomIn();
          break;
        case "-":
          handleZoomOut();
          break;
        case "r":
        case "R":
          handleRotate();
          break;
        case "0":
          handleReset();
          break;
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [handleZoomIn, handleZoomOut, handleRotate, handleReset]);

  // Mouse wheel zoom
  const handleWheel = useCallback((e: React.WheelEvent) => {
    e.preventDefault();
    const delta = e.deltaY > 0 ? -0.2 : 0.2;
    setZoom((z) => Math.min(Math.max(z + delta, IMAGE_MIN_ZOOM), IMAGE_MAX_ZOOM));
  }, []);

  // Pan/drag handlers
  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      if (zoom > 1) {
        setIsDragging(true);
        setDragStart({ x: e.clientX - position.x, y: e.clientY - position.y });
      }
    },
    [zoom, position],
  );

  const handleMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (isDragging) {
        setPosition({
          x: e.clientX - dragStart.x,
          y: e.clientY - dragStart.y,
        });
      }
    },
    [isDragging, dragStart],
  );

  const handleMouseUp = useCallback(() => {
    setIsDragging(false);
  }, []);

  const zoomPercent = Math.round(zoom * 100);

  return (
    <>
      {/* Image area */}
      <div
        className="absolute inset-0 flex items-center justify-center overflow-hidden"
        onWheel={handleWheel}
        onMouseMove={handleMouseMove}
        onMouseUp={handleMouseUp}
        onMouseLeave={handleMouseUp}
      >
        <img
          src={src}
          alt="Expanded view"
          crossOrigin="use-credentials"
          className={`max-w-none transition-transform ${isDragging ? "cursor-grabbing" : zoom > 1 ? "cursor-grab" : "cursor-zoom-in"}`}
          style={{
            transform: `translate(${position.x}px, ${position.y}px) scale(${zoom}) rotate(${rotation}deg)`,
            transitionDuration: isDragging ? "0ms" : "150ms",
          }}
          onMouseDown={handleMouseDown}
          onClick={(e) => {
            e.stopPropagation();
            if (!isDragging && zoom === 1) {
              handleZoomIn();
            }
          }}
          draggable={false}
        />
      </div>

      {/* Bottom toolbar */}
      <div
        className={`absolute bottom-6 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-2 py-1.5 bg-black/70 backdrop-blur-sm rounded-full border border-white/10 transition-opacity duration-300 ${showControls ? "opacity-100" : "opacity-0"}`}
      >
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={handleZoomOut}
          disabled={zoom <= IMAGE_MIN_ZOOM}
          title="Zoom out (-)"
        >
          <ZoomOut className="h-4 w-4" />
        </Button>

        <button
          className="min-w-15 px-2 py-1 text-xs font-medium text-neutral-300 hover:text-white hover:bg-white/10 rounded-full transition-colors"
          onClick={handleReset}
          title="Reset (0)"
        >
          {zoomPercent}%
        </button>

        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={handleZoomIn}
          disabled={zoom >= IMAGE_MAX_ZOOM}
          title="Zoom in (+)"
        >
          <ZoomIn className="h-4 w-4" />
        </Button>

        <div className="w-px h-5 bg-white/20 mx-1" />

        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={handleRotate}
          title="Rotate (R)"
        >
          <RotateCcw className="h-4 w-4" />
        </Button>

        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={(e) => {
            e.stopPropagation();
            onDownload();
          }}
          title="Download"
        >
          <Download className="h-4 w-4" />
        </Button>
      </div>
    </>
  );
}

/** Shared media lightbox wrapper with navigation and controls */
function MediaLightbox({
  entry,
  onClose,
  onNext,
  onPrev,
  hasNext,
  hasPrev,
  currentIndex,
  totalCount,
}: {
  entry: MediaEntry;
  onClose: () => void;
  onNext: () => void;
  onPrev: () => void;
  hasNext: boolean;
  hasPrev: boolean;
  currentIndex: number;
  totalCount: number;
}) {
  const [showControls, setShowControls] = useState(true);
  const [headerInfo, setHeaderInfo] = useState<string>("");
  const [fileSize, setFileSize] = useState<number | null>(null);

  // Reset header info when entry changes
  useEffect(() => {
    setHeaderInfo("");
    setFileSize(null);
  }, [entry.src]);

  // Get file size
  useEffect(() => {
    const controller = new AbortController();
    if (entry.src.startsWith("data:")) {
      const base64Part = entry.src.split(",")[1];
      if (base64Part) {
        setFileSize(Math.floor(base64Part.length * 0.75));
      }
    } else {
      fetch(entry.src, { method: "HEAD", credentials: "include", signal: controller.signal })
        .then((res) => {
          const len = res.headers.get("content-length");
          if (len) setFileSize(parseInt(len, 10));
        })
        .catch(() => {});
    }
    return () => controller.abort();
  }, [entry.src]);

  const handleNext = useCallback(() => {
    if (hasNext) {
      onNext();
    }
  }, [hasNext, onNext]);

  const handlePrev = useCallback(() => {
    if (hasPrev) {
      onPrev();
    }
  }, [hasPrev, onPrev]);

  // Shared keyboard controls (Escape, Arrow nav)
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      switch (e.key) {
        case "Escape":
          onClose();
          break;
        case "ArrowLeft":
          e.preventDefault();
          handlePrev();
          break;
        case "ArrowRight":
          e.preventDefault();
          handleNext();
          break;
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onClose, handlePrev, handleNext]);

  // Hide controls after inactivity
  useEffect(() => {
    let timeout: ReturnType<typeof setTimeout>;
    const handleMove = () => {
      setShowControls(true);
      clearTimeout(timeout);
      timeout = setTimeout(() => setShowControls(false), 3000);
    };
    window.addEventListener("mousemove", handleMove);
    timeout = setTimeout(() => setShowControls(false), 3000);
    return () => {
      window.removeEventListener("mousemove", handleMove);
      clearTimeout(timeout);
    };
  }, []);

  // Lock body scroll when lightbox is open
  useEffect(() => {
    const originalOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = originalOverflow;
    };
  }, []);

  const handleDownload = useCallback(() => {
    downloadFromUrl(entry.src, getDownloadFilename(entry.type, entry.mediaType));
  }, [entry.src, entry.type, entry.mediaType]);

  const handleHeaderInfo = useCallback((info: string) => {
    setHeaderInfo(info);
  }, []);

  const showNavigation = totalCount > 1;

  return (
    <div
      className="fixed inset-0 z-50 bg-black select-none"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      {/* Top bar */}
      <div
        className={`absolute top-0 left-0 right-0 z-10 flex items-center justify-between px-4 py-3 bg-linear-to-b from-black/80 via-black/40 to-transparent transition-opacity duration-300 ${showControls ? "opacity-100" : "opacity-0"}`}
      >
        <div className="flex items-center gap-3 text-sm text-neutral-400">
          <span className="font-medium text-white">{entry.typeLabel}</span>
          {headerInfo && (
            <>
              <span className="opacity-40">|</span>
              <span>{headerInfo}</span>
            </>
          )}
          {fileSize != null && fileSize > 0 && (
            <>
              <span className="opacity-40">|</span>
              <span>{formatBytes(fileSize)}</span>
            </>
          )}
          {showNavigation && (
            <>
              <span className="opacity-40">|</span>
              <span>
                {currentIndex + 1} / {totalCount}
              </span>
            </>
          )}
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="h-9 w-9 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={onClose}
          title="Close (Esc)"
        >
          <X className="h-5 w-5" />
        </Button>
      </div>

      {/* Navigation buttons */}
      {showNavigation && (
        <>
          <button
            aria-label="Previous"
            className={`absolute left-4 top-1/2 -translate-y-1/2 z-10 h-12 w-12 flex items-center justify-center rounded-full bg-black/50 text-white transition-all duration-150 ${!showControls ? "opacity-0" : hasPrev ? "opacity-100 hover:bg-black/70 cursor-pointer" : "opacity-30 cursor-not-allowed"}`}
            onClick={handlePrev}
            disabled={!hasPrev}
            title="Previous (Left arrow)"
          >
            <ChevronLeft className="h-6 w-6" />
          </button>
          <button
            aria-label="Next"
            className={`absolute right-4 top-1/2 -translate-y-1/2 z-10 h-12 w-12 flex items-center justify-center rounded-full bg-black/50 text-white transition-all duration-150 ${!showControls ? "opacity-0" : hasNext ? "opacity-100 hover:bg-black/70 cursor-pointer" : "opacity-30 cursor-not-allowed"}`}
            onClick={handleNext}
            disabled={!hasNext}
            title="Next (Right arrow)"
          >
            <ChevronRight className="h-6 w-6" />
          </button>
        </>
      )}

      {/* Media content - type specific */}
      {entry.type === "pdf" ? (
        <Suspense fallback={<LoadingSpinner />}>
          <PdfLightboxContent
            src={entry.src}
            showControls={showControls}
            onDownload={handleDownload}
            onHeaderInfo={handleHeaderInfo}
          />
        </Suspense>
      ) : (
        <ImageLightboxContent
          src={entry.src}
          showControls={showControls}
          onDownload={handleDownload}
          onHeaderInfo={handleHeaderInfo}
        />
      )}
    </div>
  );
}

interface MediaGalleryProviderProps {
  children: ReactNode;
  /** Blocks to extract media from */
  blocks: Block[];
  /** Project ID for resolving file references */
  projectId?: string;
}

export function MediaGalleryProvider({ children, blocks, projectId }: MediaGalleryProviderProps) {
  const filesClient = useFilesClient();
  const [openSrc, setOpenSrc] = useState<string | null>(null);

  // Extract all media (images and PDFs) from blocks
  const media = useMemo(
    () => extractMediaFromBlocks(blocks, filesClient, projectId),
    [blocks, filesClient, projectId],
  );

  const openMedia = useCallback((src: string) => {
    setOpenSrc(src);
  }, []);

  const close = useCallback(() => {
    setOpenSrc(null);
  }, []);

  // Find current entry and navigation info
  const currentIndex = openSrc ? media.findIndex((m) => m.src === openSrc) : -1;
  const currentEntry = currentIndex >= 0 ? media[currentIndex] : null;
  const hasNext = currentIndex >= 0 && currentIndex < media.length - 1;
  const hasPrev = currentIndex > 0;

  const goToNext = useCallback(() => {
    if (hasNext) {
      setOpenSrc(media[currentIndex + 1].src);
    }
  }, [hasNext, media, currentIndex]);

  const goToPrev = useCallback(() => {
    if (hasPrev) {
      setOpenSrc(media[currentIndex - 1].src);
    }
  }, [hasPrev, media, currentIndex]);

  const contextValue = useMemo<MediaGalleryContextValue>(() => ({ openMedia }), [openMedia]);

  return (
    <MediaGalleryContext.Provider value={contextValue}>
      {children}
      {openSrc &&
        currentEntry &&
        createPortal(
          <MediaLightbox
            entry={currentEntry}
            onClose={close}
            onNext={goToNext}
            onPrev={goToPrev}
            hasNext={hasNext}
            hasPrev={hasPrev}
            currentIndex={currentIndex}
            totalCount={media.length}
          />,
          document.body,
        )}
    </MediaGalleryContext.Provider>
  );
}

// Backward compatible alias
export const ImageGalleryProvider = MediaGalleryProvider;
