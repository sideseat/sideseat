import { useState, useCallback, useEffect, useRef } from "react";
import { Document, Page, pdfjs } from "react-pdf";
import "react-pdf/dist/Page/AnnotationLayer.css";
import "react-pdf/dist/Page/TextLayer.css";
import { ZoomIn, ZoomOut, Download, AlertCircle } from "lucide-react";
import { Button } from "@/components/ui/button";

// Vite's ?url suffix returns the resolved URL for the asset
import pdfjsWorker from "pdfjs-dist/build/pdf.worker.min.mjs?url";

pdfjs.GlobalWorkerOptions.workerSrc = pdfjsWorker;

// Constants hoisted to avoid recreation
const MIN_SCALE = 0.5;
const MAX_SCALE = 3;
const SCALE_STEP = 0.25;

// Memoized options for react-pdf Document to prevent unnecessary reloads
const DOCUMENT_OPTIONS = { withCredentials: true } as const;

interface PdfLightboxContentProps {
  src: string;
  showControls: boolean;
  onDownload: () => void;
  onHeaderInfo: (info: string) => void;
}

function LoadingSpinner() {
  return (
    <div className="flex items-center justify-center min-h-[200px]">
      <div className="w-10 h-10 border-2 border-white/20 border-t-white/80 rounded-full animate-spin" />
    </div>
  );
}

function ErrorState({ message, onDownload }: { message: string; onDownload: () => void }) {
  return (
    <div className="absolute inset-0 flex items-center justify-center">
      <div className="flex flex-col items-center gap-4 p-8 bg-neutral-900/80 rounded-xl border border-white/10 max-w-md text-center">
        <AlertCircle className="h-12 w-12 text-red-400" />
        <p className="text-neutral-300">{message}</p>
        <Button
          variant="ghost"
          className="text-white border border-white/20 bg-white/5 hover:bg-white/10"
          onClick={onDownload}
        >
          <Download className="h-4 w-4 mr-2" />
          Download PDF
        </Button>
      </div>
    </div>
  );
}

export default function PdfLightboxContent({
  src,
  showControls,
  onDownload,
  onHeaderInfo,
}: PdfLightboxContentProps) {
  const [numPages, setNumPages] = useState<number>(0);
  const [currentPage, setCurrentPage] = useState(1);
  const [scale, setScale] = useState(1);
  const [loadError, setLoadError] = useState<string | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const pageRefs = useRef<Map<number, HTMLDivElement>>(new Map());

  // Reset state when src changes
  useEffect(() => {
    setCurrentPage(1);
    setScale(1);
    setLoadError(null);
    setNumPages(0);
    pageRefs.current.clear();
  }, [src]);

  // Update header info when page count or current page changes
  useEffect(() => {
    if (numPages > 0) {
      onHeaderInfo(`Page ${currentPage} of ${numPages}`);
    }
  }, [currentPage, numPages, onHeaderInfo]);

  // Track visible page on scroll
  useEffect(() => {
    const container = containerRef.current;
    if (!container || numPages === 0) return;

    const handleScroll = () => {
      const containerRect = container.getBoundingClientRect();
      const containerCenter = containerRect.top + containerRect.height / 2;

      let closestPage = 1;
      let closestDistance = Infinity;

      pageRefs.current.forEach((element, pageNum) => {
        const rect = element.getBoundingClientRect();
        const pageCenter = rect.top + rect.height / 2;
        const distance = Math.abs(pageCenter - containerCenter);

        if (distance < closestDistance) {
          closestDistance = distance;
          closestPage = pageNum;
        }
      });

      setCurrentPage(closestPage);
    };

    container.addEventListener("scroll", handleScroll, { passive: true });
    return () => container.removeEventListener("scroll", handleScroll);
  }, [numPages]);

  // Keyboard controls for PDF
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      switch (e.key) {
        case "+":
        case "=":
          setScale((s) => Math.min(MAX_SCALE, s + SCALE_STEP));
          break;
        case "-":
          setScale((s) => Math.max(MIN_SCALE, s - SCALE_STEP));
          break;
        case "0":
          setScale(1);
          break;
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, []);

  const handleLoadSuccess = useCallback(({ numPages: pages }: { numPages: number }) => {
    setNumPages(pages);
  }, []);

  const handleLoadError = useCallback((error: Error) => {
    if (error.message.includes("password")) {
      setLoadError("Password-protected PDFs are not supported");
    } else {
      setLoadError("Failed to load PDF");
    }
  }, []);

  const handleZoomIn = useCallback(() => {
    setScale((s) => Math.min(MAX_SCALE, s + SCALE_STEP));
  }, []);

  const handleZoomOut = useCallback(() => {
    setScale((s) => Math.max(MIN_SCALE, s - SCALE_STEP));
  }, []);

  const handleResetZoom = useCallback(() => {
    setScale(1);
  }, []);

  const setPageRef = useCallback((pageNum: number, element: HTMLDivElement | null) => {
    if (element) {
      pageRefs.current.set(pageNum, element);
    } else {
      pageRefs.current.delete(pageNum);
    }
  }, []);

  if (loadError) {
    return <ErrorState message={loadError} onDownload={onDownload} />;
  }

  const scalePercent = Math.round(scale * 100);

  return (
    <>
      {/* PDF content area - scrollable container */}
      <div ref={containerRef} className="absolute inset-0 overflow-auto pt-16 pb-24">
        <Document
          file={src}
          onLoadSuccess={handleLoadSuccess}
          onLoadError={handleLoadError}
          loading={<LoadingSpinner />}
          options={DOCUMENT_OPTIONS}
          className="flex flex-col items-center gap-4 py-4"
        >
          {Array.from({ length: numPages }, (_, index) => (
            <div key={index + 1} ref={(el) => setPageRef(index + 1, el)}>
              <Page
                pageNumber={index + 1}
                scale={scale}
                loading={<LoadingSpinner />}
                renderTextLayer={true}
                renderAnnotationLayer={true}
                className="shadow-2xl"
              />
            </div>
          ))}
        </Document>
      </div>

      {/* Bottom toolbar */}
      <div
        className={`absolute bottom-6 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-2 py-1.5 bg-black/70 backdrop-blur-sm rounded-full border border-white/10 transition-opacity duration-300 ${showControls ? "opacity-100" : "opacity-0"}`}
      >
        {/* Zoom controls */}
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={handleZoomOut}
          disabled={scale <= MIN_SCALE}
          title="Zoom out (-)"
        >
          <ZoomOut className="h-4 w-4" />
        </Button>

        <button
          className="min-w-15 px-2 py-1 text-xs font-medium text-neutral-300 hover:text-white hover:bg-white/10 rounded-full transition-colors"
          onClick={handleResetZoom}
          title="Reset zoom (0)"
        >
          {scalePercent}%
        </button>

        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-neutral-400 hover:text-white hover:bg-white/10 rounded-full"
          onClick={handleZoomIn}
          disabled={scale >= MAX_SCALE}
          title="Zoom in (+)"
        >
          <ZoomIn className="h-4 w-4" />
        </Button>

        <div className="w-px h-5 bg-white/20 mx-1" />

        {/* Download */}
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

        {/* Page indicator (only show if multiple pages) */}
        {numPages > 1 && (
          <>
            <div className="w-px h-5 bg-white/20 mx-1" />
            <span className="px-2 text-xs text-neutral-300 min-w-16 text-center">
              {currentPage} / {numPages}
            </span>
          </>
        )}
      </div>
    </>
  );
}
