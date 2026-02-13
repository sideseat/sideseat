import { useMemo, useState, useCallback, useEffect } from "react";
import type { LucideIcon } from "lucide-react";
import { Image, Music, Video, FileText, File, Maximize2, Download, Eye } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useFilesClient } from "@/lib/app-context";
import {
  downloadFromUrl,
  formatBytes,
  getExtensionForMediaType,
  isPlaceholderData,
  getMediaTypeLabel,
} from "@/lib/utils";
import { useMediaGallery } from "../image-gallery-context";

interface BaseMediaProps {
  mediaType?: string;
  source: string;
  data: string;
  /** Project ID for resolving file references */
  projectId?: string;
}

interface ImageContentProps extends BaseMediaProps {
  type: "image";
  detail?: string;
}

interface AudioContentProps extends BaseMediaProps {
  type: "audio";
}

interface VideoContentProps extends BaseMediaProps {
  type: "video";
}

interface DocumentContentProps extends BaseMediaProps {
  type: "document";
  name?: string;
}

interface FileContentProps extends BaseMediaProps {
  type: "file";
  name?: string;
}

type MediaContentProps =
  | ImageContentProps
  | AudioContentProps
  | VideoContentProps
  | DocumentContentProps
  | FileContentProps;

interface MediaConfig {
  icon: LucideIcon;
  iconClass: string;
  label: string;
  extra?: string;
}

function getMediaConfig(props: MediaContentProps): MediaConfig {
  switch (props.type) {
    case "image":
      return {
        icon: Image,
        iconClass: "text-blue-600 dark:text-blue-400",
        label: "Image",
        extra: props.detail ? `(${props.detail})` : undefined,
      };
    case "audio":
      return {
        icon: Music,
        iconClass: "text-green-600 dark:text-green-400",
        label: "Audio",
      };
    case "video":
      return {
        icon: Video,
        iconClass: "text-purple-600 dark:text-purple-400",
        label: "Video",
      };
    case "document":
      return {
        icon: FileText,
        iconClass: "text-orange-600 dark:text-orange-400",
        label: props.name || "Document",
      };
    case "file":
      return {
        icon: File,
        iconClass: "text-gray-600 dark:text-gray-400",
        label: props.name || "File",
      };
  }
}

/** Generate download filename with correct extension */
function getDownloadFilename(name?: string, mediaType?: string, fallbackName = "download"): string {
  const ext = getExtensionForMediaType(mediaType);

  if (name) {
    // Check if name already has the correct extension
    const nameLower = name.toLowerCase();
    const extLower = ext.toLowerCase();
    if (nameLower.endsWith(`.${extLower}`)) {
      return name;
    }
    // Check if name has any extension
    const lastDot = name.lastIndexOf(".");
    if (lastDot > 0) {
      // Replace existing extension with correct one
      return `${name.substring(0, lastDot)}.${ext}`;
    }
    // Add extension
    return `${name}.${ext}`;
  }

  return `${fallbackName}.${ext}`;
}

/** Image viewer with gallery-based lightbox */
function ImageViewer({
  src,
  mediaType,
  onError,
}: {
  src: string;
  mediaType?: string;
  onError: () => void;
}) {
  const { openMedia } = useMediaGallery();
  const [isLoaded, setIsLoaded] = useState(false);
  const [dimensions, setDimensions] = useState<{ w: number; h: number } | null>(null);
  const [fileSize, setFileSize] = useState<number | null>(null);

  const typeLabel = getMediaTypeLabel(mediaType);

  // Reset state when src changes
  useEffect(() => {
    setIsLoaded(false);
    setDimensions(null);
    setFileSize(null);
  }, [src]);

  const handleLoad = useCallback((e: React.SyntheticEvent<HTMLImageElement>) => {
    const img = e.currentTarget;
    setDimensions({ w: img.naturalWidth, h: img.naturalHeight });
    setIsLoaded(true);
  }, []);

  // Get file size
  useEffect(() => {
    if (src.startsWith("data:")) {
      const base64Part = src.split(",")[1];
      if (base64Part) {
        setFileSize(Math.floor(base64Part.length * 0.75));
      }
      return;
    }

    const controller = new AbortController();
    fetch(src, { method: "HEAD", signal: controller.signal, credentials: "include" })
      .then((res) => {
        const len = res.headers.get("content-length");
        if (len) setFileSize(parseInt(len, 10));
      })
      .catch(() => {});

    return () => controller.abort();
  }, [src]);

  const handleDownload = useCallback(() => {
    downloadFromUrl(src, getDownloadFilename(undefined, mediaType, "image"));
  }, [src, mediaType]);

  const handleOpen = useCallback(() => {
    openMedia(src);
  }, [src, openMedia]);

  return (
    <div className="rounded-lg border border-border/40 bg-muted/20 overflow-hidden inline-block shadow-sm">
      {/* Image container with loading state */}
      <div className="relative bg-black/5 dark:bg-white/5">
        {/* Loading skeleton */}
        {!isLoaded && (
          <div className="absolute inset-0 flex items-center justify-center min-h-[120px] min-w-40">
            <div className="w-8 h-8 border-2 border-muted-foreground/20 border-t-muted-foreground/60 rounded-full animate-spin" />
          </div>
        )}
        {/* Image */}
        <img
          src={src}
          alt="Image content"
          crossOrigin="use-credentials"
          className={`block max-w-full max-h-80 object-contain cursor-pointer transition-opacity duration-200 ${isLoaded ? "opacity-100" : "opacity-0"}`}
          onClick={handleOpen}
          onLoad={handleLoad}
          onError={onError}
        />
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-1.5 px-2.5 py-1.5 border-t border-border/30 bg-muted/40 text-muted-foreground">
        <span className="text-[11px] font-semibold uppercase tracking-wide">{typeLabel}</span>
        {dimensions && (
          <>
            <span className="text-[11px] opacity-40">|</span>
            <span className="text-[11px]">
              {dimensions.w}×{dimensions.h}
            </span>
          </>
        )}
        {fileSize != null && fileSize > 0 && (
          <>
            <span className="text-[11px] opacity-40">|</span>
            <span className="text-[11px]">{formatBytes(fileSize)}</span>
          </>
        )}

        <div className="flex-1" />

        <Button
          variant="ghost"
          size="icon"
          className="h-5 w-5 opacity-60 hover:opacity-100"
          onClick={handleOpen}
          title="Enlarge"
        >
          <Maximize2 className="h-3 w-3" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-5 w-5 opacity-60 hover:opacity-100"
          onClick={handleDownload}
          title="Download"
        >
          <Download className="h-3 w-3" />
        </Button>
      </div>
    </div>
  );
}

export function MediaContent(props: MediaContentProps) {
  const { type, mediaType, source, data, projectId } = props;
  const config = getMediaConfig(props);
  const Icon = config.icon;
  const filesClient = useFilesClient();
  const [loadError, setLoadError] = useState(false);
  const { openMedia } = useMediaGallery();

  // Check if data is a placeholder (not actual content)
  const isPlaceholder = isPlaceholderData(data);
  const isPdf = mediaType === "application/pdf";

  // Resolve the media URL using FilesClient
  const resolvedUrl = useMemo(() => {
    // Don't resolve placeholder data
    if (isPlaceholder) return null;

    if (!projectId) {
      if (source === "url") return data;
      if (source === "base64") {
        const mime = mediaType || "application/octet-stream";
        return `data:${mime};base64,${data}`;
      }
      return null;
    }
    return filesClient.resolveContentBlockSource(projectId, source, data, mediaType);
  }, [filesClient, projectId, source, data, mediaType, isPlaceholder]);

  const handleOpenPdf = useCallback(() => {
    if (resolvedUrl && isPdf) {
      openMedia(resolvedUrl);
    }
  }, [resolvedUrl, isPdf, openMedia]);

  const canRenderMedia =
    resolvedUrl && !loadError && (type === "image" || type === "audio" || type === "video");

  if (canRenderMedia) {
    if (type === "image") {
      return (
        <ImageViewer src={resolvedUrl} mediaType={mediaType} onError={() => setLoadError(true)} />
      );
    }

    if (type === "audio") {
      return (
        <div className="rounded-md border border-border/50 bg-muted/30 overflow-hidden">
          <div className="px-3 py-2">
            <audio
              controls
              crossOrigin="use-credentials"
              className="w-full max-w-md"
              onError={() => setLoadError(true)}
            >
              <source src={resolvedUrl} type={mediaType || "audio/mpeg"} />
              Your browser does not support the audio element.
            </audio>
          </div>
          <div className="flex items-center gap-2 px-3 py-1.5 border-t border-border/30 bg-muted/50">
            <span className="text-xs font-medium text-muted-foreground">
              {getMediaTypeLabel(mediaType)}
            </span>
            <div className="flex-1" />
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              onClick={() =>
                downloadFromUrl(resolvedUrl, getDownloadFilename(undefined, mediaType, "audio"))
              }
              title="Download"
            >
              <Download className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      );
    }

    if (type === "video") {
      return (
        <div className="rounded-md border border-border/50 bg-muted/30 overflow-hidden">
          <video
            controls
            crossOrigin="use-credentials"
            className="max-w-full max-h-96"
            onError={() => setLoadError(true)}
          >
            <source src={resolvedUrl} type={mediaType || "video/mp4"} />
            Your browser does not support the video element.
          </video>
          <div className="flex items-center gap-2 px-3 py-1.5 border-t border-border/30 bg-muted/50">
            <span className="text-xs font-medium text-muted-foreground">
              {getMediaTypeLabel(mediaType)}
            </span>
            <div className="flex-1" />
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              onClick={() =>
                downloadFromUrl(resolvedUrl, getDownloadFilename(undefined, mediaType, "video"))
              }
              title="Download"
            >
              <Download className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      );
    }
  }

  // Fallback placeholder with download for documents/files
  const fileName = props.type === "document" || props.type === "file" ? props.name : undefined;
  const canDownload = resolvedUrl && (type === "document" || type === "file");

  return (
    <div
      className={`rounded-md border border-border/50 bg-muted/30 px-3 py-2 ${isPdf && resolvedUrl ? "cursor-pointer hover:bg-muted/50 transition-colors" : ""}`}
      onClick={isPdf && resolvedUrl ? handleOpenPdf : undefined}
    >
      <div className="flex items-center gap-2 min-h-6">
        <Icon className={`h-4 w-4 ${config.iconClass}`} aria-hidden="true" />
        <span className="text-sm font-medium">{config.label}</span>
        <span className="text-xs text-muted-foreground">
          {getMediaTypeLabel(mediaType)}
          {config.extra && ` ${config.extra}`}
        </span>
        <div className="flex-1" />
        {isPdf && resolvedUrl && (
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={(e) => {
              e.stopPropagation();
              handleOpenPdf();
            }}
            title="View PDF"
          >
            <Eye className="h-3.5 w-3.5" />
          </Button>
        )}
        {canDownload && (
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={(e) => {
              e.stopPropagation();
              downloadFromUrl(resolvedUrl, getDownloadFilename(fileName, mediaType, type));
            }}
            title="Download"
          >
            <Download className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
    </div>
  );
}
