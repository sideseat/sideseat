import { AlertTriangle, Check, Copy, Download, ShieldAlert } from "lucide-react";
import { toast } from "sonner";

import type { CreateApiKeyResponse } from "@/api/api-keys";
import { SCOPE_BADGE_VARIANT } from "@/api/api-keys";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useCopy } from "@/hooks/use-copy";
import { cn } from "@/lib/utils";

interface ApiKeyCreatedDialogProps {
  createdKey: CreateApiKeyResponse | null;
  onClose: () => void;
}

export function ApiKeyCreatedDialog({ createdKey, onClose }: ApiKeyCreatedDialogProps) {
  const { copied, copy } = useCopy();

  if (!createdKey) return null;

  const handleCopy = async () => {
    await copy(createdKey.key);
    toast.success("API key copied to clipboard");
  };

  const handleDownload = () => {
    // Base64 encode key with empty password for OTEL Basic auth format
    const base64Key = btoa(`${createdKey.key}:`);
    const content = `# SideSeat API Key
# Name: ${createdKey.name}
# Scope: ${createdKey.scope}
# Created: ${createdKey.created_at}
# WARNING: Keep this file secure and never commit to version control

SIDESEAT_API_KEY=${createdKey.key}
OTEL_EXPORTER_OTLP_HEADERS=Authorization=Basic%20${base64Key}
`;
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `sideseat-api-key-${createdKey.name.toLowerCase().replace(/\s+/g, "-")}.env`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    toast.success("API key downloaded");
  };

  return (
    <Dialog open={!!createdKey} onOpenChange={() => {}}>
      <DialogContent className="[&>button]:hidden sm:max-w-lg">
        <DialogHeader className="overflow-hidden">
          <DialogTitle className="flex items-center gap-3 overflow-hidden">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-amber-500/15">
              <ShieldAlert className="h-5 w-5 text-amber-500" />
            </div>
            <div className="min-w-0 flex-1 overflow-hidden">
              <span className="block">Save Your API Key</span>
              <span className="block text-sm font-normal text-muted-foreground truncate">
                For {createdKey.name}
              </span>
            </div>
          </DialogTitle>
        </DialogHeader>

        {/* Warning banner */}
        <div className="flex items-start gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-3">
          <AlertTriangle className="h-5 w-5 shrink-0 text-amber-600 dark:text-amber-400 mt-0.5" />
          <div className="text-sm">
            <p className="font-medium text-amber-700 dark:text-amber-300">
              This key will only be shown once
            </p>
            <p className="mt-0.5 text-amber-600/80 dark:text-amber-400/80">
              Copy it now or download the .env file. You won&apos;t be able to see it again.
            </p>
          </div>
        </div>

        {/* Key display */}
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
              Your API Key
            </span>
            <Badge variant={SCOPE_BADGE_VARIANT[createdKey.scope]}>{createdKey.scope}</Badge>
          </div>
          <div className="rounded-lg bg-muted p-4">
            <code className="block text-sm text-foreground font-mono break-all select-all leading-relaxed">
              {createdKey.key}
            </code>
          </div>
        </div>

        {/* Action buttons */}
        <div className="flex gap-3">
          <Button
            className={cn("flex-1 gap-2", copied && "bg-green-600 hover:bg-green-600")}
            onClick={handleCopy}
          >
            {copied ? (
              <>
                <Check className="h-4 w-4" />
                Copied!
              </>
            ) : (
              <>
                <Copy className="h-4 w-4" />
                Copy Key
              </>
            )}
          </Button>
          <Button className="flex-1 gap-2" variant="outline" onClick={handleDownload}>
            <Download className="h-4 w-4" />
            Download .env
          </Button>
        </div>

        <DialogFooter className="sm:justify-center">
          <Button onClick={onClose} variant="secondary" className="w-full">
            I have saved this key
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
